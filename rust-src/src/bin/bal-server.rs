use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Full, combinators::BoxBody};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

use std::env;
use std::net::IpAddr;

use std::fs;
use std::path::Path;
use std::sync::{Arc, RwLock};
use sqlite::{Connection, State, Value};
use std::collections::HashMap;

use bitcoin::{Network, Transaction, consensus};

use chrono::Utc;
use hex_conservative::FromHex;
use log::{debug, error, info, trace, warn};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json;

use bal_server::db::{
    create_database, execute_insert, get_last_used_address_by_ip, get_next_address_index,
    insert_xpub, open_db, save_new_address,
};
use bal_server::xpub::new_address_from_xpub;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const NETWORKS: [&str; 5] = ["bitcoin", "testnet", "testnet4", "signet", "regtest"];

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NetConfig {
    address: String,
    fixed_fee: u64,
    xpub: bool,
    network: Network,
    name: String,
    enabled: bool,
}

impl NetConfig {
    fn default_network(name: String, network: Network) -> Self {
        NetConfig {
            address: "".to_string(),
            fixed_fee: 50000,
            xpub: false,
            name,
            network,
            enabled: false,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct MyConfig {
    regtest: NetConfig,
    signet: NetConfig,
    testnet: NetConfig,
    testnet4: NetConfig,
    mainnet: NetConfig,
    info: String,
    bind_address: String,
    bind_port: u16,
    db_file: String,
    pub_key_path: String,
    expose_stats: bool,
    logo_text: String,
    logo_format: String,
    public_url: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InfoResponse {
    pub address: String,
    pub base_fee: u64,
    pub chain: String,
    pub info: String,
    pub version: String,
    pub logo_text: String,
    pub logo_url: String,
}

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

/// Paginated response for /txlist: `total` is the count of rows matching the
/// filters (ignoring limit/offset), so the UI can render pagination controls;
/// `txs` is the current page.
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

#[derive(Debug, Serialize, Deserialize, Clone)]
struct BitcoinSettings {
    address: String,
    fee: u64,
    info: String,
    #[serde(default)]
    logo_text: String,
    #[serde(default)]
    logo_format: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct BackupConfig {
    backup_path: String,
}

impl Default for BackupConfig {
    fn default() -> Self {
        BackupConfig {
            backup_path: String::new(),
        }
    }
}

impl Default for MyConfig {
    fn default() -> Self {
        MyConfig {
            regtest: NetConfig::default_network("regtest".to_string(), Network::Regtest),
            signet: NetConfig::default_network("signet".to_string(), Network::Signet),
            testnet: NetConfig::default_network("testnet".to_string(), Network::Testnet),
            testnet4: NetConfig::default_network("testnet4".to_string(), Network::Testnet4),
            mainnet: NetConfig::default_network("bitcoin".to_string(), Network::Bitcoin),
            bind_address: "127.0.0.1".to_string(),
            bind_port: 9137,
            db_file: "bal.db".to_string(),
            info: "Will Executor Server".to_string(),
            pub_key_path: "public_key.pem".to_string(),
            expose_stats: env::var("BAL_SERVER_EXPOSE_STATS")
                .unwrap_or("false".to_string())
                .parse::<bool>()
                .unwrap(),
            logo_text: String::new(),
            logo_format: String::new(),
            public_url: String::new(),
        }
    }
}

impl MyConfig {
    fn get_net_config(&self, param: &str) -> &NetConfig {
        match param {
            "regtest" => &self.regtest,
            "testnet" => &self.testnet,
            "testnet4" => &self.testnet4,
            "signet" => &self.signet,
            _ => &self.mainnet,
        }
    }
}

fn settings_file_path(db_file: &str) -> String {
    let path = std::path::Path::new(db_file);
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    dir.join("settings.json").to_string_lossy().to_string()
}

fn logo_file_path(db_file: &str) -> String {
    let path = std::path::Path::new(db_file);
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    dir.join("custom_logo").to_string_lossy().to_string()
}

fn load_settings_file(cfg: &Arc<RwLock<MyConfig>>) {
    let settings_path;
    {
        let cfg_read = cfg.read().unwrap();
        settings_path = settings_file_path(&cfg_read.db_file);
    }
    match fs::read_to_string(&settings_path) {
        Ok(content) => {
            match serde_json::from_str::<BitcoinSettings>(&content) {
                Ok(s) => {
                    let mut cfg_write = cfg.write().unwrap();
                    if !s.address.is_empty() && s.address.len() > 5 {
                        let is_xpub = s.address[1..4] == *"pub";
                        cfg_write.mainnet.address = s.address;
                        cfg_write.mainnet.enabled = true;
                        if is_xpub { cfg_write.mainnet.xpub = true; }
                    }
                    if s.fee > 0 {
                        cfg_write.mainnet.fixed_fee = s.fee;
                    }
                    if !s.info.is_empty() {
                        cfg_write.info = s.info;
                    }
                    if !s.logo_text.is_empty() {
                        cfg_write.logo_text = s.logo_text;
                    }
                    if !s.logo_format.is_empty() {
                        cfg_write.logo_format = s.logo_format;
                    }
                    info!("Loaded settings from {}", settings_path);
                }
                Err(e) => warn!("Failed to parse settings file: {}", e),
            }
        }
        Err(_) => debug!("No settings file at {} (first run)", settings_path),
    }
}

fn save_settings_file(cfg: &MyConfig) -> Result<(), Box<dyn std::error::Error>> {
    let settings_path = settings_file_path(&cfg.db_file);
    let s = BitcoinSettings {
        address: cfg.mainnet.address.clone(),
        fee: cfg.mainnet.fixed_fee,
        info: cfg.info.clone(),
        logo_text: cfg.logo_text.clone(),
        logo_format: cfg.logo_format.clone(),
    };
    let json = serde_json::to_string_pretty(&s)?;
    fs::write(&settings_path, json)?;
    info!("Saved settings to {}", settings_path);
    Ok(())
}

async fn echo_version() -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    Ok(Response::new(full(VERSION)))
}

async fn echo_home(cfg: &MyConfig) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    debug!("echo_home: {}", cfg.info);
    Ok(Response::new(full(cfg.info.clone())))
}

async fn echo_backup(
    cfg: &MyConfig,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    match fs::read(&cfg.db_file) {
        Ok(data) => {
            let filename = format!("bal-backup-{}.db", chrono::Utc::now().format("%Y%m%d"));
            Ok(Response::builder()
                .header("Content-Type", "application/octet-stream")
                .header("Content-Disposition", format!("attachment; filename=\"{}\"", filename))
                .body(full(data))
                .unwrap())
        }
        Err(e) => {
            let mut resp = Response::new(full(format!("error: {}", e)));
            *resp.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
            Ok(resp)
        }
    }
}

fn backup_config_path(db_file: &str) -> String {
    let db_path = Path::new(db_file);
    db_path.parent().unwrap_or(Path::new(".")).join("backup_config.json").to_string_lossy().to_string()
}

fn load_backup_config(db_file: &str) -> BackupConfig {
    let cfg_path = backup_config_path(db_file);
    match fs::read_to_string(&cfg_path) {
        Ok(content) => serde_json::from_str::<BackupConfig>(&content).unwrap_or_default(),
        Err(_) => BackupConfig::default(),
    }
}

fn save_backup_config(db_file: &str, config: &BackupConfig) -> Result<(), Box<dyn std::error::Error>> {
    let cfg_path = backup_config_path(db_file);
    let json = serde_json::to_string_pretty(config)?;
    fs::write(&cfg_path, json)?;
    Ok(())
}

fn backup_dir_path(db_file: &str) -> String {
    let config = load_backup_config(db_file);
    if !config.backup_path.is_empty() {
        return config.backup_path;
    }
    let db_path = Path::new(db_file);
    db_path.parent().unwrap_or(Path::new(".")).join("backup").to_string_lossy().to_string()
}

async fn echo_backups_list(
    cfg: &MyConfig,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    let bak_dir = backup_dir_path(&cfg.db_file);
    let mut backups: Vec<serde_json::Value> = vec![];
    if let Ok(entries) = fs::read_dir(&bak_dir) {
        for entry in entries.flatten() {
            if let Ok(metadata) = entry.metadata() {
                if metadata.is_file() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.ends_with(".db") {
                        let size = metadata.len();
                        let modified = metadata.modified()
                            .map(|t| {
                                let secs = t.duration_since(std::time::UNIX_EPOCH)
                                    .map(|d| d.as_secs() as i64)
                                    .unwrap_or(0);
                                secs
                            })
                            .unwrap_or(0);
                        backups.push(serde_json::json!({
                            "filename": name,
                            "size": size,
                            "modified": modified
                        }));
                    }
                }
            }
        }
    }
    // Sort by modified time (newest first)
    backups.sort_by(|a, b| b["modified"].as_i64().unwrap_or(0).cmp(&a["modified"].as_i64().unwrap_or(0)));
    Ok(Response::new(full(serde_json::to_string(&backups).unwrap())))
}

async fn echo_pub_key(
    cfg: &MyConfig,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    let pub_key = fs::read_to_string(&cfg.pub_key_path)
        .expect(format!("Failed to read public key file {}", cfg.pub_key_path).as_str());
    Ok(Response::new(full(pub_key)))
}

async fn echo_stats(
    param: &str,
    cfg: &MyConfig,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    info!("echo stats!!! {} - {}", param, cfg.expose_stats);
    let netconfig = MyConfig::get_net_config(cfg, param);
    if !netconfig.enabled {
        debug!("network disabled {}", param);
        return Ok(Response::new(full("network disabled")));
    }
    let sql = format!(
        "SELECT
  s.report_date,
  s.chain,
  s.totals,
  s.waiting,
  s.sent,
  s.failed,
  s.waiting_profit,
  s.sent_profit,
  s.missed_profit,
  s.unique_inputs,
  IFNULL((SELECT SUM(CAST(our_fees AS INTEGER)) FROM tbl_tx WHERE status=1 AND confirmations>0 AND network=s.chain), 0) as confirmed_profit,
  IFNULL((SELECT SUM(CAST(our_fees AS INTEGER)) FROM tbl_tx WHERE status=1 AND confirmations=0 AND network=s.chain), 0) as mempool_profit
FROM tbl_stats s WHERE s.chain = '{}'
  ",
        netconfig.name
    );
    let mut stats: Vec<StatsResponse> = vec![];
    let db = open_db(&cfg.db_file).unwrap();
    let _ = db.iterate(&sql, |pairs| {
        let row: HashMap<_, _> = pairs
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.map(|s| s)))
            .collect();
        println!("row report date {}", row["report_date"].clone().unwrap());
        dbg!(&row);
        stats.push(StatsResponse {
            report_date: row["report_date"].clone().unwrap().to_string(),
            chain: row["chain"].clone().unwrap().to_string(),
            totals: row["totals"].clone().unwrap().parse::<i64>().unwrap(),
            waiting: row["waiting"].clone().unwrap().parse::<i64>().unwrap(),
            sent: row["sent"].clone().unwrap().parse::<i64>().unwrap(),
            failed: row["failed"].clone().unwrap().parse::<i64>().unwrap(),
            waiting_profit: row["waiting_profit"].clone().unwrap().parse::<i64>().unwrap(),
            sent_profit: row["sent_profit"].clone().unwrap().parse::<i64>().unwrap(),
            missed_profit: row["missed_profit"].clone().unwrap().parse::<i64>().unwrap(),
            unique_inputs: row["unique_inputs"].clone().unwrap().parse::<i64>().unwrap(),
            confirmed_profit: row["confirmed_profit"].clone().unwrap().parse::<i64>().unwrap(),
            mempool_profit: row["mempool_profit"].clone().unwrap().parse::<i64>().unwrap(),
        });
        true
    });
    match serde_json::to_string(&stats) {
        Ok(json_data) => {
            debug!("echo info reply: {}", json_data);
            return Ok(Response::new(full(json_data)));
        }
        Err(err) => Ok(Response::new(full(format!("error:{}", err)))),
    }
}

