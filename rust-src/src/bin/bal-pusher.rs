//! bal-pusher — Block watcher and transaction broadcaster for Bitcoin After Life.
//!
//! Subscribes to Bitcoin Core's ZMQ `hashblock` topic. On each new block,
//! queries the SQLite DB for transactions whose locktime has been reached
//! (i.e., locktime < current MTP or block height) and broadcasts them
//! via Bitcoin Core RPC `sendrawtransaction`.
//!
//! Locktime logic:
//!   - locktime < 500_000_000  → interpreted as block height
//!   - locktime >= 500_000_000 → interpreted as Unix timestamp (MTP)
//! (This matches Bitcoin Core's own locktime semantics)
extern crate bitcoincore_rpc;
extern crate zmq;
use bitcoin::Network;

use bitcoincore_rpc::{Auth, Client, Error, RpcApi, bitcoin};
use bitcoincore_rpc_json::GetBlockchainInfoResult;

use byteorder::{LittleEndian, ReadBytesExt};
use hex;
use log::{debug, error, info, trace, warn};
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use serde_json::Value as SerdeValue;
use sqlite::{Connection, Value};
use std::collections::HashMap;
use std::env;
use std::error::Error as StdError;
use std::io::Cursor;
use std::str;
use std::{thread, time::Duration};
use zmq::{Context, DEALER, DONTWAIT, Socket};

use base64::{Engine as _, engine::general_purpose};
use chrono::{DateTime as ChronoDateTime, TimeDelta, Utc as ChronoUtc};
use openssl::pkey::PKey;
use openssl::sign::Signer;
use reqwest::Client as rClient;
use std::fs;
use std::time::Instant;

/// Threshold between block-height and timestamp locktimes (Bitcoin consensus rule).
/// Values below this are block heights; values at or above are Unix timestamps.
const LOCKTIME_THRESHOLD: i64 = 5000000;
/// Single source of truth: inherited from Cargo.toml `version`, same as bal-server.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Top-level pusher configuration. Loaded from environment at startup.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MyConfig {
    zmq_listener: String,
    db_file: String,
    bitcoin_dir: String,
    regtest: NetworkParams,
    testnet: NetworkParams,
    testnet4: NetworkParams,
    signet: NetworkParams,
    mainnet: NetworkParams,
    send_stats: bool,
    url: String,
    ssl_key_path: String,
}

impl Default for MyConfig {
    fn default() -> Self {
        MyConfig {
            zmq_listener: env::var("BAL_PUSHER_ZMQ_LISTENER")
                .unwrap_or("tcp://127.0.0.1:28332".to_string()),
            db_file: env::var("BAL_PUSHER_DB_FILE").unwrap_or("bal.db".to_string()),
            bitcoin_dir: env::var("BAL_PUSHER_BITCOIN_DIR").unwrap_or("".to_string()),
            regtest: get_network_params_default(Network::Regtest),
            testnet: get_network_params_default(Network::Testnet),
            testnet4: get_network_params_default(Network::Testnet4),
            signet: get_network_params_default(Network::Signet),
            mainnet: get_network_params_default(Network::Bitcoin),
            send_stats: env::var("BAL_PUSHER_SEND_STATS")
                .unwrap_or("false".to_string())
                .parse::<bool>()
                .unwrap(),
            url: env::var("BAL_SERVER_URL").unwrap_or("http://localhost/".to_string()),
            ssl_key_path: env::var("SSL_KEY_PATH").unwrap_or("privkey.pem".to_string()),
        }
    }
}

/// Open the SQLite database with sane concurrency settings.
///
/// Both bal-server and bal-pusher access the same file. Without WAL and a
/// busy timeout, concurrent writers can hit "database is locked" errors.
/// WAL lets readers and one writer proceed concurrently; busy_timeout makes
/// a contender wait instead of failing immediately.
/// (Ported from upstream bal-server 0.3.0 src/db.rs::open_db.)
fn open_db(path: &str) -> Result<Connection, sqlite::Error> {
    let conn = sqlite::open(path)?;
    // busy_timeout BEFORE journal_mode: the WAL switch itself may contend.
    let _ = conn.execute("PRAGMA busy_timeout = 5000;");
    let _ = conn.execute("PRAGMA journal_mode = WAL;");
    let _ = conn.execute("PRAGMA synchronous = NORMAL;");
    Ok(conn)
}

/// Per-network Bitcoin Core connection parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct NetworkParams {
    host: String,
    port: u16,
    dir_path: String,
    db_field: String,
    cookie_file: String,
    rpc_user: String,
    rpc_pass: String,
    zmq_listener: String,
}

fn get_network_params(cfg: &MyConfig, network: Network) -> &NetworkParams {
    match network {
        Network::Testnet => &cfg.testnet,
        Network::Testnet4 => &cfg.testnet4,
        Network::Signet => &cfg.signet,
        Network::Regtest => &cfg.regtest,
        _ => &cfg.mainnet,
    }
}

