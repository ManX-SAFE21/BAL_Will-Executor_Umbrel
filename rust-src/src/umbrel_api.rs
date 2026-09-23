//! Umbrel-specific HTTP endpoints (the SAFE21 dashboard backend), layered on
//! top of the vanilla upstream bal-server.
//!
//! WHY A SEPARATE MODULE: to keep upstream `src/` pristine so future syncs from
//! the Gitea source are trivial. The only touch-points in upstream files are:
//!   - `lib.rs`            : `pub mod umbrel_api;`   (feature-gated)
//!   - `bal-server.rs main`: `.configure(bal_server::umbrel_api::configure)`
//!   - `db.rs`             : one ALTER TABLE adding the `confirmations` column
//! Everything else the Umbrel product needs lives here.
//!
//! DESIGN: handlers are self-contained. They read configuration from the
//! environment and open their own SQLite connection per request via
//! `crate::db::open_db`, so this module does NOT depend on the binary's
//! private `AppState`. Endpoints are registered under non-colliding paths so
//! the upstream router stays untouched.

use actix_web::{HttpRequest, HttpResponse, Responder, web};
use bitcoin::Transaction;
use hex_conservative::FromHex;
use serde::{Deserialize, Serialize};
use sqlite::{State, Value};
use std::env;
use std::fs;
use std::path::Path;

use crate::db::open_db;

const NETWORKS: [&str; 5] = ["bitcoin", "testnet", "testnet4", "signet", "regtest"];

// ---------------------------------------------------------------------------
// Config helpers (self-contained — read straight from the environment)
// ---------------------------------------------------------------------------

fn db_file() -> String {
    env::var("BAL_SERVER_DB_FILE").unwrap_or_else(|_| "bal.db".to_string())
}

/// Does `addr` parse as a mainnet Bitcoin address?
///
/// The dashboard's address is the one `entrypoint-server.sh` maps to
/// `BAL_SERVER_BITCOIN_ADDRESS`, i.e. mainnet — and it is where every future
/// fee is paid. A typo here silently sends income to an address nobody holds,
/// and nothing downstream would complain, so refuse it at the door.
fn is_valid_bitcoin_address(addr: &str) -> bool {
    use std::str::FromStr;
    bitcoin::Address::from_str(addr)
        .map(|a| a.is_valid_for_network(bitcoin::Network::Bitcoin))
        .unwrap_or(false)
}

fn net_from_str(s: &str) -> bitcoin::Network {
    match s {
        "regtest" => bitcoin::Network::Regtest,
        "testnet" => bitcoin::Network::Testnet,
        "testnet4" => bitcoin::Network::Testnet4,
        "signet" => bitcoin::Network::Signet,
        _ => bitcoin::Network::Bitcoin,
    }
}