/// Read a single query-string parameter from `uri` (everything after '?').
/// Returns the raw (still URL-encoded) value; callers sanitize as needed.
fn query_param(uri: &str, key: &str) -> Option<String> {
    let q = uri.split('?').nth(1)?;
    q.split('&').find_map(|pair| {
        let mut it = pair.splitn(2, '=');
        match (it.next(), it.next()) {
            (Some(k), v) if k == key => Some(v.unwrap_or("").to_string()),
            _ => None,
        }
    })
}

async fn echo_txlist(
    uri: &str,
    cfg: &MyConfig,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    // --- Parse & sanitize inputs (all values are BOUND, never interpolated) ---
    let network = query_param(uri, "network").filter(|s| !s.is_empty());
    let show_failed = query_param(uri, "show_failed").as_deref() == Some("1");
    // txid search: keep only hex chars (neutralizes LIKE metacharacters and any
    // injection attempt), cap at a full txid length. Prefix match, index-friendly.
    let search: String = query_param(uri, "search")
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(64)
        .collect();
    let limit: i64 = query_param(uri, "limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(25)
        .clamp(1, 200);
    let offset: i64 = query_param(uri, "offset")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
        .max(0);
    // ORDER BY column is chosen from a fixed whitelist — never user text.
    let order_expr = match query_param(uri, "sort").as_deref() {
        Some("txid") => "txid",
        Some("network") => "network",
        Some("our_fees") => "CAST(our_fees AS INTEGER)",
        Some("locktime") => "locktime",
        // Lifecycle order of the unified UI "State" column:
        // waiting(0) < mempool(1) < confirmed(2) < done-elsewhere(3) < rejected(4)
        Some("state") => "CASE \
            WHEN status=0 THEN 0 \
            WHEN status=1 AND confirmations>0 THEN 2 \
            WHEN status=1 THEN 1 \
            WHEN status=2 AND confirmations=-1 THEN 3 \
            ELSE 4 END",
        _ => "date_creation",
    };
    let dir = if query_param(uri, "dir").as_deref() == Some("asc") { "ASC" } else { "DESC" };

    // --- Build a parameterized WHERE shared by the COUNT and the SELECT ---
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

    let db = match open_db(&cfg.db_file) {
        Ok(d) => d,
        Err(e) => {
            let mut resp = Response::new(full(format!("db error: {}", e)));
            *resp.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
            return Ok(resp);
        }
    };

    // --- Total count matching the filters (for pagination) ---
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

    // --- Page of rows ---
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

    let payload = TxListResponse { total, txs };
    match serde_json::to_string(&payload) {
        Ok(json_data) => Ok(Response::new(full(json_data))),
        Err(err) => Ok(Response::new(full(format!("error:{}", err)))),
    }
}

async fn echo_txdetail(
    uri: &str,
    cfg: &MyConfig,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    let txid = uri.split("?txid=").nth(1)
        .map(|v| v.split('&').next().unwrap_or(""))
        .filter(|s| !s.is_empty())
        .unwrap_or("")
        .to_string();

    if txid.is_empty() {
        let mut resp = Response::new(full("missing txid"));
        *resp.status_mut() = StatusCode::BAD_REQUEST;
        return Ok(resp);
    }

    let db = open_db(&cfg.db_file).unwrap();
    let mut stmt = db.prepare("SELECT * FROM tbl_tx WHERE txid = ? LIMIT 1").unwrap();
    stmt.bind((1, txid.as_str())).unwrap();

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

        let netconfig = MyConfig::get_net_config(cfg, &data.network);
        let bitcoin_network = netconfig.network;

        let mut inp_stmt = db.prepare("SELECT in_txid, in_vout FROM tbl_inp WHERE txid = ?").unwrap();
        inp_stmt.bind((1, txid.as_str())).unwrap();
        while let Ok(State::Row) = inp_stmt.next() {
            data.inputs.push(InputEntry {
                in_txid: inp_stmt.read::<String, _>("in_txid").unwrap_or_default(),
                in_vout: inp_stmt.read::<String, _>("in_vout").unwrap_or_default(),
            });
        }

        // Parse raw tx to decode addresses for old entries and calculate total output
        let tx_output_data = if !data.tx.is_empty() {
            Vec::<u8>::from_hex(&data.tx).ok()
                .and_then(|b| bitcoin::consensus::encode::deserialize::<Transaction>(&b).ok())
                .map(|t| t.output.into_iter().map(|o| (o.script_pubkey, o.value.to_sat())).collect::<Vec<_>>())
                .unwrap_or_default()
        } else {
            vec![]
        };

        let mut out_stmt = db.prepare("SELECT script_pubkey, amount, vout, address FROM tbl_out WHERE txid = ? ORDER BY vout").unwrap();
        out_stmt.bind((1, txid.as_str())).unwrap();
        let mut total_out: u64 = 0;
        while let Ok(State::Row) = out_stmt.next() {
            let script_pubkey = out_stmt.read::<String, _>("script_pubkey").unwrap_or_default();
            let amount = out_stmt.read::<String, _>("amount").unwrap_or_default();
            let vout = out_stmt.read::<String, _>("vout").unwrap_or_default();
            let mut address = out_stmt.read::<String, _>("address").unwrap_or_default();
            if address.is_empty() {
                let idx: usize = vout.parse().unwrap_or(0);
                address = tx_output_data.get(idx)
                    .map(|(s, _)| s)
                    .and_then(|s| bitcoin::Address::from_script(s.as_script(), bitcoin_network).ok())
                    .map(|a| a.to_string())
                    .unwrap_or_default();
            }
            if let Ok(sat) = amount.parse::<u64>() { total_out += sat; }
            data.outputs.push(OutputEntry { script_pubkey, address, amount, vout });
        }
        data.total_output = total_out.to_string();
    }

    match serde_json::to_string(&data) {
        Ok(json_data) => Ok(Response::new(full(json_data))),
        Err(err) => Ok(Response::new(full(format!("error:{}", err)))),
    }
}