/// Build default NetworkParams for a given Bitcoin network variant.
fn get_network_params_default(network: Network) -> NetworkParams {
    match network {
        Network::Testnet => NetworkParams {
            host: "http://127.0.0.1".to_string(),
            port: 18332,
            dir_path: "testnet3/".to_string(),
            db_field: "testnet".to_string(),
            cookie_file: "".to_string(),
            rpc_user: "".to_string(),
            rpc_pass: "".to_string(),
            zmq_listener: "tcp://127.0.0.1:23332".to_string(),
        },
        Network::Testnet4 => NetworkParams {
            host: "http://127.0.0.1".to_string(),
            port: 18332,
            dir_path: "testnet4/".to_string(),
            db_field: "testnet4".to_string(),
            cookie_file: "".to_string(),
            rpc_user: "".to_string(),
            rpc_pass: "".to_string(),
            zmq_listener: "tcp://127.0.0.1:24332".to_string(),
        },
        Network::Signet => NetworkParams {
            host: "http://127.0.0.1".to_string(),
            port: 18332,
            dir_path: "signet/".to_string(),
            db_field: "signet".to_string(),
            cookie_file: "".to_string(),
            rpc_user: "".to_string(),
            rpc_pass: "".to_string(),
            zmq_listener: "tcp://127.0.0.1:22332".to_string(),
        },
        Network::Regtest => NetworkParams {
            host: "http://127.0.0.1".to_string(),
            port: 18443,
            dir_path: "regtest/".to_string(),
            db_field: "regtest".to_string(),
            cookie_file: "".to_string(),
            rpc_user: "".to_string(),
            rpc_pass: "".to_string(),
            zmq_listener: "tcp://127.0.0.1:21332".to_string(),
        },
        _ => NetworkParams {
            host: "http://127.0.0.1".to_string(),
            port: 8332,
            dir_path: "".to_string(),
            db_field: "bitcoin".to_string(),
            cookie_file: "".to_string(),
            rpc_user: "".to_string(),
            rpc_pass: "".to_string(),
            zmq_listener: "tcp://127.0.0.1:28332".to_string(),
        },
    }
}

/// Resolve the path to the Bitcoin RPC cookie file.
/// Uses the configured override if set; otherwise builds the default path from $HOME.
fn get_cookie_filename(network: &NetworkParams) -> Result<String, Box<dyn StdError>> {
    if network.cookie_file != "" {
        Ok(network.cookie_file.clone())
    } else {
        match env::var_os("HOME") {
            Some(home) => match home.to_str() {
                Some(home_str) => {
                    let cookie_file_path =
                        format!("{}/.bitcoin/{}.cookie", home_str, network.dir_path);
                    Ok(cookie_file_path)
                }
                None => Err("wrong HOME value".into()),
            },
            None => Err("Please Set HOME environment variable".into()),
        }
    }
}

/// Try to connect to Bitcoin Core using RPC username/password.
fn get_client_from_username(
    url: &String,
    network: &NetworkParams,
) -> Result<(Client, GetBlockchainInfoResult), Box<dyn StdError>> {
    if network.rpc_user != "" {
        match Client::new(
            &url[..],
            Auth::UserPass(network.rpc_user.to_string(), network.rpc_pass.to_string()),
        ) {
            Ok(client) => match client.get_blockchain_info() {
                Ok(bcinfo) => Ok((client, bcinfo)),
                Err(err) => Err(err.into()),
            },
            Err(err) => Err(err.into()),
        }
    } else {
        Err("Failed".into())
    }
}

/// Try to connect to Bitcoin Core using the cookie file (preferred for local nodes).
fn get_client_from_cookie(
    url: &String,
    network: &NetworkParams,
) -> Result<(Client, GetBlockchainInfoResult), Box<dyn StdError>> {
    match get_cookie_filename(network) {
        Ok(cookie) => match Client::new(&url[..], Auth::CookieFile(cookie.into())) {
            Ok(client) => match client.get_blockchain_info() {
                Ok(bcinfo) => Ok((client, bcinfo)),
                Err(err) => Err(err.into()),
            },
            Err(err) => Err(err.into()),
        },
        Err(err) => Err(err.into()),
    }
}

/// Connect to Bitcoin Core.
///
/// On Umbrel the cookie file is the most reliable auth method, because the
/// bitcoincore-rpc crate (v0.19) can fail HTTP-Basic auth (HTTP 401) with
/// Umbrel's auto-generated RPC password (it contains base64 chars like '=').
///
/// Strategy:
///   1. If a cookie file is configured AND exists → try cookie first.
///   2. Otherwise (or if cookie fails) → try username/password.
fn get_client(
    network: &NetworkParams,
) -> Result<(Client, GetBlockchainInfoResult), Box<dyn StdError>> {
    let url = format!("{}:{}/", network.host, &network.port);

    let cookie_available =
        !network.cookie_file.is_empty() && std::path::Path::new(&network.cookie_file).exists();

    if cookie_available {
        match get_client_from_cookie(&url, network) {
            Ok(client) => return Ok(client),
            Err(err) => {
                warn!("cookie auth failed ({}), falling back to user/password", err);
            }
        }
    }

    match get_client_from_username(&url, network) {
        Ok(client) => Ok(client),
        Err(user_err) => {
            // Last resort: try cookie even if it wasn't the primary choice
            match get_client_from_cookie(&url, network) {
                Ok(client) => Ok(client),
                Err(cookie_err) => Err(format!(
                    "all auth methods failed (user/pass: {} | cookie: {})",
                    user_err, cookie_err
                )
                .into()),
            }
        }
    }
}