/// Read one parameter from a raw query string (everything after `?`).
/// Returns the still-URL-encoded value; callers sanitize as needed.
fn query_param(qs: &str, key: &str) -> Option<String> {
    qs.split('&').find_map(|pair| {
        let mut it = pair.splitn(2, '=');
        match (it.next(), it.next()) {
            (Some(k), v) if k == key => Some(v.unwrap_or("").to_string()),
            _ => None,
        }
    })
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
pub struct StatsResponse {
    pub report_date: String,
    pub chain: String,
    pub totals: i64,
    pub waiting: i64,
    pub sent: i64,
    pub failed: i64,
    pub waiting_profit: i64,
    pub sent_profit: i64,
    pub missed_profit: i64,
    pub unique_inputs: i64,
    // Umbrel additions (need the `confirmations` column):
    pub confirmed_profit: i64,
    pub mempool_profit: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TxListEntry {
    pub txid: String,
    pub network: String,
    pub our_fees: String,
    pub our_address: String,
    pub status: i64,
    pub locktime: i64,
    pub date_creation: String,
    pub confirmations: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TxListResponse {
    pub total: i64,
    pub txs: Vec<TxListEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InputEntry {
    pub in_txid: String,
    pub in_vout: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OutputEntry {
    pub script_pubkey: String,
    pub amount: String,
    pub vout: String,
    pub address: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TxDetailResponse {
    pub txid: String,
    pub network: String,
    pub our_fees: String,
    pub our_address: String,
    pub status: i64,
    pub locktime: i64,
    pub date_creation: String,
    pub date_update: String,
    pub tx: String,
    pub push_err: Option<String>,
    pub network_fees: String,
    pub total_output: String,
    pub inputs: Vec<InputEntry>,
    pub outputs: Vec<OutputEntry>,
    pub confirmations: i64,
}

// ---------------------------------------------------------------------------
// GET /txstats/{network}
// Extended per-network stats: base tbl_stats row + confirmed_profit and
// mempool_profit computed from the `confirmations` column (Umbrel additions).
// ---------------------------------------------------------------------------

async fn stats(path: web::Path<String>) -> impl Responder {
    let chain = path.into_inner();
    if !NETWORKS.contains(&chain.as_str()) {
        return HttpResponse::NotFound().body("unknown network");
    }
    let db = match open_db(&db_file()) {
        Ok(d) => d,
        Err(e) => {
            log::error!("umbrel stats: db open failed: {}", e);
            return HttpResponse::InternalServerError().body("db error");
        }
    };
    // The chain is bound; the subqueries reference the column `s.chain`, so no
    // user text is ever interpolated into the SQL.
    let sql = "SELECT s.report_date, s.chain, s.totals, s.waiting, s.sent, s.failed, \
               s.waiting_profit, s.sent_profit, s.missed_profit, s.unique_inputs, \
               IFNULL((SELECT SUM(CAST(our_fees AS INTEGER)) FROM tbl_tx \
                       WHERE status=1 AND confirmations>0 AND network=s.chain), 0) AS confirmed_profit, \
               IFNULL((SELECT SUM(CAST(our_fees AS INTEGER)) FROM tbl_tx \
                       WHERE status=1 AND confirmations=0 AND network=s.chain), 0) AS mempool_profit \
               FROM tbl_stats s WHERE s.chain = ?";
    let mut stmt = match db.prepare(sql) {
        Ok(s) => s,
        Err(e) => {
            log::error!("umbrel stats: prepare failed: {}", e);
            return HttpResponse::InternalServerError().body("db error");
        }
    };
    if let Err(e) = stmt.bind((1, Value::String(chain.clone()))) {
        log::error!("umbrel stats: bind failed: {}", e);
        return HttpResponse::InternalServerError().body("db error");
    }
    let mut out: Vec<StatsResponse> = Vec::new();
    while let Ok(State::Row) = stmt.next() {
        out.push(StatsResponse {
            report_date: stmt.read::<String, _>("report_date").unwrap_or_default(),
            chain: stmt.read::<String, _>("chain").unwrap_or_default(),
            totals: stmt.read::<i64, _>("totals").unwrap_or(0),
            waiting: stmt.read::<i64, _>("waiting").unwrap_or(0),
            sent: stmt.read::<i64, _>("sent").unwrap_or(0),
            failed: stmt.read::<i64, _>("failed").unwrap_or(0),
            waiting_profit: stmt.read::<i64, _>("waiting_profit").unwrap_or(0),
            sent_profit: stmt.read::<i64, _>("sent_profit").unwrap_or(0),
            missed_profit: stmt.read::<i64, _>("missed_profit").unwrap_or(0),
            unique_inputs: stmt.read::<i64, _>("unique_inputs").unwrap_or(0),
            confirmed_profit: stmt.read::<i64, _>("confirmed_profit").unwrap_or(0),
            mempool_profit: stmt.read::<i64, _>("mempool_profit").unwrap_or(0),
        });
    }
    HttpResponse::Ok().json(out)
}

// ---------------------------------------------------------------------------
// GET /txlist?network=&show_failed=&search=&limit=&offset=&sort=&dir=
// Paginated / searchable / sortable transaction list. Fully parameterized;
// ORDER BY chosen from a fixed whitelist; txid search sanitized to hex.
// ---------------------------------------------------------------------------

async fn txlist(req: HttpRequest) -> impl Responder {
    let qs = req.query_string();
    let network = query_param(qs, "network").filter(|s| !s.is_empty());
    let show_failed = query_param(qs, "show_failed").as_deref() == Some("1");
    let search: String = query_param(qs, "search")
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(64)
        .collect();
    let limit: i64 = query_param(qs, "limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(25)
        .clamp(1, 200);
    let offset: i64 = query_param(qs, "offset")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
        .max(0);
    let order_expr = match query_param(qs, "sort").as_deref() {
        Some("txid") => "txid",
        Some("network") => "network",
        Some("our_fees") => "CAST(our_fees AS INTEGER)",
        Some("locktime") => "locktime",
        // Lifecycle order of the unified UI "State" column.
        Some("state") => "CASE \
            WHEN status=0 THEN 0 \
            WHEN status=1 AND confirmations>0 THEN 2 \
            WHEN status=1 THEN 1 \
            WHEN status=2 AND confirmations=-1 THEN 3 \
            ELSE 4 END",
        _ => "date_creation",
    };
    let dir = if query_param(qs, "dir").as_deref() == Some("asc") {
        "ASC"
    } else {
        "DESC"
    };

    let mut where_parts: Vec<&str> = vec!["1=1"];
    let mut binds: Vec<Value> = Vec::new();
    if let Some(ref net) = network {
        where_parts.push("network = ?");
        binds.push(Value::String(net.clone()));
    }
    if !show_failed {
        where_parts.push("status IN (0,1)");
    }
    if !search.is_empty() {
        where_parts.push("txid LIKE ?");
        binds.push(Value::String(format!("{}%", search)));
    }
    let where_sql = where_parts.join(" AND ");

    let db = match open_db(&db_file()) {
        Ok(d) => d,
        Err(e) => {
            log::error!("umbrel txlist: db open failed: {}", e);
            return HttpResponse::InternalServerError().body("db error");
        }
    };

    let count_sql = format!("SELECT COUNT(*) AS c FROM tbl_tx WHERE {}", where_sql);
    let mut total: i64 = 0;
    if let Ok(mut cstmt) = db.prepare(&count_sql) {
        for (i, v) in binds.iter().enumerate() {
            let _ = cstmt.bind((i + 1, v.clone()));
        }
        if let Ok(State::Row) = cstmt.next() {
            total = cstmt.read::<i64, _>("c").unwrap_or(0);
        }
    }

    let select_sql = format!(
        "SELECT txid, network, our_fees, our_address, status, locktime, date_creation, confirmations \
         FROM tbl_tx WHERE {} ORDER BY {} {} LIMIT ? OFFSET ?",
        where_sql, order_expr, dir
    );
    let mut txs: Vec<TxListEntry> = Vec::new();
    if let Ok(mut stmt) = db.prepare(&select_sql) {
        let mut i: usize = 1;
        for v in &binds {
            let _ = stmt.bind((i, v.clone()));
            i += 1;
        }
        let _ = stmt.bind((i, Value::Integer(limit)));
        let _ = stmt.bind((i + 1, Value::Integer(offset)));
        while let Ok(State::Row) = stmt.next() {
            txs.push(TxListEntry {
                txid: stmt.read::<String, _>("txid").unwrap_or_default(),
                network: stmt.read::<String, _>("network").unwrap_or_default(),
                our_fees: stmt.read::<String, _>("our_fees").unwrap_or_default(),
                our_address: stmt.read::<String, _>("our_address").unwrap_or_default(),
                status: stmt.read::<i64, _>("status").unwrap_or(0),
                locktime: stmt.read::<i64, _>("locktime").unwrap_or(0),
                date_creation: stmt.read::<String, _>("date_creation").unwrap_or_default(),
                confirmations: stmt.read::<i64, _>("confirmations").unwrap_or(0),
            });
        }
    }

    HttpResponse::Ok().json(TxListResponse { total, txs })
}

// ---------------------------------------------------------------------------
// GET /txdetail?txid=<hex>
// Full detail for one transaction, including decoded inputs/outputs.
// ---------------------------------------------------------------------------

async fn txdetail(req: HttpRequest) -> impl Responder {
    let txid: String = query_param(req.query_string(), "txid")
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(64)
        .collect();
    if txid.is_empty() {
        return HttpResponse::BadRequest().body("missing txid");
    }

    let db = match open_db(&db_file()) {
        Ok(d) => d,
        Err(e) => {
            log::error!("umbrel txdetail: db open failed: {}", e);
            return HttpResponse::InternalServerError().body("db error");
        }
    };
    let mut stmt = match db.prepare("SELECT * FROM tbl_tx WHERE txid = ? LIMIT 1") {
        Ok(s) => s,
        Err(e) => {
            log::error!("umbrel txdetail: prepare failed: {}", e);
            return HttpResponse::InternalServerError().body("db error");
        }
    };
    if stmt.bind((1, txid.as_str())).is_err() {
        return HttpResponse::InternalServerError().body("db error");
    }

    let mut data = TxDetailResponse {
        txid: String::new(),
        network: String::new(),
        our_fees: String::new(),
        our_address: String::new(),
        status: 0,
        locktime: 0,
        date_creation: String::new(),
        date_update: String::new(),
        tx: String::new(),
        push_err: None,
        network_fees: String::new(),
        total_output: String::new(),
        inputs: vec![],
        outputs: vec![],
        confirmations: 0,
    };

    if let Ok(State::Row) = stmt.next() {
        data.txid = stmt.read::<String, _>("txid").unwrap_or_default();
        data.network = stmt.read::<String, _>("network").unwrap_or_default();
        data.our_fees = stmt.read::<String, _>("our_fees").unwrap_or_default();
        data.our_address = stmt.read::<String, _>("our_address").unwrap_or_default();
        data.status = stmt.read::<i64, _>("status").unwrap_or(0);
        data.locktime = stmt.read::<i64, _>("locktime").unwrap_or(0);
        data.date_creation = stmt.read::<String, _>("date_creation").unwrap_or_default();
        data.date_update = stmt.read::<String, _>("date_update").unwrap_or_default();
        data.tx = stmt.read::<String, _>("tx").unwrap_or_default();
        data.push_err = stmt.read::<String, _>("push_err").ok();
        data.network_fees = stmt.read::<String, _>("network_fees").unwrap_or_default();
        data.confirmations = stmt.read::<i64, _>("confirmations").unwrap_or(0);

        let bitcoin_network = net_from_str(&data.network);

        if let Ok(mut inp_stmt) = db.prepare("SELECT in_txid, in_vout FROM tbl_inp WHERE txid = ?") {
            let _ = inp_stmt.bind((1, txid.as_str()));
            while let Ok(State::Row) = inp_stmt.next() {
                data.inputs.push(InputEntry {
                    in_txid: inp_stmt.read::<String, _>("in_txid").unwrap_or_default(),
                    in_vout: inp_stmt.read::<String, _>("in_vout").unwrap_or_default(),
                });
            }
        }

        // Decode the raw tx once to recover output addresses for legacy rows.
        let tx_output_data: Vec<(bitcoin::ScriptBuf, u64)> = if !data.tx.is_empty() {
            Vec::<u8>::from_hex(&data.tx)
                .ok()
                .and_then(|b| consensus_deserialize(&b))
                .map(|t| {
                    t.output
                        .into_iter()
                        .map(|o| (o.script_pubkey, o.value.to_sat()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        } else {
            vec![]
        };

        if let Ok(mut out_stmt) = db
            .prepare("SELECT script_pubkey, amount, vout, address FROM tbl_out WHERE txid = ? ORDER BY vout")
        {
            let _ = out_stmt.bind((1, txid.as_str()));
            let mut total_out: u64 = 0;
            while let Ok(State::Row) = out_stmt.next() {
                let script_pubkey = out_stmt.read::<String, _>("script_pubkey").unwrap_or_default();
                let amount = out_stmt.read::<String, _>("amount").unwrap_or_default();
                let vout = out_stmt.read::<String, _>("vout").unwrap_or_default();
                let mut address = out_stmt.read::<String, _>("address").unwrap_or_default();
                if address.is_empty() {
                    let idx: usize = vout.parse().unwrap_or(0);
                    address = tx_output_data
                        .get(idx)
                        .and_then(|(s, _)| {
                            bitcoin::Address::from_script(s.as_script(), bitcoin_network).ok()
                        })
                        .map(|a| a.to_string())
                        .unwrap_or_default();
                }
                if let Ok(sat) = amount.parse::<u64>() {
                    total_out += sat;
                }
                data.outputs.push(OutputEntry {
                    script_pubkey,
                    address,
                    amount,
                    vout,
                });
            }
            data.total_output = total_out.to_string();
        }
    }

    HttpResponse::Ok().json(data)
}

/// Deserialize a raw Bitcoin transaction from bytes (helper kept local so the
/// call sites stay tidy).
fn consensus_deserialize(bytes: &[u8]) -> Option<Transaction> {
    bitcoin::consensus::encode::deserialize::<Transaction>(bytes).ok()
}

// ===========================================================================
// Management endpoints: backup / restore / merge / settings / branding.
// All file/SQL operations are self-contained (own connection per request).
//
// NOTE on settings: the values below (bitcoin address, fee, info) are persisted
// to settings.json, which the container entrypoint reads into the BAL_SERVER_*
// environment at startup. Upstream's config is immutable at runtime, so a
// settings change takes effect on the next container restart (see AGENTS.md).
// ===========================================================================

#[derive(Debug, Serialize, Deserialize, Default)]
struct BitcoinSettings {
    #[serde(default)]
    address: String,
    #[serde(default)]
    fee: u64,
    #[serde(default)]
    info: String,
    #[serde(default)]
    logo_text: String,
    #[serde(default)]
    logo_format: String,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct BackupConfig {
    #[serde(default)]
    backup_path: String,
}

// ---- path helpers (all derived from the db_file directory) ----

fn data_dir() -> std::path::PathBuf {
    Path::new(&db_file())
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

fn settings_path() -> String {
    data_dir().join("settings.json").to_string_lossy().into_owned()
}

fn logo_path() -> String {
    data_dir().join("custom_logo").to_string_lossy().into_owned()
}

fn backup_config_path() -> String {
    data_dir()
        .join("backup_config.json")
        .to_string_lossy()
        .into_owned()
}

fn load_settings() -> BitcoinSettings {
    fs::read_to_string(settings_path())
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}

fn save_settings(s: &BitcoinSettings) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(s).unwrap_or_default();
    fs::write(settings_path(), json)
}

fn load_backup_config() -> BackupConfig {
    fs::read_to_string(backup_config_path())
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}

fn backup_dir() -> String {
    let c = load_backup_config();
    if !c.backup_path.is_empty() {
        return c.backup_path;
    }
    data_dir().join("backup").to_string_lossy().into_owned()
}

// ---- image helpers (for custom branding logo) ----

fn detect_image_format(data: &[u8]) -> Option<&'static str> {
    if data.len() < 4 {
        return None;
    }
    if data[0] == 0x89 && data[1] == b'P' && data[2] == b'N' && data[3] == b'G' {
        return Some("png");
    }
    if data[0] == 0xff && data[1] == 0xd8 {
        return Some("jpeg");
    }
    if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        return Some("webp");
    }
    let head = std::str::from_utf8(&data[..data.len().min(256)]).unwrap_or("");
    if head.trim_start().starts_with("<?xml") || head.trim_start().starts_with("<svg") {
        return Some("svg");
    }
    None
}

fn mime_for_format(fmt: &str) -> &'static str {
    match fmt {
        "png" => "image/png",
        "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

// Recompute per-network tbl_stats from tbl_tx (used after a merge/restore).
const RECOMPUTE_STATS_SQL: &str = "INSERT INTO tbl_stats(report_date, chain, totals, waiting, sent, failed, waiting_profit, sent_profit, missed_profit, unique_inputs) \
SELECT CURRENT_TIMESTAMP, network, COUNT(*), \
SUM(CASE WHEN status=0 AND (locktime < 500000000 OR locktime > CAST(strftime('%s','now') AS INTEGER)) THEN 1 ELSE 0 END), \
SUM(CASE WHEN status=1 THEN 1 ELSE 0 END), \
SUM(CASE WHEN status=2 THEN 1 ELSE 0 END), \
COALESCE(SUM(CASE WHEN status=0 AND (locktime < 500000000 OR locktime > CAST(strftime('%s','now') AS INTEGER)) THEN CAST(our_fees AS INTEGER) ELSE 0 END), 0), \
COALESCE(SUM(CASE WHEN status=1 THEN CAST(our_fees AS INTEGER) ELSE 0 END), 0), \
COALESCE(SUM(CASE WHEN status=2 THEN CAST(our_fees AS INTEGER) ELSE 0 END), 0), 0 \
FROM tbl_tx GROUP BY network";

/// Merge all rows from a server-local SQLite file at `src` into the live db
/// (INSERT OR IGNORE, then recompute stats). Shared by /merge and /restore.
/// Does the SQLite file at `path` actually carry the three tables a merge
/// needs? This has to EXECUTE the query: `prepare()` alone succeeds on any
/// readable SQLite file regardless of its schema, which is how the original
/// check passed everything.
fn db_has_our_tables(path: &str) -> bool {
    let db = match open_db(path) {
        Ok(d) => d,
        Err(_) => return false,
    };
    let mut stmt = match db.prepare(
        "SELECT COUNT(*) AS n FROM sqlite_master \
         WHERE type='table' AND name IN ('tbl_tx','tbl_inp','tbl_out')",
    ) {
        Ok(s) => s,
        Err(_) => return false,
    };
    matches!(stmt.next(), Ok(State::Row)) && stmt.read::<i64, _>("n").unwrap_or(0) == 3
}

/// Merge all rows from a server-local SQLite file at `src` into the live db
/// (INSERT OR IGNORE, then recompute stats). Shared by /merge and /restore.
///
/// Every step is checked: the original ignored all of them with `let _ =` and
/// still answered `{"status":"ok"}`, so a merge that silently did nothing —
/// or that deleted tbl_stats and failed to rebuild it — looked successful.
fn merge_from_db(src: &str) -> HttpResponse {
    let db = match open_db(&db_file()) {
        Ok(d) => d,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .body(format!("{{\"error\":\"db open: {}\"}}", e));
        }
    };

    let fail = |db: &sqlite::Connection, step: &str, e: String| -> HttpResponse {
        let _ = db.execute("ROLLBACK");
        let _ = db.execute("DETACH DATABASE bak");
        log::error!("umbrel merge: {} failed: {}", step, e);
        HttpResponse::InternalServerError()
            .body(format!("{{\"error\":\"{} failed: {}\"}}", step, e))
    };

    if let Err(e) = db.execute("BEGIN IMMEDIATE") {
        return HttpResponse::InternalServerError()
            .body(format!("{{\"error\":\"begin: {}\"}}", e));
    }

    // Bind the path instead of interpolating it into the SQL text.
    match db.prepare("ATTACH DATABASE ? AS bak") {
        Ok(mut stmt) => {
            if let Err(e) = stmt.bind((1, Value::String(src.to_string()))) {
                return fail(&db, "attach-bind", e.to_string());
            }
            if let Err(e) = stmt.next() {
                return fail(&db, "attach", e.to_string());
            }
        }
        Err(e) => return fail(&db, "attach-prepare", e.to_string()),
    }

    // Required: validated to exist before we got here.
    for (step, sql) in [
        ("tbl_tx", "INSERT OR IGNORE INTO tbl_tx SELECT * FROM bak.tbl_tx"),
        ("tbl_inp", "INSERT OR IGNORE INTO tbl_inp SELECT * FROM bak.tbl_inp"),
        ("tbl_out", "INSERT OR IGNORE INTO tbl_out SELECT * FROM bak.tbl_out"),
    ] {
        if let Err(e) = db.execute(sql) {
            return fail(&db, step, e.to_string());
        }
    }

    // Optional: older backups legitimately lack these, so a failure here is
    // reported but must not abort an otherwise good merge.
    let mut skipped: Vec<&str> = Vec::new();
    for (name, sql) in [
        ("tbl_xpub", "INSERT OR IGNORE INTO tbl_xpub(network,xpub) SELECT network,xpub FROM bak.tbl_xpub"),
        ("tbl_address", "INSERT OR IGNORE INTO tbl_address SELECT * FROM bak.tbl_address"),
    ] {
        if db.execute(sql).is_err() {
            skipped.push(name);
        }
    }

    // Stats are derived: deleting them is only safe if the rebuild succeeds.
    if let Err(e) = db.execute("DELETE FROM tbl_stats") {
        return fail(&db, "clear-stats", e.to_string());
    }
    if let Err(e) = db.execute(RECOMPUTE_STATS_SQL) {
        return fail(&db, "recompute-stats", e.to_string());
    }

    let _ = db.execute("DETACH DATABASE bak");
    if let Err(e) = db.execute("COMMIT") {
        return fail(&db, "commit", e.to_string());
    }

    HttpResponse::Ok().json(serde_json::json!({
        "status": "ok",
        "skipped_tables": skipped,
    }))
}

// ---- handlers ----

async fn backup() -> impl Responder {
    match fs::read(db_file()) {
        Ok(data) => {
            let filename = format!("bal-backup-{}.db", chrono::Utc::now().format("%Y%m%d"));
            HttpResponse::Ok()
                .insert_header(("Content-Type", "application/octet-stream"))
                .insert_header((
                    "Content-Disposition",
                    format!("attachment; filename=\"{}\"", filename),
                ))
                .body(data)
        }
        Err(e) => HttpResponse::InternalServerError().body(format!("error: {}", e)),
    }
}

async fn backups_list() -> impl Responder {
    let dir = backup_dir();
    let mut backups: Vec<serde_json::Value> = vec![];
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if let Ok(md) = entry.metadata() {
                if md.is_file() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name.ends_with(".db") {
                        let modified = md
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs() as i64)
                            .unwrap_or(0);
                        backups.push(serde_json::json!({
                            "filename": name,
                            "size": md.len(),
                            "modified": modified
                        }));
                    }
                }
            }
        }
    }
    backups.sort_by(|a, b| {
        b["modified"]
            .as_i64()
            .unwrap_or(0)
            .cmp(&a["modified"].as_i64().unwrap_or(0))
    });
    HttpResponse::Ok().json(backups)
}

async fn backup_config() -> impl Responder {
    HttpResponse::Ok().json(load_backup_config())
}

async fn set_backup_config(body: web::Bytes) -> impl Responder {
    let cfg: BackupConfig = match serde_json::from_slice(&body) {
        Ok(c) => c,
        Err(e) => return HttpResponse::BadRequest().body(format!("Invalid JSON: {}", e)),
    };
    // This value becomes `backup_dir()`, which /restore joins a filename onto
    // and the pusher writes weekly backups into. `restore` sanitizes only the
    // filename, so an unvalidated directory here would move the whole target
    // elsewhere. Empty means "use the default next to the database".
    if !cfg.backup_path.is_empty() {
        let p = Path::new(&cfg.backup_path);
        if !p.is_absolute() {
            return HttpResponse::BadRequest()
                .body("{\"error\":\"backup path must be absolute\"}");
        }
        if p.components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return HttpResponse::BadRequest()
                .body("{\"error\":\"backup path must not contain '..'\"}");
        }
    }
    let json = serde_json::to_string_pretty(&cfg).unwrap_or_default();
    match fs::write(backup_config_path(), json) {
        Ok(_) => HttpResponse::Ok().body("ok"),
        Err(e) => HttpResponse::InternalServerError().body(format!("error: {}", e)),
    }
}

async fn merge(body: web::Bytes) -> impl Responder {
    // Staged inside the app's own data directory, not /tmp: a predictable name
    // in a world-writable directory invites a symlink being planted at that
    // path, and `fs::write` would follow it.
    // Unique per request, not just per process: actix serves requests
    // concurrently, so a name derived only from the PID is the SAME path for
    // two overlapping merges — the second upload would overwrite the first
    // between its validation and its merge, and the database actually merged
    // would not be the one that passed the check.
    let uniq = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = data_dir()
        .join(format!("merge-upload-{}-{}.db.tmp", std::process::id(), uniq))
        .to_string_lossy()
        .into_owned();

    if let Err(e) = fs::write(&tmp, &body) {
        return HttpResponse::InternalServerError()
            .body(format!("{{\"error\":\"cannot write temp file: {}\"}}", e));
    }
    if !db_has_our_tables(&tmp) {
        let _ = fs::remove_file(&tmp);
        return HttpResponse::BadRequest().body(
            "{\"error\":\"not a Will Executor database (tbl_tx/tbl_inp/tbl_out missing)\"}",
        );
    }
    let resp = merge_from_db(&tmp);
    let _ = fs::remove_file(&tmp);
    resp
}

async fn restore(path: web::Path<String>) -> impl Responder {
    let filename = path.into_inner();
    // Prevent path traversal — filename must be a plain name in the backup dir.
    if filename.contains("..") || filename.contains('/') || filename.contains('\\') {
        return HttpResponse::BadRequest().body("{\"error\":\"invalid filename\"}");
    }
    let backup_path = Path::new(&backup_dir()).join(&filename);
    if !backup_path.exists() {
        return HttpResponse::BadRequest()
            .body(format!("{{\"error\":\"backup file '{}' not found\"}}", filename));
    }
    merge_from_db(&backup_path.to_string_lossy())
}

async fn get_settings() -> impl Responder {
    let s = load_settings();
    let has_logo = Path::new(&logo_path()).exists();
    HttpResponse::Ok().json(serde_json::json!({
        "address": s.address,
        "fee": s.fee,
        "info": s.info,
        "logo_text": s.logo_text,
        "has_logo": has_logo,
    }))
}

async fn set_settings(body: web::Bytes) -> impl Responder {
    let incoming: BitcoinSettings = match serde_json::from_slice(&body) {
        Ok(s) => s,
        Err(e) => return HttpResponse::BadRequest().body(format!("Invalid JSON: {}", e)),
    };
    if incoming.fee == 0 {
        return HttpResponse::BadRequest().body("Fee cannot be 0");
    }
    // Merge into the persisted settings, preserving logo_format (set by upload).
    let mut s = load_settings();
    if !incoming.address.is_empty() {
        if !is_valid_bitcoin_address(&incoming.address) {
            return HttpResponse::BadRequest()
                .body("Not a valid Bitcoin (mainnet) address — refusing to save it.");
        }
        s.address = incoming.address;
    }
    s.fee = incoming.fee;
    if !incoming.info.is_empty() {
        s.info = incoming.info;
    }
    if incoming.logo_text.len() <= 30 {
        s.logo_text = incoming.logo_text;
    }
    match save_settings(&s) {
        Ok(_) => HttpResponse::Ok().body("ok"),
        Err(e) => HttpResponse::InternalServerError().body(format!("error: {}", e)),
    }
}

// POST /restart-server — explicit, admin-triggered restart so a settings
// change (address/fee/info) takes effect. Upstream reads its config once at
// startup from env vars, and entrypoint-server.sh only re-derives those env
// vars from settings.json on container start — so a running server keeps the
// old values until it restarts.
//
// SAFE BY DESIGN: this does NOT touch the Docker socket or spawn any process
// (that would need host-level Docker access — root-equivalent, and something
// this container must never have). It only exits the current process; Docker
// Compose's `restart: unless-stopped` policy on bal-server brings it back up
// within a couple of seconds, at which point the entrypoint re-reads the
// fresh settings.json. No new privileges, no new attack surface.
//
// The exit is delayed briefly so this handler's "restarting" response reaches
// the client before the process dies (an immediate exit would race the
// in-flight HTTP response and the client would see a connection reset).
async fn restart_server() -> impl Responder {
    log::warn!("umbrel: restart-server requested — exiting for Docker to restart this container");
    actix_web::rt::spawn(async {
        actix_web::rt::time::sleep(std::time::Duration::from_millis(400)).await;
        std::process::exit(0);
    });
    HttpResponse::Ok().json(serde_json::json!({"status": "restarting"}))
}

async fn upload_logo(body: web::Bytes) -> impl Responder {
    if body.len() > 204800 {
        return HttpResponse::BadRequest().body("Logo too large. Maximum 200KB.");
    }
    let fmt = match detect_image_format(&body) {
        Some(f) => f,
        None => {
            return HttpResponse::BadRequest()
                .body("Unsupported image format. Use PNG, JPEG, WebP, or SVG.");
        }
    };
    if let Err(e) = fs::write(logo_path(), &body) {
        return HttpResponse::InternalServerError().body(format!("Failed to save logo: {}", e));
    }
    let mut s = load_settings();
    s.logo_format = fmt.to_string();
    let _ = save_settings(&s);
    HttpResponse::Ok().body("ok")
}

async fn remove_logo() -> impl Responder {
    let p = logo_path();
    if Path::new(&p).exists() {
        let _ = fs::remove_file(&p);
    }
    let mut s = load_settings();
    s.logo_format.clear();
    let _ = save_settings(&s);
    HttpResponse::Ok().body("ok")
}

async fn custom_logo() -> impl Responder {
    match fs::read(logo_path()) {
        Ok(data) => {
            let mime = detect_image_format(&data)
                .map(mime_for_format)
                .unwrap_or("application/octet-stream");
            // SVG is a document, not just a picture: an uploaded one can carry
            // <script>, and browsing straight to this URL would execute it on
            // our own origin (stored XSS). `sandbox` with an empty default-src
            // kills scripting, plugins and navigation while still rendering the
            // image; `nosniff` stops a PNG/JPEG being re-interpreted as markup.
            // Applied to every format — it costs nothing for raster images.
            HttpResponse::Ok()
                .insert_header(("Content-Type", mime))
                .insert_header(("Cache-Control", "public, max-age=86400"))
                .insert_header(("X-Content-Type-Options", "nosniff"))
                .insert_header((
                    "Content-Security-Policy",
                    "default-src 'none'; style-src 'unsafe-inline'; sandbox",
                ))
                .body(data)
        }
        Err(_) => HttpResponse::NotFound().body("Not found"),
    }
}

// GET /blockheight — current chain tip published by the pusher (heartbeat).
async fn blockheight() -> impl Responder {
    match fs::read_to_string(data_dir().join("block-status.json")) {
        Ok(s) => HttpResponse::Ok()
            .content_type("application/json")
            .body(s),
        Err(_) => HttpResponse::Ok().json(serde_json::json!({ "height": null })),
    }
}

// ---------------------------------------------------------------------------
// Route registration — the single hook upstream `main()` calls.
// ---------------------------------------------------------------------------

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg
        // read endpoints
        .service(web::resource("/txstats/{network}").route(web::get().to(stats)))
        .service(web::resource("/txlist").route(web::get().to(txlist)))
        .service(web::resource("/txdetail").route(web::get().to(txdetail)))
        // backup / restore / merge
        .service(web::resource("/backup").route(web::get().to(backup)))
        .service(web::resource("/backups/list").route(web::get().to(backups_list)))
        .service(
            web::resource("/backup/config")
                .route(web::get().to(backup_config))
                .route(web::post().to(set_backup_config)),
        )
        .service(web::resource("/merge").route(web::post().to(merge)))
        .service(web::resource("/restore/{file}").route(web::post().to(restore)))
        // settings + branding
        .service(
            web::resource("/settings")
                .route(web::get().to(get_settings))
                .route(web::post().to(set_settings)),
        )
        .service(web::resource("/restart-server").route(web::post().to(restart_server)))
        .service(web::resource("/upload-logo").route(web::post().to(upload_logo)))
        .service(web::resource("/remove-logo").route(web::post().to(remove_logo)))
        .service(web::resource("/custom-logo").route(web::get().to(custom_logo)))
        .service(web::resource("/blockheight").route(web::get().to(blockheight)));
}