async fn echo_merge(
    whole_body: &Bytes,
    cfg: &MyConfig,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    let tmp_path = format!("/tmp/bal-merge-{}.db", std::process::id());

    // Write uploaded data to temporary file
    if let Err(e) = fs::write(&tmp_path, whole_body) {
        let mut resp = Response::new(full(format!("{{\"error\":\"cannot write temp file: {}\"}}", e)));
        *resp.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
        return Ok(resp);
    }

    // Validate it is a valid SQLite database
    let bak_db = sqlite::open(&tmp_path);
    if bak_db.is_err() {
        let _ = fs::remove_file(&tmp_path);
        let mut resp = Response::new(full("{\"error\":\"uploaded file is not a valid SQLite database\"}"));
        *resp.status_mut() = StatusCode::BAD_REQUEST;
        return Ok(resp);
    }
    let bak_db = bak_db.unwrap();

    // Verify the backup has the expected tables
    let tables_ok = bak_db.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name IN ('tbl_tx','tbl_inp','tbl_out')").is_ok();
    if !tables_ok {
        let _ = fs::remove_file(&tmp_path);
        let mut resp = Response::new(full("{\"error\":\"backup file does not contain expected tables (tbl_tx, tbl_inp, tbl_out)\"}"));
        *resp.status_mut() = StatusCode::BAD_REQUEST;
        return Ok(resp);
    }

    // Count rows in backup before merge
    let bak_count: i64 = {
        let mut stmt = bak_db.prepare("SELECT COUNT(*) FROM tbl_tx").unwrap();
        if let Ok(sqlite::State::Row) = stmt.next() {
            stmt.read::<i64, _>(0).unwrap_or(0)
        } else { 0 }
    };

    // Open main database and attach backup
    let main_db = match open_db(&cfg.db_file) {
        Ok(db) => db,
        Err(e) => {
            let _ = fs::remove_file(&tmp_path);
            let mut resp = Response::new(full(format!("{{\"error\":\"cannot open main database: {}\"}}", e)));
            *resp.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
            return Ok(resp);
        }
    };

    // Count rows in main before merge
    let main_before: i64 = {
        let mut stmt = main_db.prepare("SELECT COUNT(*) FROM tbl_tx").unwrap();
        if let Ok(sqlite::State::Row) = stmt.next() {
            stmt.read::<i64, _>(0).unwrap_or(0)
        } else { 0 }
    };

    // Perform merge inside a transaction
    let _ = main_db.execute("BEGIN IMMEDIATE");

    let attach_sql = format!("ATTACH DATABASE '{}' AS bak", tmp_path);
    if let Err(e) = main_db.execute(&attach_sql) {
        let _ = main_db.execute("ROLLBACK");
        let _ = fs::remove_file(&tmp_path);
        let mut resp = Response::new(full(format!("{{\"error\":\"attach failed: {}\"}}", e)));
        *resp.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
        return Ok(resp);
    }

    let _ = main_db.execute("INSERT OR IGNORE INTO tbl_tx SELECT * FROM bak.tbl_tx");
    let _ = main_db.execute("INSERT OR IGNORE INTO tbl_inp SELECT * FROM bak.tbl_inp");
    let _ = main_db.execute("INSERT OR IGNORE INTO tbl_out SELECT * FROM bak.tbl_out");
    let _ = main_db.execute("INSERT OR IGNORE INTO tbl_xpub(network,xpub) SELECT network,xpub FROM bak.tbl_xpub");
    let _ = main_db.execute("INSERT OR IGNORE INTO tbl_address SELECT * FROM bak.tbl_address");

    // Recalculate statistics
    let _ = main_db.execute("DELETE FROM tbl_stats");
    let stats_sql = "INSERT INTO tbl_stats(report_date, chain, totals, waiting, sent, failed, waiting_profit, sent_profit, missed_profit, unique_inputs) SELECT CURRENT_TIMESTAMP, network, COUNT(*), SUM(CASE WHEN status=0 AND (locktime < 500000000 OR locktime > CAST(strftime('%s','now') AS INTEGER)) THEN 1 ELSE 0 END), SUM(CASE WHEN status=1 THEN 1 ELSE 0 END), SUM(CASE WHEN status=2 THEN 1 ELSE 0 END), COALESCE(SUM(CASE WHEN status=0 AND (locktime < 500000000 OR locktime > CAST(strftime('%s','now') AS INTEGER)) THEN CAST(our_fees AS INTEGER) ELSE 0 END), 0), COALESCE(SUM(CASE WHEN status=1 THEN CAST(our_fees AS INTEGER) ELSE 0 END), 0), COALESCE(SUM(CASE WHEN status=2 THEN CAST(our_fees AS INTEGER) ELSE 0 END), 0), 0 FROM tbl_tx GROUP BY network";
    if let Err(e) = main_db.execute(stats_sql) {
        warn!("stats recalculation after merge failed (non-fatal): {}", e);
    }

    let _ = main_db.execute("DETACH DATABASE bak");

    let _ = main_db.execute("COMMIT");

    // Count rows after merge
    let main_after: i64 = {
        let mut stmt = main_db.prepare("SELECT COUNT(*) FROM tbl_tx").unwrap();
        if let Ok(sqlite::State::Row) = stmt.next() {
            stmt.read::<i64, _>(0).unwrap_or(0)
        } else { 0 }
    };

    // Cleanup temp file
    let _ = fs::remove_file(&tmp_path);

    let merged = main_after - main_before;
    let skipped = bak_count - merged;

    info!("merge: imported {} new transactions, {} skipped, total {} tx in database", merged, skipped, main_after);

    let result = serde_json::json!({
        "merged": merged,
        "skipped": skipped,
        "total": main_after
    });

    Ok(Response::new(full(serde_json::to_string(&result).unwrap())))
}