/// Create a weekly backup of the database using VACUUM INTO.
///
/// Checks if today is Sunday (chrono: %u = Monday=1, Sunday=7).
/// Saves the backup to `{db_dir}/backup/bal-YYYYMMDD.db`.
/// Removes backups older than 12 weeks.
/// If a backup for this Sunday already exists, skips it.
fn load_backup_config(db_file: &str) -> SerdeValue {
    let cfg_path = format!(
        "{}/backup_config.json",
        std::path::Path::new(db_file).parent()
            .unwrap_or(std::path::Path::new("."))
            .to_string_lossy()
    );
    match std::fs::read_to_string(&cfg_path) {
        Ok(content) => serde_json::from_str::<SerdeValue>(&content).unwrap_or(SerdeValue::Null),
        Err(_) => SerdeValue::Null,
    }
}

fn run_weekly_backup(db_file: &str) {
    let now = ChronoUtc::now();
    // chrono %u: Monday=1, Tuesday=2, ..., Sunday=7
    let weekday = now.format("%u").to_string();
    if weekday != "7" {
        debug!("weekly backup: not Sunday (weekday={}), skipping", weekday);
        return;
    }

    let db_path = std::path::Path::new(db_file);
    let config = load_backup_config(db_file);
    let configured_path = config.get("backup_path").and_then(|v| v.as_str()).filter(|p| !p.is_empty());
    let backup_dir = match configured_path {
        Some(p) => std::path::PathBuf::from(p),
        None => db_path.parent().unwrap_or(std::path::Path::new(".")).join("backup"),
    };
    if let Err(e) = std::fs::create_dir_all(&backup_dir) {
        warn!("weekly backup: cannot create backup directory: {}", e);
        return;
    }

    let backup_filename = format!("bal-{}.db", now.format("%Y%m%d"));
    let backup_path = backup_dir.join(&backup_filename);

    // If backup for today already exists, skip
    if backup_path.exists() {
        info!("weekly backup: {} already exists, skipping", backup_filename);
        return;
    }

    // Use VACUUM INTO for a consistent snapshot (SQLite 3.27+)
    let bak_path_str = backup_path.to_string_lossy().to_string();
    let sql = format!("VACUUM INTO '{}'", bak_path_str.replace("'", "''"));
    match open_db(db_file) {
        Ok(db) => {
            if let Err(e) = db.execute(&sql) {
                warn!("weekly backup: VACUUM INTO failed: {}", e);
                return;
            }
            info!("weekly backup: created {}", backup_filename);

            // Remove backups older than 12 weeks
            let cutoff = now - TimeDelta::try_weeks(12).expect("12 weeks is a valid duration");
            if let Ok(entries) = std::fs::read_dir(&backup_dir) {
                for entry in entries.flatten() {
                    if let Ok(metadata) = entry.metadata() {
                        if let Ok(modified) = metadata.modified() {
                            let modified: ChronoDateTime<ChronoUtc> = modified.into();
                            if modified < cutoff {
                                if let Err(e) = std::fs::remove_file(entry.path()) {
                                    warn!("weekly backup: failed to remove old backup {:?}: {}", entry.path(), e);
                                } else {
                                    info!("weekly backup: removed old backup {:?}", entry.file_name());
                                }
                            }
                        }
                    }
                }
            }
        }
        Err(e) => {
            warn!("weekly backup: cannot open database: {}", e);
        }
    }
}