async fn echo_restore(
    filename: &str,
    cfg: &MyConfig,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    let bak_dir = backup_dir_path(&cfg.db_file);
    let backup_path = Path::new(&bak_dir).join(filename);

    // Security: prevent path traversal
    if filename.contains("..") || filename.contains("/") || filename.contains("\\") {
        let mut resp = Response::new(full("{\"error\":\"invalid filename\"}"));
        *resp.status_mut() = StatusCode::BAD_REQUEST;
        return Ok(resp);
    }

    // Check the file exists (sqlite::open creates a new file if it doesn't exist)
    if !backup_path.exists() {
        let mut resp = Response::new(full(format!("{{\"error\":\"backup file '{}' not found\"}}", filename)));
        *resp.status_mut() = StatusCode::BAD_REQUEST;
        return Ok(resp);
    }

    // Open the backup database
    let bak_db = match sqlite::open(&*backup_path.to_string_lossy()) {
        Ok(db) => db,
        Err(_) => {
            let mut resp = Response::new(full(format!("{{\"error\":\"backup file '{}' is not a valid database\"}}", filename)));
            *resp.status_mut() = StatusCode::BAD_REQUEST;
            return Ok(resp);
        }
    };

    // Verify the backup has the expected tables
    let tables_ok = bak_db.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name IN ('tbl_tx','tbl_inp','tbl_out')").is_ok();
    if !tables_ok {
        let mut resp = Response::new(full("{\"error\":\"backup file does not contain expected tables (tbl_tx, tbl_inp, tbl_out)\"}"));
        *resp.status_mut() = StatusCode::BAD_REQUEST;
        return Ok(resp);
    }

    // Count rows in backup before merge
    let bak_count: i64 = {
        let mut stmt = bak_db.prepare("SELECT COUNT(*) FROM tbl_tx").unwrap();
        if let Ok(sqlite::State::Row) = stmt.next() {
            stmt.read::<i64, _>(0).unwrap_or(0)
        } else { 0 }
    };

    // Open main database
    let main_db = match open_db(&cfg.db_file) {
        Ok(db) => db,
        Err(e) => {
            let mut resp = Response::new(full(format!("{{\"error\":\"cannot open main database: {}\"}}", e)));
            *resp.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
            return Ok(resp);
        }
    };

    // Count rows in main before merge
    let main_before: i64 = {
        let mut stmt = main_db.prepare("SELECT COUNT(*) FROM tbl_tx").unwrap();
        if let Ok(sqlite::State::Row) = stmt.next() {
            stmt.read::<i64, _>(0).unwrap_or(0)
        } else { 0 }
    };

    // Perform merge inside a transaction
    let _ = main_db.execute("BEGIN IMMEDIATE");

    let bak_path_str = backup_path.to_string_lossy().to_string().replace("'", "''");
    let attach_sql = format!("ATTACH DATABASE '{}' AS bak", bak_path_str);
    if let Err(e) = main_db.execute(&attach_sql) {
        let _ = main_db.execute("ROLLBACK");
        let mut resp = Response::new(full(format!("{{\"error\":\"attach failed: {}\"}}", e)));
        *resp.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
        return Ok(resp);
    }

    let _ = main_db.execute("INSERT OR IGNORE INTO tbl_tx SELECT * FROM bak.tbl_tx");
    let _ = main_db.execute("INSERT OR IGNORE INTO tbl_inp SELECT * FROM bak.tbl_inp");
    let _ = main_db.execute("INSERT OR IGNORE INTO tbl_out SELECT * FROM bak.tbl_out");
    let _ = main_db.execute("INSERT OR IGNORE INTO tbl_xpub(network,xpub) SELECT network,xpub FROM bak.tbl_xpub");
    let _ = main_db.execute("INSERT OR IGNORE INTO tbl_address SELECT * FROM bak.tbl_address");

    // Recalculate statistics
    let _ = main_db.execute("DELETE FROM tbl_stats");
    let stats_sql = "INSERT INTO tbl_stats(report_date, chain, totals, waiting, sent, failed, waiting_profit, sent_profit, missed_profit, unique_inputs) SELECT CURRENT_TIMESTAMP, network, COUNT(*), SUM(CASE WHEN status=0 AND (locktime < 500000000 OR locktime > CAST(strftime('%s','now') AS INTEGER)) THEN 1 ELSE 0 END), SUM(CASE WHEN status=1 THEN 1 ELSE 0 END), SUM(CASE WHEN status=2 THEN 1 ELSE 0 END), COALESCE(SUM(CASE WHEN status=0 AND (locktime < 500000000 OR locktime > CAST(strftime('%s','now') AS INTEGER)) THEN CAST(our_fees AS INTEGER) ELSE 0 END), 0), COALESCE(SUM(CASE WHEN status=1 THEN CAST(our_fees AS INTEGER) ELSE 0 END), 0), COALESCE(SUM(CASE WHEN status=2 THEN CAST(our_fees AS INTEGER) ELSE 0 END), 0), 0 FROM tbl_tx GROUP BY network";
    if let Err(e) = main_db.execute(stats_sql) {
        warn!("stats recalculation after restore failed (non-fatal): {}", e);
    }

    let _ = main_db.execute("DETACH DATABASE bak");

    let _ = main_db.execute("COMMIT");

    // Count rows after merge
    let main_after: i64 = {
        let mut stmt = main_db.prepare("SELECT COUNT(*) FROM tbl_tx").unwrap();
        if let Ok(sqlite::State::Row) = stmt.next() {
            stmt.read::<i64, _>(0).unwrap_or(0)
        } else { 0 }
    };

    let merged = main_after - main_before;
    let skipped = bak_count - merged;

    info!("restore from {}: imported {} new transactions, {} skipped, total {} tx in database", filename, merged, skipped, main_after);

    let result = serde_json::json!({
        "merged": merged,
        "skipped": skipped,
        "total": main_after,
        "filename": filename
    });

    Ok(Response::new(full(serde_json::to_string(&result).unwrap())))
}

async fn echo_backup_config(
    cfg: &MyConfig,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    let config = load_backup_config(&cfg.db_file);
    Ok(Response::new(full(serde_json::to_string(&config).unwrap())))
}

async fn echo_set_backup_config(
    whole_body: Bytes,
    cfg: &Arc<RwLock<MyConfig>>,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    let strbody = match std::str::from_utf8(&whole_body) {
        Ok(s) => s,
        Err(_) => {
            let mut resp = Response::new(full("Invalid UTF-8"));
            *resp.status_mut() = StatusCode::BAD_REQUEST;
            return Ok(resp);
        }
    };
    let config: BackupConfig = match serde_json::from_str(strbody) {
        Ok(c) => c,
        Err(e) => {
            let mut resp = Response::new(full(format!("Invalid JSON: {}", e)));
            *resp.status_mut() = StatusCode::BAD_REQUEST;
            return Ok(resp);
        }
    };
    let db_file = { cfg.read().unwrap().db_file.clone() };
    if let Err(e) = save_backup_config(&db_file, &config) {
        let mut resp = Response::new(full(format!("error: {}", e)));
        *resp.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
        return Ok(resp);
    }
    info!("Backup config saved: backup_path={}", config.backup_path);
    Ok(Response::new(full("ok")))
}

async fn echo_info(
    param: &str,
    cfg: &MyConfig,
    remote_addr: &String,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    info!("echo info!!!{}", param);
    let netconfig = MyConfig::get_net_config(cfg, param);
    if !netconfig.enabled {
        debug!("network disabled {}", param);
        return Ok(Response::new(full("network disabled")));
    }
    let address = match netconfig.xpub {
        false => {
            let address = netconfig.address.to_string();
            trace!("is address: {}", &address);
            address
        }
        true => {
            let db = open_db(&cfg.db_file).unwrap();
            match get_last_used_address_by_ip(
                &db,
                &netconfig.name,
                &netconfig.address,
                &remote_addr,
            ) {
                Some(address) => address,
                None => {
                    let next = get_next_address_index(&db, &netconfig.name, &netconfig.address);
                    let address =
                        new_address_from_xpub(&netconfig.address, next.1, netconfig.network)
                            .unwrap();
                    save_new_address(&db, next.0, &address.0, &address.1, &remote_addr);
                    debug!("save new address {} {}", address.0, address.1);
                    trace!("next {} {}", next.0, next.1);
                    address.0
                }
            }
        }
    };
    let logo_url = if cfg.logo_format.is_empty() || cfg.public_url.is_empty() {
        String::new()
    } else {
        format!("{}/api/custom-logo", cfg.public_url.trim_end_matches('/'))
    };
    let info = InfoResponse {
        address,
        base_fee: netconfig.fixed_fee,
        chain: netconfig.network.to_string(),
        info: cfg.info.to_string(),
        version: VERSION.to_string(),
        logo_text: cfg.logo_text.clone(),
        logo_url,
    };
    trace!("address: {:#?}", info);
    match serde_json::to_string(&info) {
        Ok(json_data) => {
            debug!("echo info reply: {}", json_data);
            return Ok(Response::new(full(json_data)));
        }
        Err(err) => Ok(Response::new(full(format!("error:{}", err)))),
    }
}

async fn echo_search(
    whole_body: &Bytes,
    cfg: &MyConfig,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    info!("echo search!!!");
    let strbody = std::str::from_utf8(whole_body).unwrap();
    info!("{}", strbody);

    let mut response = Response::new(full("Bad data received".to_owned()));
    *response.status_mut() = StatusCode::BAD_REQUEST;
    if !strbody.is_empty() && strbody.len() <= 70 {
        let db = open_db(&cfg.db_file).unwrap();
        let mut statement = db
            .prepare("SELECT * FROM tbl_tx WHERE txid = ? LIMIT 1")
            .unwrap();
        statement.bind((1, strbody)).unwrap();

        if let Ok(State::Row) = statement.next() {
            let mut response_data = HashMap::new();
            match statement.read::<String, _>("status") {
                Ok(value) => response_data.insert("status", value),
                Err(e) => { error!("Error reading status: {}", e); None }
            };
            match statement.read::<String, _>("tx") {
                Ok(value) => response_data.insert("tx", value),
                Err(e) => { error!("Error reading tx: {}", e); None }
            };
            match statement.read::<String, _>("our_address") {
                Ok(value) => response_data.insert("our_address", value),
                Err(e) => { error!("Error reading address: {}", e); None }
            };
            match statement.read::<String, _>("our_fees") {
                Ok(value) => response_data.insert("our_fees", value),
                Err(e) => { error!("Error reading fees: {}", e); None }
            };
            match statement.read::<String, _>("reqid") {
                Ok(value) => response_data.insert("time", value),
                Err(e) => { error!("Error reading reqid: {}", e); None }
            };
            // confirmations lets the UI resolve the same unified "State" it shows
            // in the transaction list (distinguishes DONE-ELSEWHERE from REJECTED).
            match statement.read::<i64, _>("confirmations") {
                Ok(value) => response_data.insert("confirmations", value.to_string()),
                Err(e) => { error!("Error reading confirmations: {}", e); None }
            };
            response = match serde_json::to_string(&response_data) {
                Ok(json_data) => Response::new(full(json_data)),
                Err(_) => response,
            };
            return Ok(response);
        }
    }
    Ok(response)
}