/// Core logic: query DB for matured transactions and broadcast them.
///
/// Called on startup and on every new block (via ZMQ notification).
/// Queries tbl_tx for rows where:
///   status = 0 (waiting) AND
///   (locktime < current block height  OR
///    (locktime > LOCKTIME_THRESHOLD AND locktime < median_time_past))
async fn main_result(cfg: &MyConfig, network_params: &NetworkParams) -> Result<(), Box<dyn StdError>> {
    match get_client(network_params) {
        Ok((rpc, bcinfo)) => {
            info!("connected");
            info!("median time: {}", bcinfo.median_time);
            info!("blocks: {}", bcinfo.blocks);
            debug!("best block hash: {}", bcinfo.best_block_hash);

            let average_time = bcinfo.median_time;
            let db = open_db(&cfg.db_file).unwrap();
            info!("db open {}", &cfg.db_file);

            // Safe now: defensive row reads, runs at most once per process.
            backfill_network_fees(&db, &rpc, &network_params.db_field);

            // This query implements Bitcoin's two-mode locktime semantics:
            //   - Block height:  locktime < current block height
            //   - Timestamp:     locktime > threshold AND locktime < MTP
            let sqlquery = "SELECT * FROM tbl_tx WHERE network = :network AND status = :status AND ( locktime < :bestblock_height  OR locktime > :locktime_threshold AND locktime < :bestblock_time);";
            let query_tx = db.prepare(sqlquery).unwrap().into_iter();
            trace!("query_tx: {}", sqlquery);
            trace!(":locktime_threshold: {}", LOCKTIME_THRESHOLD);
            trace!(":bestblock_time: {}", average_time);
            trace!(":bestblock_height: {}", bcinfo.blocks);
            trace!(":network: {}", network_params.db_field.clone());
            trace!(":status: {}", 0);

            let mut pushed_txs: Vec<String> = Vec::new();
            let mut invalid_txs: std::collections::HashMap<String, String> = HashMap::new();
            for row in query_tx
                .bind::<&[(_, Value)]>(
                    &[
                        (":locktime_threshold", (LOCKTIME_THRESHOLD as i64).into()),
                        (":bestblock_time", (average_time as i64).into()),
                        (":bestblock_height", (bcinfo.blocks as i64).into()),
                        (":network", network_params.db_field.clone().into()),
                        (":status", 0.into()),
                    ][..],
                )
                .unwrap()
                .map(|r| r.unwrap())
            {
                let txid: &str = row.read("txid");
                let tx: Option<&str> = row.read("tx");
                let locktime: i64 = row.read("locktime");
                let tx = match tx {
                    Some(t) if !t.is_empty() => t,
                    _ => {
                        warn!("skipping tx {}: missing or empty tx hex", txid);
                        continue;
                    }
                };
                info!("to be pushed: {}: {}", txid, locktime);
                match rpc.send_raw_transaction(tx) {
                    Ok(o) => {
                        info!("tx: {} pushed OK\n{}", txid, o);
                        pushed_txs.push(txid.to_string());
                        calc_and_store_network_fee(&db, &rpc, txid, tx);
                    }
                    Err(err) => {
                        warn!("Error: {}\n{}", err, txid);
                        invalid_txs.insert(txid.to_string(), err.to_string());
                    }
                };
            }

            // Update status for successfully broadcast transactions
            if pushed_txs.len() > 0 {
                let sql = format!(
                    "UPDATE tbl_tx SET status = 1 WHERE txid in ('{}');",
                    pushed_txs.join("','")
                );
                trace!("sqlok: {}", &sql);
                let _ = db.execute(&sql);
            }

            // Update status for failed transactions (status=2 means broadcast failed)
            if invalid_txs.len() > 0 {
                for (txid, txerr) in &invalid_txs {
                    let sql = format!(
                        "UPDATE tbl_tx SET status = 2, push_err='{txerr}' WHERE txid = '{txid}'"
                    );
                    trace!("sqlerror: {}", &sql);
                    let _ = db.execute(&sql);
                }
            }

            // Check blockchain confirmations for already-sent transactions.
            // Uses get_raw_transaction_info RPC (requires txindex=1 on Bitcoin Core).
            // Updates the confirmations column in tbl_tx which is exposed via the API.
            let check_sql = format!(
                "SELECT txid FROM tbl_tx WHERE status = 1 AND network = '{}'",
                network_params.db_field
            );
            if let Ok(mut check_stmt) = db.prepare(&check_sql) {
                let mut conf_txids: Vec<String> = Vec::new();
                while let Ok(sqlite::State::Row) = check_stmt.next() {
                    if let Ok(t) = check_stmt.read::<String, _>("txid") {
                        conf_txids.push(t);
                    }
                }
                for txid_str in &conf_txids {
                    if let Ok(txid) = txid_str.parse::<bitcoin::Txid>() {
                        match rpc.get_raw_transaction_info(&txid, None) {
                            Ok(info) => {
                                let confs = info.confirmations.unwrap_or(0) as i64;
                                let sql = format!("UPDATE tbl_tx SET confirmations = {confs} WHERE txid = '{txid_str}'");
                                let _ = db.execute(&sql);
                            }
                            Err(_) => {
                                match rpc.get_mempool_entry(&txid) {
                                    Ok(_) => {
                                        debug!("confirm check: {} still in mempool, unconfirmed", txid_str);
                                    }
                                    Err(_) => {
                                        warn!("tx {} not in mempool or blockchain — likely double-spent by another will executor", txid_str);
                                        let sql = format!("UPDATE tbl_tx SET status = 2, confirmations = -1, push_err = 'double-spent: tx evicted from mempool' WHERE txid = '{txid_str}'");
                                        let _ = db.execute(&sql);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if let Err(e) = send_stats_report(cfg, bcinfo).await {
                // Never discard silently: a failing welist report went unnoticed
                // for days once (broken IPv4 route) because the error was swallowed.
                warn!("send_stats_report failed: {}", e);
            }
            if let Err(e) = calculate_stats(&db, network_params.db_field.clone()).await {
                warn!("calculate_stats failed: {}", e);
            }
        }
        Err(erx) => {
            return Err(format!("impossible to get client {}", erx).into());
        }
    }
    Ok(())
}

/// Calculate and store the network fee for a broadcast transaction.
///
/// Parses the raw tx hex, fetches each input's value from the blockchain
/// via RPC, then stores `sum(inputs) - sum(outputs)` as the miner fee.
fn calc_and_store_network_fee(db: &Connection, rpc: &Client, txid: &str, tx_hex: &str) {
    if let Ok(tx_bytes) = hex::decode(tx_hex) {
        if let Ok(decoded_tx) = bitcoin::consensus::encode::deserialize::<bitcoin::Transaction>(&tx_bytes) {
            let total_output: u64 = decoded_tx.output.iter().map(|o| o.value.to_sat()).sum();
            let mut total_input: u64 = 0;
            let mut fee_err = false;
            for inp in &decoded_tx.input {
                let prevout = &inp.previous_output;
                match rpc.get_raw_transaction(&prevout.txid, None) {
                    Ok(parent_tx) => {
                        if let Some(txout) = parent_tx.output.get(prevout.vout as usize) {
                            total_input += txout.value.to_sat();
                        } else {
                            warn!("network_fee: vout {} not found in parent tx {} for input of {}", prevout.vout, prevout.txid, txid);
                            fee_err = true;
                            break;
                        }
                    }
                    Err(e) => {
                        warn!("network_fee: cannot fetch parent tx {} for {}: {}", prevout.txid, txid, e);
                        fee_err = true;
                        break;
                    }
                }
            }
            if !fee_err && total_input > total_output {
                let network_fees = (total_input - total_output).to_string();
                match db.prepare("UPDATE tbl_tx SET network_fees = ? WHERE txid = ?") {
                    Ok(mut stmt) => {
                        let _ = stmt.bind((1, Value::String(network_fees.clone())));
                        let _ = stmt.bind((2, Value::String(txid.to_string())));
                        let _ = stmt.next();
                    }
                    Err(e) => warn!("network_fee: failed to prepare update for {}: {}", txid, e),
                }
                info!("tx: {} network fee: {} sats", txid, network_fees);
            } else if !fee_err {
                warn!("network_fee: total_input ({}) <= total_output ({}) for tx {}", total_input, total_output, txid);
            }
        }
    }
}

/// Backfill network fees for all previously broadcast transactions
/// that have an empty `network_fees` column.
///
/// Runs at most once per process (guarded by BACKFILL_DONE) — main_result is
/// called on every block, and without the guard a tx whose parent cannot be
/// fetched would be retried (with RPC calls) on every single block forever.
fn backfill_network_fees(db: &Connection, rpc: &Client, network: &str) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static BACKFILL_DONE: AtomicBool = AtomicBool::new(false);
    if BACKFILL_DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    let sql = "SELECT txid, tx FROM tbl_tx WHERE (network_fees IS NULL OR network_fees = '') AND network = ? AND status IN (0, 1)";
    let stmt = match db.prepare(sql) {
        Ok(s) => s,
        Err(e) => {
            warn!("backfill_network_fees: failed to prepare query: {}", e);
            return;
        }
    };
    let iter = match stmt.into_iter()
        .bind((1, Value::String(network.to_string())))
    {
        Ok(it) => it,
        Err(e) => {
            warn!("backfill_network_fees: failed to bind: {}", e);
            return;
        }
    };
    let mut count = 0u32;
    for row in iter {
        // Never unwrap a row result and never read columns as &str directly:
        // a single row holding an unexpected type (e.g. BLOB in a TEXT
        // column) used to panic inside the sqlite crate ("failed to convert")
        // and crash-loop the whole pusher.
        let mut row = match row {
            Ok(r) => r,
            Err(e) => {
                warn!("backfill_network_fees: row read error: {}", e);
                continue;
            }
        };
        // take() returns the raw sqlite::Value without any conversion, so a
        // BLOB or NULL in a TEXT column can no longer panic the process.
        let txid = match row.take("txid") {
            Value::String(s) => s,
            _ => continue,
        };
        let tx_hex = match row.take("tx") {
            Value::String(s) => s,
            Value::Binary(b) => hex::encode(b),
            _ => continue,
        };
        if tx_hex.is_empty() {
            continue;
        }
        info!("network_fee backfill: processing tx {}", &txid);
        calc_and_store_network_fee(db, rpc, &txid, &tx_hex);
        count += 1;
    }
    if count > 0 {
        info!("backfill_network_fees: calculated network fees for {} transaction(s) on {}", count, network);
    }
}

/// Recompute aggregate statistics and upsert into tbl_stats.
/// Called after each block so the stats endpoint always has fresh data.
async fn calculate_stats(db: &Connection, chain: String) -> Result<(), reqwest::Error> {
    let sql = "DELETE FROM tbl_stats WHERE chain = '{chain}';";
    if let Err(err) = db.execute(&sql) {
        error!("error deleting from tbl_stats where chain:{chain} error: {err}");
    }
    let sql = format!(
        "INSERT INTO tbl_stats (
  report_date, chain, totals, waiting, sent, failed,
  waiting_profit, sent_profit, missed_profit, unique_inputs
)
VALUES (
  CURRENT_TIMESTAMP,
  '{chain}',
  (SELECT COUNT(*) FROM tbl_tx WHERE network = '{chain}'),
  (SELECT COUNT(*) FROM tbl_tx WHERE status = 0 AND (locktime < 500000000 OR locktime > CAST(strftime('%s','now') AS INTEGER)) AND network = '{chain}'),
  (SELECT COUNT(*) FROM tbl_tx WHERE status = 1 AND network = '{chain}'),
  (SELECT COUNT(*) FROM tbl_tx WHERE status = 2 AND network = '{chain}'),
  (SELECT IFNULL(SUM(our_fees),0) FROM tbl_tx WHERE status = 0 AND (locktime < 500000000 OR locktime > CAST(strftime('%s','now') AS INTEGER)) AND network = '{chain}'),
  (SELECT IFNULL(SUM(our_fees),0) FROM tbl_tx WHERE status = 1 AND network = '{chain}'),
  (SELECT IFNULL(SUM(our_fees),0) FROM tbl_tx WHERE status = 2 AND network = '{chain}'),
  (SELECT COUNT(DISTINCT tbl_inp.in_txid)
     FROM tbl_inp
     JOIN tbl_tx ON tbl_inp.txid = tbl_tx.txid
     WHERE tbl_tx.status = 0 AND (tbl_tx.locktime < 500000000 OR tbl_tx.locktime > CAST(strftime('%s','now') AS INTEGER)) AND tbl_tx.network = '{chain}')
)
ON CONFLICT(chain) DO UPDATE SET
  report_date = excluded.report_date,
  totals = excluded.totals,
  waiting = excluded.waiting,
  sent = excluded.sent,
  failed = excluded.failed,
  waiting_profit = excluded.waiting_profit,
  sent_profit = excluded.sent_profit,
  missed_profit = excluded.missed_profit,
  unique_inputs = excluded.unique_inputs;
  "
    );

    if let Err(err) = db.execute(&sql) {
        error!("error inserting creating stats table {err}");
    } else {
        info!("tbl_stats creation success");
    }
    Ok(())
}

/// Optionally send block timing statistics to the welist aggregator.
/// Only active when BAL_PUSHER_SEND_STATS=true.
/// The payload is signed with the Ed25519 private key for authenticity.
async fn send_stats_report(
    cfg: &MyConfig,
    bcinfo: GetBlockchainInfoResult,
) -> Result<(), reqwest::Error> {
    if cfg.send_stats {
        debug!("sending report to welist");
        let welist_url = env::var("WELIST_SERVER_URL")
            .unwrap_or("https://welist.bitcoin-after.life".to_string());

        // NOTE: welist.bitcoin-after.life publishes both A (IPv4) and AAAA
        // (IPv6) records. On the Umbrel network the IPv4 route to the welist
        // host is broken (connections time out at the data stage), while IPv6
        // works. The glibc resolver / happy-eyeballs would otherwise try the
        // broken IPv4 address first and stall. We therefore resolve the host
        // and, when an IPv6 address is available (the container is attached to
        // an IPv6-enabled docker network — see docker-compose.yml), pin the
        // connection to it, keeping the hostname for the Host header/TLS SNI.
        // If no IPv6 address is available we fall back to the default resolver.
        let (welist_host, welist_port) = parse_host_port(&welist_url);
        let client = {
            let mut builder = rClient::builder();
            if let Some(ipv6_addr) = resolve_preferring_ipv6(&welist_host, welist_port).await {
                debug!("pinning {} to address {}", welist_host, ipv6_addr);
                builder = builder.resolve(&welist_host, ipv6_addr);
            } else {
                debug!("no resolved address to pin for {}, using default resolver", welist_host);
            }
            builder.build().unwrap_or_else(|_| rClient::new())
        };
        let url = format!("{}/ping", welist_url);
        debug!("welist url: {}", url);
        let chain = bcinfo.chain.to_string().to_lowercase();
        let message = format!(
            "{0}{1}{2}{3}{4}",
            cfg.url, chain, bcinfo.blocks, bcinfo.median_time, bcinfo.best_block_hash
        );
        trace!("message to be sent: {}", message);
        let sign = sign_message(cfg.ssl_key_path.as_str(), &message.as_str());
        let response = client
            .post(url)
            .header("User-Agent", format!("bal-pusher/{}", VERSION))
            .json(&json!(
            {
                "url":              cfg.url,
                "chain":            chain,
                "height":           bcinfo.blocks,
                "median_time":      bcinfo.median_time,
                "last_block_hash":  bcinfo.best_block_hash,
                "signature":        sign,
            }))
            .send()
            .await?;
        if !response.status().is_success() {
            warn!(
                "Non-success response: {} {}",
                response.status(),
                response.status().canonical_reason().unwrap_or("")
            );
        }
        let body = &(response.text().await?);
        info!("Report to welist({})\tSent: {}", welist_url, body);
    } else {
        debug!("Not sending stats");
    }
    Ok(())
}

/// Extract the (host, port) pair from a base URL like "https://host[:port]".
/// Defaults to port 443 for https and 80 for http/anything else.
fn parse_host_port(base_url: &str) -> (String, u16) {
    let without_scheme = base_url.splitn(2, "://").nth(1).unwrap_or(base_url);
    let is_https = base_url.starts_with("https://");
    let host_part = without_scheme.split('/').next().unwrap_or(without_scheme);
    match host_part.rsplit_once(':') {
        Some((host, port_str)) => {
            let port = port_str
                .parse::<u16>()
                .unwrap_or(if is_https { 443 } else { 80 });
            (host.to_string(), port)
        }
        None => (host_part.to_string(), if is_https { 443 } else { 80 }),
    }
}

/// Resolve `host` and return an address, preferring IPv6 (AAAA) when present.
/// On the Umbrel network the IPv4 route to the welist host is broken, so we
/// pin to the IPv6 address when the container has IPv6 connectivity. Falls
/// back to the first IPv4 address if no IPv6 address is available.
async fn resolve_preferring_ipv6(host: &str, port: u16) -> Option<std::net::SocketAddr> {
    use std::net::ToSocketAddrs;
    let host = host.to_string();
    tokio::task::spawn_blocking(move || {
        let addrs: Vec<std::net::SocketAddr> =
            format!("{}:{}", host, port).to_socket_addrs().ok()?.collect();
        addrs
            .iter()
            .find(|a| a.is_ipv6())
            .cloned()
            .or_else(|| addrs.into_iter().next())
    })
    .await
    .ok()
    .flatten()
}

/// Sign a message with the Ed25519 private key and return it as a Base64 string.
/// Used to authenticate stats reports sent to the welist aggregator.
fn sign_message(private_key_path: &str, message: &str) -> String {
    let key_data = fs::read(private_key_path).unwrap();
    let private_key = PKey::private_key_from_pem(&key_data).unwrap();
    let mut signer = Signer::new_without_digest(&private_key).unwrap();
    let signature = signer.sign_oneshot_to_vec(message.as_bytes()).unwrap();
    let signature_b64 = general_purpose::STANDARD.encode(&signature);
    signature_b64
}

/// Parse BAL_PUSHER_* environment variables into the config struct.
fn parse_env(cfg: &mut MyConfig) {
    cfg.regtest = parse_env_netconfig(cfg, "regtest");
    cfg.signet = parse_env_netconfig(cfg, "signet");
    cfg.testnet = parse_env_netconfig(cfg, "testnet");
    cfg.testnet4 = parse_env_netconfig(cfg, "testnet4");
    drop(parse_env_netconfig(cfg, "bitcoin"));
}

/// Parse per-network environment variables for a given chain name.
fn parse_env_netconfig(cfg_lock: &mut MyConfig, chain: &str) -> NetworkParams {
    let cfg = match chain {
        "regtest" => &mut cfg_lock.regtest,
        "signet" => &mut cfg_lock.signet,
        "testnet" => &mut cfg_lock.testnet,
        "testnet4" => &mut cfg_lock.testnet4,
        &_ => &mut cfg_lock.mainnet,
    };
    if let Ok(value) = env::var(format!("BAL_PUSHER_{}_HOST", chain.to_uppercase())) {
        cfg.host = value;
    }
    if let Ok(value) = env::var(format!("BAL_PUSHER_{}_PORT", chain.to_uppercase())) {
        if let Ok(value) = value.parse::<u64>() {
            cfg.port = value.try_into().unwrap();
        }
    }
    if let Ok(value) = env::var(format!("BAL_PUSHER_{}_DIR_PATH", chain.to_uppercase())) {
        cfg.dir_path = value;
    }
    if let Ok(value) = env::var(format!("BAL_PUSHER_{}_DB_FIELD", chain.to_uppercase())) {
        cfg.db_field = value;
    }
    if let Ok(value) = env::var(format!("BAL_PUSHER_{}_COOKIE_FILE", chain.to_uppercase())) {
        cfg.cookie_file = value;
    }
    if let Ok(value) = env::var(format!("BAL_PUSHER_{}_RPC_USER", chain.to_uppercase())) {
        cfg.rpc_user = value;
    }
    if let Ok(value) = env::var(format!("BAL_PUSHER_{}_RPC_PASSWORD", chain.to_uppercase())) {
        cfg.rpc_pass = value;
    }
    // FIX: the BAL_PUSHER_*_ZMQ_HASHBLOCK env var must populate the ZMQ listener,
    // not rpc_pass (which was a bug in the original code that broke ZMQ on mainnet).
    if let Ok(value) = env::var(format!("BAL_PUSHER_{}_ZMQ_HASHBLOCK", chain.to_uppercase())) {
        cfg.zmq_listener = value;
    }
    cfg.clone()
}

/// Try a non-blocking send on a ZMQ DEALER socket to test if the endpoint is reachable.
fn check_zmq_connection(endpoint: &str) -> bool {
    trace!("check zmq connection");
    let context = Context::new();
    let socket = match context.socket(DEALER) {
        Ok(sock) => sock,
        Err(_) => return false,
    };
    if socket.connect(endpoint).is_err() {
        return false;
    }
    // DONTWAIT makes send() return immediately instead of blocking
    socket.send("", DONTWAIT).is_ok()
}

/// Tracks ZMQ connection health to detect stalled nodes.
struct ConnectionMonitor {
    last_message_time: Instant,
    timeout: Duration,
    consecutive_timeouts: u32,
    max_consecutive_timeouts: u32,
}

impl ConnectionMonitor {
    fn new(timeout_secs: u64, max_timeouts: u32) -> Self {
        Self {
            last_message_time: Instant::now(),
            timeout: Duration::from_secs(timeout_secs),
            consecutive_timeouts: 0,
            max_consecutive_timeouts: max_timeouts,
        }
    }

    fn update(&mut self) {
        self.last_message_time = Instant::now();
        self.consecutive_timeouts = 0;
    }

    fn check_connection(&mut self) -> ConnectionStatus {
        let elapsed = self.last_message_time.elapsed();
        if elapsed > self.timeout {
            self.consecutive_timeouts += 1;
            if self.consecutive_timeouts >= self.max_consecutive_timeouts {
                ConnectionStatus::Lost(elapsed)
            } else {
                ConnectionStatus::Warning(elapsed)
            }
        } else {
            ConnectionStatus::Healthy
        }
    }

    fn reset(&mut self) {
        self.consecutive_timeouts = 0;
        self.last_message_time = Instant::now();
    }
}

enum ConnectionStatus {
    Healthy,
    Warning(Duration),
    Lost(Duration),
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    env_logger::init();
    let mut cfg = MyConfig::default();

    // BAL_PUSHER_DB_FILE is required (not optional) so we use unwrap here.
    // If it's missing, the process crashes with a clear error message.
    let _dbfile = env::var("BAL_PUSHER_DB_FILE").unwrap();
    parse_env(&mut cfg);

    // The network to watch is passed as a command-line argument (default: "bitcoin")
    let mut args = std::env::args();
    let _exe_name = args.next().unwrap();
    let arg_network = match args.next() {
        Some(nargs) => nargs,
        None => "bitcoin".to_string(),
    };
    let network = match arg_network.as_str() {
        "testnet" => Network::Testnet,
        "testnet4" => Network::Testnet4,
        "signet" => Network::Signet,
        "regtest" => Network::Regtest,
        _ => Network::Bitcoin,
    };

    info!("Network: {}", arg_network);
    let network_params = get_network_params(&cfg, network);

    // Subscribe to Bitcoin Core's ZMQ hashblock topic.
    // hashblock publishes the 32-byte block hash on every new block.
    let context = Context::new();
    let socket: Socket = context.socket(zmq::SUB).unwrap();

    // Resolve the ZMQ listener address with the following priority:
    //   1. network_params.zmq_listener  (from BAL_PUSHER_BITCOIN_ZMQ_HASHBLOCK)
    //   2. cfg.zmq_listener             (from BAL_PUSHER_ZMQ_LISTENER)
    // Any non-loopback value wins over the hardcoded 127.0.0.1 default.
    let zmq_address = if !network_params.zmq_listener.starts_with("tcp://127.0.0.1") {
        network_params.zmq_listener.clone()
    } else if !cfg.zmq_listener.starts_with("tcp://127.0.0.1") {
        cfg.zmq_listener.clone()
    } else {
        network_params.zmq_listener.clone()
    };
    info!("zmq listening on: {}", zmq_address);
    socket.connect(&zmq_address).unwrap();

    // Subscribe to all topics (empty filter = receive everything)
    socket.set_subscribe(b"").unwrap();

    // Run once immediately at startup (process any txs that matured while we were offline)
    // Retry up to 10 times with 6s delay in case Bitcoin Core is not yet reachable
    for attempt in 1..=10u32 {
        match main_result(&cfg, network_params).await {
            Ok(_) => break,
            Err(e) => {
                warn!("startup main_result attempt {}/10 failed: {}. Retrying in 6s├óÔé¼┬ª", attempt, e);
                thread::sleep(Duration::from_secs(6));
            }
        }
    }
    // Weekly automatic backup (checks if Sunday)
    run_weekly_backup(&cfg.db_file);

    info!("waiting new blocks..");

    let mut last_seq: Vec<u8> = [0; 4].to_vec();
    loop {
        let message = socket.recv_multipart(0).unwrap();
        let topic = message[0].clone();
        let body = message[1].clone();
        let seq = message[2].clone();
        last_seq = seq;
        debug!(
            "ZMQ:GET TOPIC: {}",
            String::from_utf8(topic.clone()).expect("invalid topic")
        );
        trace!("ZMQ:GET BODY: {}", hex::encode(&body));
        if topic == b"hashblock" {
            info!("NEW BLOCK: {}", hex::encode(&body));
            let _ = main_result(&cfg, network_params).await;
        }
        // Small sleep to avoid busy-waiting if ZMQ delivers messages in bursts
        thread::sleep(Duration::from_millis(100));
    }
}

/// Convert a 4-byte little-endian ZMQ sequence number to a readable string.
fn seq_to_str(seq: &Vec<u8>) -> String {
    if seq.len() == 4 {
        let mut rdr = Cursor::new(seq);
        let sequence = rdr
            .read_u32::<LittleEndian>()
            .expect("Failed to read integer");
        return sequence.to_string();
    }
    "Unknown".to_string()
}