async fn echo_push(
    whole_body: &Bytes,
    cfg: &MyConfig,
    param: &str,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    trace!("echo_push");
    let strbody = std::str::from_utf8(whole_body).unwrap();
    let mut response = Response::new(full("Bad data received".to_owned()));
    let mut response_not_enable = Response::new(full("Network not enabled".to_owned()));
    *response.status_mut() = StatusCode::BAD_REQUEST;
    *response_not_enable.status_mut() = StatusCode::BAD_REQUEST;
    let netconfig = MyConfig::get_net_config(cfg, param);
    if !netconfig.enabled {
        trace!("network not enabled {}", &netconfig.name);
        return Ok(response_not_enable);
    }
    let req_time = Utc::now().timestamp_nanos_opt().unwrap();

    let db = open_db(&cfg.db_file).unwrap();

    let lines = strbody.split("\n");
    let sqltxshead = "INSERT INTO tbl_tx (txid, wtxid, ntxid, tx, locktime, reqid, network, our_address, our_fees)".to_string();
    let mut sqltxs = "".to_string();
    let sqlinpshead = "INSERT INTO tbl_inp (txid, in_txid, in_vout )".to_string();
    let mut sqlinps = "".to_string();
    let sqloutshead = "INSERT INTO tbl_out (txid, vout, script_pubkey, amount, address )".to_string();
    let mut sqlouts = "".to_string();
    let mut union_tx = true;
    let mut union_inps = true;
    let mut union_outs = true;
    let mut already_present = false;
    let mut ptx: Vec<(usize, Value)> = vec![];
    let mut pinps: Vec<(usize, Value)> = vec![];
    let mut pouts: Vec<(usize, Value)> = vec![];
    let mut linenum = 1;
    let mut lineinp = 1;
    let mut lineout = 1;
    for line in lines {
        if line.is_empty() {
            trace!("line len is: {}", line.len());
            continue;
        }
        let linea = format!("{req_time}:{line}");
        info!("New Tx: {}", linea);
        let raw_tx = match Vec::<u8>::from_hex(line) {
            Ok(raw_tx) => raw_tx,
            Err(err) => { error!("rawtx error: {}", err); continue; }
        };
        if !raw_tx.is_empty() {
            trace!("len: {}", raw_tx.len());
            let tx: Transaction = match consensus::deserialize(&raw_tx) {
                Ok(tx) => tx,
                Err(err) => { error!("error: unable to parse tx: {}\n{}", line, err); continue; }
            };
            let txid = tx.compute_txid().to_string();
            trace!("txid: {}", txid);
            let mut statement = db.prepare("SELECT * FROM tbl_tx WHERE txid = ?").unwrap();
            statement.bind((1, &txid[..])).unwrap();
            if let Ok(State::Row) = statement.next() {
                trace!("already present");
                already_present = true;
                continue;
            }
            let ntxid = tx.compute_ntxid();
            let wtxid = tx.compute_wtxid();
            let mut found = false;
            let locktime = tx.lock_time;
            let mut our_address: String = "".to_string();
            let mut our_fees: u64 = 0;
            for input in tx.input {
                if !union_inps { sqlinps = format!("{sqlinps} UNION ALL"); } else { union_inps = false; }
                sqlinps = format!("{sqlinps} SELECT ?, ?, ?");
                pinps.push((lineinp, Value::String(txid.to_string())));
                pinps.push((lineinp + 1, Value::String(input.previous_output.txid.to_string())));
                pinps.push((lineinp + 2, Value::String(input.previous_output.vout.to_string())));
                lineinp += 3;
            }
            if netconfig.fixed_fee == 0 { found = true; }
            for (idx, output) in tx.output.into_iter().enumerate() {
                let script_pubkey = output.script_pubkey;
                let address = match bitcoin::Address::from_script(script_pubkey.as_script(), netconfig.network) {
                    Ok(address) => address.to_string(),
                    Err(_) => String::new(),
                };
                let amount = output.value;
                our_fees = netconfig.fixed_fee;
                if netconfig.xpub {
                    let sql = "select * from tbl_address where address=?";
                    let mut stmt = db.prepare(sql).expect("failed to fetch addresses");
                    stmt.bind((1, Value::String(address.to_string()))).unwrap();
                    if let Ok(State::Row) = stmt.next() { our_address = address.to_string(); }
                } else {
                    our_address = netconfig.address.to_string();
                }
                if address == our_address && amount.to_sat() >= netconfig.fixed_fee {
                    our_fees = amount.to_sat();
                    found = true;
                    trace!("address and fees are correct {}: {}", our_address, our_fees);
                }
                if !union_outs { sqlouts = format!("{sqlouts} UNION ALL"); } else { union_outs = false; }
                sqlouts = format!("{sqlouts} SELECT ?, ?, ?, ?, ?");
                pouts.push((lineout, Value::String(txid.to_string())));
                pouts.push((lineout + 1, Value::Integer(idx.try_into().unwrap())));
                pouts.push((lineout + 2, Value::String(script_pubkey.to_string())));
                pouts.push((lineout + 3, Value::Integer(amount.to_sat().try_into().unwrap())));
                pouts.push((lineout + 4, Value::String(address.to_string())));
                lineout += 5;
            }
            if !found {
                let err_msg = format!(
                    "No output pays >= {} sat to server address ({}). Each tx must include an output paying at least the minimum fee to the server's address.",
                    netconfig.fixed_fee, netconfig.address
                );
                error!("{}", err_msg);
                let mut resp = Response::new(full(err_msg));
                *resp.status_mut() = StatusCode::BAD_REQUEST;
                return Ok(resp);
            } else {
                if !union_tx { sqltxs = format!("{sqltxs} UNION ALL"); } else { union_tx = false; }
                sqltxs = format!("{sqltxs}  SELECT ?, ?, ?, ?, ?, ?, ?, ?, ?");
                ptx.push((linenum, Value::String(txid)));
                ptx.push((linenum + 1, Value::String(wtxid.to_string())));
                ptx.push((linenum + 2, Value::String(ntxid.to_string())));
                ptx.push((linenum + 3, Value::String(line.to_string())));
                ptx.push((linenum + 4, Value::String(locktime.to_string())));
                ptx.push((linenum + 5, Value::String(req_time.to_string())));
                ptx.push((linenum + 6, Value::String(netconfig.name.to_string())));
                ptx.push((linenum + 7, Value::String(our_address.to_string())));
                ptx.push((linenum + 8, Value::String(our_fees.to_string())));
                linenum += 9;
            }
        } else {
            trace!("rawTx len is: {}", raw_tx.len());
            debug!("{}", &sqltxs);
        }
    }
    if sqltxs.is_empty() {
        if already_present {
            return Ok(Response::new(full("already present")));
        }
        return Ok(Response::new(full("no valid transactions")));
    }
    let sqltxs = format!("{}{};", sqltxshead, sqltxs);
    let sqlinps = format!("{}{};", sqlinpshead, sqlinps);
    let sqlouts = format!("{}{};", sqloutshead, sqlouts);
    if let Err(err) = execute_insert(&db, sqltxs, ptx, sqlinps, pinps, sqlouts, pouts) {
        debug!("{}", err);
        return Ok(response);
    }
    Ok(Response::new(full("thx")))
}

fn echo_get_settings(cfg: &MyConfig) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    let has_logo = Path::new(&logo_file_path(&cfg.db_file)).exists();
    #[derive(Serialize)]
    struct SettingsResp {
        address: String,
        fee: u64,
        info: String,
        logo_text: String,
        has_logo: bool,
    }
    let resp = SettingsResp {
        address: cfg.mainnet.address.clone(),
        fee: cfg.mainnet.fixed_fee,
        info: cfg.info.clone(),
        logo_text: cfg.logo_text.clone(),
        has_logo,
    };
    match serde_json::to_string(&resp) {
        Ok(json) => Ok(Response::new(full(json))),
        Err(e) => {
            let mut resp = Response::new(full(format!("error: {}", e)));
            *resp.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
            Ok(resp)
        }
    }
}

async fn echo_set_settings(
    whole_body: Bytes,
    cfg: &Arc<RwLock<MyConfig>>,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    let strbody = match std::str::from_utf8(&whole_body) {
        Ok(s) => s,
        Err(_) => {
            let mut resp = Response::new(full("Invalid UTF-8"));
            *resp.status_mut() = StatusCode::BAD_REQUEST;
            return Ok(resp);
        }
    };
    let settings: BitcoinSettings = match serde_json::from_str(strbody) {
        Ok(s) => s,
        Err(e) => {
            let mut resp = Response::new(full(format!("Invalid JSON: {}", e)));
            *resp.status_mut() = StatusCode::BAD_REQUEST;
            return Ok(resp);
        }
    };
    if settings.fee == 0 {
        let mut resp = Response::new(full("Fee cannot be 0"));
        *resp.status_mut() = StatusCode::BAD_REQUEST;
        return Ok(resp);
    }
    {
        let mut cfg_write = cfg.write().unwrap();
        if !settings.address.is_empty() && settings.address.len() > 5 {
            let old_addr = cfg_write.mainnet.address.clone();
            let is_xpub = settings.address[1..4] == *"pub";
            cfg_write.mainnet.address = settings.address;
            if !cfg_write.mainnet.enabled {
                cfg_write.mainnet.enabled = true;
            }
            cfg_write.mainnet.xpub = is_xpub;
            info!("Settings: address changed from {} to {}", old_addr, cfg_write.mainnet.address);
        }
        cfg_write.mainnet.fixed_fee = settings.fee;
        info!("Settings: fee changed to {}", settings.fee);
        if !settings.info.is_empty() {
            let old_info = cfg_write.info.clone();
            cfg_write.info = settings.info;
            info!("Settings: description changed from '{}' to '{}'", old_info, cfg_write.info);
        }
        if settings.logo_text.len() <= 30 {
            let old_text = cfg_write.logo_text.clone();
            cfg_write.logo_text = settings.logo_text;
            if old_text != cfg_write.logo_text {
                info!("Settings: logo text changed from '{}' to '{}'", old_text, cfg_write.logo_text);
            }
        }
        if let Err(e) = save_settings_file(&cfg_write) {
            error!("Failed to save settings file: {}", e);
        }
    }
    Ok(Response::new(full("ok")))
}

fn detect_image_format(data: &[u8]) -> Option<&'static str> {
    if data.len() < 4 {
        return None;
    }
    // PNG: \x89PNG
    if data[0] == 0x89 && data[1] == b'P' && data[2] == b'N' && data[3] == b'G' {
        return Some("png");
    }
    // JPEG: \xff\xd8
    if data[0] == 0xff && data[1] == 0xd8 {
        return Some("jpeg");
    }
    // WebP: RIFF....WEBP
    if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        return Some("webp");
    }
    // SVG: <?xml or <svg
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

async fn echo_upload_logo(
    whole_body: Bytes,
    cfg: &Arc<RwLock<MyConfig>>,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    if whole_body.len() > 204800 {
        let mut resp = Response::new(full("Logo too large. Maximum 200KB."));
        *resp.status_mut() = StatusCode::BAD_REQUEST;
        return Ok(resp);
    }
    let fmt = match detect_image_format(&whole_body) {
        Some(f) => f,
        None => {
            let mut resp = Response::new(full("Unsupported image format. Use PNG, JPEG, WebP, or SVG."));
            *resp.status_mut() = StatusCode::BAD_REQUEST;
            return Ok(resp);
        }
    };
    let db_file = { cfg.read().unwrap().db_file.clone() };
    let logo_path = logo_file_path(&db_file);
    if let Err(e) = fs::write(&logo_path, &whole_body) {
        error!("Failed to save logo: {}", e);
        let mut resp = Response::new(full(format!("Failed to save logo: {}", e)));
        *resp.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
        return Ok(resp);
    }
    {
        let mut cfg_write = cfg.write().unwrap();
        cfg_write.logo_format = fmt.to_string();
        if let Err(e) = save_settings_file(&cfg_write) {
            error!("Failed to save settings after logo upload: {}", e);
        }
    }
    info!("Logo uploaded: format={}, size={}", fmt, whole_body.len());
    Ok(Response::new(full("ok")))
}

async fn echo_remove_logo(
    cfg: &Arc<RwLock<MyConfig>>,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    let db_file = { cfg.read().unwrap().db_file.clone() };
    let logo_path = logo_file_path(&db_file);
    if Path::new(&logo_path).exists() {
        if let Err(e) = fs::remove_file(&logo_path) {
            error!("Failed to remove logo: {}", e);
        }
    }
    {
        let mut cfg_write = cfg.write().unwrap();
        cfg_write.logo_format.clear();
        if let Err(e) = save_settings_file(&cfg_write) {
            error!("Failed to save settings after logo removal: {}", e);
        }
    }
    info!("Logo removed");
    Ok(Response::new(full("ok")))
}

async fn echo_custom_logo(
    cfg: &MyConfig,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    let logo_path = logo_file_path(&cfg.db_file);
    match fs::read(&logo_path) {
        Ok(data) => {
            let mime = mime_for_format(&cfg.logo_format);
            Ok(Response::builder()
                .header("Content-Type", mime)
                .header("Cache-Control", "public, max-age=86400")
                .body(full(data))
                .unwrap())
        }
        Err(_) => {
            let mut resp = Response::new(full("Not found"));
            *resp.status_mut() = StatusCode::NOT_FOUND;
            Ok(resp)
        }
    }
}

fn match_uri<'a>(path: &str, uri: &'a str) -> Option<&'a str> {
    let re = Regex::new(path).unwrap();
    re.captures(uri).map(|caps| {
        caps.name("param").map(|m| m.as_str()).unwrap_or("")
    })
}

async fn echo(
    req: Request<hyper::body::Incoming>,
    cfg: &Arc<RwLock<MyConfig>>,
    ip: &String,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    let mut not_found = Response::new(empty());
    *not_found.status_mut() = StatusCode::NOT_FOUND;

    let uri = req.uri().path().to_string();
    let full_uri = req.uri().to_string();

    // Handle /settings early (GET needs no body, POST collects body first)
    if uri == "/settings" {
        match *req.method() {
            Method::GET => {
                let cfg_read = cfg.read().unwrap();
                return echo_get_settings(&cfg_read);
            }
            Method::POST => {
                let whole_body = req.collect().await?.to_bytes();
                return echo_set_settings(whole_body, cfg).await;
            }
            _ => return Ok(not_found),
        }
    }

    let remote_addr = req
        .headers()
        .get("X-Real-IP")
        .and_then(|value| value.to_str().ok())
        .and_then(|xff| xff.split(',').next())
        .map(|ip| ip.trim().to_string())
        .unwrap_or_else(|| ip.to_string());
    trace!("{}: {}", remote_addr, uri);

    let mut ret: Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> = Ok(not_found);

    match *req.method() {
        Method::POST => {
            let whole_body = req.collect().await?.to_bytes();
            // Clone config before any .await to avoid holding RwLockReadGuard across await
            let cloned_cfg = { cfg.read().unwrap().clone() };
            if let Some(param) = match_uri(r"^/?((?P<param>[^/]+)/)?pushtxs$", uri.as_str()) {
                ret = echo_push(&whole_body, &cloned_cfg, param).await;
            }
            if uri == "/searchtx" {
                ret = echo_search(&whole_body, &cloned_cfg).await;
            }
            if uri == "/merge" {
                ret = echo_merge(&whole_body, &cloned_cfg).await;
            }
            if uri.starts_with("/restore/") {
                let filename = &uri["/restore/".len()..];
                if filename.is_empty() {
                    let mut resp = Response::new(full("{\"error\":\"missing filename\"}"));
                    *resp.status_mut() = StatusCode::BAD_REQUEST;
                    ret = Ok(resp);
                } else {
                    ret = echo_restore(filename, &cloned_cfg).await;
                }
            }
            if uri == "/backup/config" {
                ret = echo_set_backup_config(whole_body.clone(), cfg).await;
            }
            if uri == "/upload-logo" {
                ret = echo_upload_logo(whole_body, cfg).await;
            }
            if uri == "/remove-logo" {
                ret = echo_remove_logo(cfg).await;
            }
            ret
        }
        Method::GET => {
            let cloned_cfg = { cfg.read().unwrap().clone() };
            if let Some(param) = match_uri(r"^/?((?P<param>[^/]+)/)?stats$", uri.as_str()) {
                ret = echo_stats(param, &cloned_cfg).await;
            }
            if let Some(param) = match_uri(r"^/?((?P<param>[^/]+)/)?info$", uri.as_str()) {
                ret = echo_info(param, &cloned_cfg, &remote_addr).await;
            }
            if uri.starts_with("/txlist") { ret = echo_txlist(&full_uri, &cloned_cfg).await; }
            if uri.starts_with("/txdetail") { ret = echo_txdetail(&full_uri, &cloned_cfg).await; }
            if uri == "/version" { ret = echo_version().await; }
            if uri == "/backup" { ret = echo_backup(&cloned_cfg).await; }
            if uri == "/backups/list" { ret = echo_backups_list(&cloned_cfg).await; }
            if uri == "/backup/config" { ret = echo_backup_config(&cloned_cfg).await; }
            if uri == "/.pub_key.pem" { ret = echo_pub_key(&cloned_cfg).await; }
            if uri == "/custom-logo" { ret = echo_custom_logo(&cloned_cfg).await; }
            if uri == "/" { ret = echo_home(&cloned_cfg).await; }
            ret
        }
        _ => ret,
    }
}

fn empty() -> BoxBody<Bytes, hyper::Error> {
    Empty::<Bytes>::new().map_err(|never| match never {}).boxed()
}

fn full<T: Into<Bytes>>(chunk: T) -> BoxBody<Bytes, hyper::Error> {
    Full::new(chunk.into()).map_err(|never| match never {}).boxed()
}

fn parse_env(cfg: &Arc<RwLock<MyConfig>>) {
    let mut cfg_write = cfg.write().unwrap();
    if let Ok(value) = env::var("BAL_SERVER_DB_FILE") { cfg_write.db_file = value; }
    if let Ok(value) = env::var("BAL_SERVER_BIND_ADDRESS") { cfg_write.bind_address = value; }
    if let Ok(value) = env::var("BAL_SERVER_BIND_PORT") {
        if let Ok(v) = value.parse::<u16>() { cfg_write.bind_port = v; }
    }
    if let Ok(value) = env::var("BAL_SERVER_PUB_KEY_PATH") { cfg_write.pub_key_path = value; }
    if let Ok(value) = env::var("BAL_SERVER_INFO") { cfg_write.info = value; }
    if let Ok(value) = env::var("BAL_PUBLIC_URL") { cfg_write.public_url = value; }
    parse_env_netconfig(&mut cfg_write, "regtest");
    parse_env_netconfig(&mut cfg_write, "signet");
    parse_env_netconfig(&mut cfg_write, "testnet");
    parse_env_netconfig(&mut cfg_write, "testnet4");
    let _ = parse_env_netconfig(&mut cfg_write, "bitcoin");
}

fn parse_env_netconfig(cfg_write: &mut MyConfig, chain: &str) {
    let cfg = match chain {
        "regtest" => &mut cfg_write.regtest,
        "signet" => &mut cfg_write.signet,
        "testnet" => &mut cfg_write.testnet,
        "testnet4" => &mut cfg_write.testnet4,
        &_ => &mut cfg_write.mainnet,
    };
    if let Ok(value) = env::var(format!("BAL_SERVER_{}_ADDRESS", chain.to_uppercase())) {
        cfg.address = value;
        if cfg.address.len() > 5 {
            if cfg.address[1..4] == *"pub" { cfg.xpub = true; }
            cfg.enabled = true;
        }
    }
    if let Ok(value) = env::var(format!("BAL_SERVER_{}_FIXED_FEE", chain.to_uppercase())) {
        if let Ok(v) = value.parse::<u64>() { cfg.fixed_fee = v; }
    }
}

fn init_network(db: &Connection, cfg: &MyConfig) {
    for network in NETWORKS {
        let netconfig = MyConfig::get_net_config(cfg, network);
        insert_xpub(db, &netconfig.name, &netconfig.address);
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    env_logger::init();
    let cfg: Arc<RwLock<MyConfig>> = Arc::<RwLock<MyConfig>>::default();
    parse_env(&cfg);
    load_settings_file(&cfg);

    let (addr, db_file) = {
        let cfg_read = cfg.read().unwrap();
        (cfg_read.bind_address.clone(), cfg_read.db_file.clone())
    };

    let db = open_db(&db_file).unwrap();
    {
        let cfg_read = cfg.read().unwrap();
        create_database(&db);
        init_network(&db, &cfg_read);
    }

    let addr: IpAddr = addr.parse()?;
    let listener = TcpListener::bind((addr, {
        let cfg_read = cfg.read().unwrap();
        cfg_read.bind_port
    })).await?;
    info!("Listening on http://{}", addr);

    loop {
        let (stream, _) = listener.accept().await?;
        let ip = stream.peer_addr()?.to_string().split(":").next().unwrap().to_string();
        let io = TokioIo::new(stream);
        let cfg = cfg.clone();
        tokio::task::spawn(async move {
            if let Err(err) = http1::Builder::new()
                .serve_connection(io, service_fn(|req: Request<hyper::body::Incoming>| async {
                    echo(req, &cfg, &ip).await
                }))
                .await
            {
                error!("Error serving connection: {:?}", err);
            }
        });
    }
}

