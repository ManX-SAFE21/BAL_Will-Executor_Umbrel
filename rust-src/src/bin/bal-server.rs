use actix_governor::{Governor, GovernorConfigBuilder};
use actix_web::middleware;
use actix_web::web::Bytes;
use actix_web::{App, HttpResponse, HttpServer, Responder, web};
use bitcoin::{Network, Transaction, consensus};
use chrono::Utc;
use hex_conservative::FromHex;
use log::{debug, error, info, trace};
use serde::{Deserialize, Serialize};
use sqlite::State;
use sqlite::{Connection, Value};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::sync::Mutex;

use bal_server::db::{
    check_duplicate_txids, create_database, execute_insert, get_all_addresses_by_xpub,
    get_last_used_address_by_ip, get_next_address_index, insert_xpub, open_db, save_new_address,
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
                .unwrap_or(false),
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

#[derive(Debug, Serialize, Deserialize)]
pub struct InfoResponse {
    pub address: String,
    pub base_fee: u64,
    pub chain: String,
    pub info: String,
    pub version: String,
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
}

#[derive(Debug, Clone)]
#[expect(dead_code)]
struct ActixConfig {
    max_body_size: usize,
    timeout_secs: u64,
    rate_limit_pushtxs: (u64, u32),
    rate_limit_searchtx: (u64, u32),
    rate_limit_info: (u64, u32),
    rate_limit_default: (u64, u32),
    workers: usize,
    max_connections: usize,
}

fn parse_actix_config() -> ActixConfig {
    ActixConfig {
        max_body_size: env::var("BAL_SERVER_ACTIX_MAX_BODY_SIZE")
            .unwrap_or("1048576".to_string())
            .parse::<usize>()
            .unwrap_or(1_048_576),
        timeout_secs: env::var("BAL_SERVER_ACTIX_TIMEOUT_SECS")
            .unwrap_or("5".to_string())
            .parse::<u64>()
            .unwrap_or(5),
        rate_limit_pushtxs: (
            env::var("BAL_SERVER_ACTIX_PUSHTXS_PER_SEC")
                .unwrap_or("1".to_string())
                .parse::<u64>()
                .unwrap_or(1),
            env::var("BAL_SERVER_ACTIX_PUSHTXS_BURST")
                .unwrap_or("3".to_string())
                .parse::<u32>()
                .unwrap_or(3),
        ),
        rate_limit_searchtx: (
            env::var("BAL_SERVER_ACTIX_SEARCHTX_PER_SEC")
                .unwrap_or("5".to_string())
                .parse::<u64>()
                .unwrap_or(5),
            env::var("BAL_SERVER_ACTIX_SEARCHTX_BURST")
                .unwrap_or("10".to_string())
                .parse::<u32>()
                .unwrap_or(10),
        ),
        rate_limit_info: (
            env::var("BAL_SERVER_ACTIX_INFO_PER_SEC")
                .unwrap_or("20".to_string())
                .parse::<u64>()
                .unwrap_or(20),
            env::var("BAL_SERVER_ACTIX_INFO_BURST")
                .unwrap_or("30".to_string())
                .parse::<u32>()
                .unwrap_or(30),
        ),
        rate_limit_default: (
            env::var("BAL_SERVER_ACTIX_DEFAULT_PER_SEC")
                .unwrap_or("50".to_string())
                .parse::<u64>()
                .unwrap_or(50),
            env::var("BAL_SERVER_ACTIX_DEFAULT_BURST")
                .unwrap_or("100".to_string())
                .parse::<u32>()
                .unwrap_or(100),
        ),
        workers: env::var("BAL_SERVER_ACTIX_WORKERS")
            .unwrap_or("4".to_string())
            .parse::<usize>()
            .unwrap_or(4),
        max_connections: env::var("BAL_SERVER_ACTIX_MAX_CONNECTIONS")
            .unwrap_or("100".to_string())
            .parse::<usize>()
            .unwrap_or(100),
    }
}

struct AppState {
    db: Mutex<Connection>,
    cfg: MyConfig,
}

async fn echo_home(data: web::Data<AppState>) -> impl Responder {
    HttpResponse::Ok().body(data.cfg.info.clone())
}

async fn echo_pub_key(data: web::Data<AppState>) -> impl Responder {
    match fs::read_to_string(&data.cfg.pub_key_path) {
        Ok(pub_key) => HttpResponse::Ok().body(pub_key),
        Err(e) => {
            error!(
                "Failed to read public key file {}: {}",
                data.cfg.pub_key_path, e
            );
            HttpResponse::InternalServerError().body("error")
        }
    }
}

async fn echo_version() -> impl Responder {
    HttpResponse::Ok().body(VERSION)
}

fn is_valid_ip(ip: &str) -> bool {
    ip.parse::<std::net::IpAddr>().is_ok()
}

fn extract_client_ip(req: &actix_web::HttpRequest) -> String {
    if let Some(val) = req.headers().get("X-Real-IP")
        && let Ok(s) = val.to_str()
    {
        let ip = s.split(',').next().unwrap_or(s).trim();
        if is_valid_ip(ip) {
            debug!("client IP from X-Real-IP: {}", ip);
            return ip.to_string();
        }
    }
    if let Some(val) = req.headers().get("X-Forwarded-For")
        && let Ok(s) = val.to_str()
    {
        let ip = s.split(',').next().unwrap_or(s).trim();
        if is_valid_ip(ip) {
            debug!("client IP from X-Forwarded-For: {}", ip);
            return ip.to_string();
        }
    }
    let fallback = req
        .connection_info()
        .peer_addr()
        .unwrap_or("unknown")
        .to_string();
    debug!("client IP from peer_addr fallback: {}", fallback);
    fallback
}

async fn echo_info(
    path: web::Path<String>,
    data: web::Data<AppState>,
    req: actix_web::HttpRequest,
) -> impl Responder {
    let param = path.into_inner();
    if !NETWORKS.contains(&param.as_str()) {
        return HttpResponse::NotFound().body("error");
    }
    info!("echo info!!!{}", param);
    let netconfig = data.cfg.get_net_config(&param);
    if !netconfig.enabled {
        debug!("network disabled {}", param);
        return HttpResponse::BadRequest().body("error");
    }
    let remote_addr = extract_client_ip(&req);
    let address = match netconfig.xpub {
        false => {
            let address = netconfig.address.to_string();
            trace!("is address: {}", &address);
            address
        }
        true => {
            // Lock #1: fetch existing address OR atomically claim next index
            let next_idx = {
                let db = match data.db.lock() {
                    Ok(g) => g,
                    Err(_p) => {
                        error!("DB mutex poisoned in echo_info (lookup phase)");
                        return HttpResponse::InternalServerError().body("error");
                    }
                };
                match get_last_used_address_by_ip(
                    &db,
                    &netconfig.name,
                    &netconfig.address,
                    &remote_addr,
                ) {
                    Some(address) => {
                        return HttpResponse::Ok().json(InfoResponse {
                            address,
                            base_fee: netconfig.fixed_fee,
                            chain: netconfig.network.to_string(),
                            info: data.cfg.info.to_string(),
                            version: VERSION.to_string(),
                        });
                    }
                    None => get_next_address_index(&db, &netconfig.name, &netconfig.address),
                }
            }; // lock released

            // Derive address (CPU-bound, no lock held)
            let derived =
                match new_address_from_xpub(&netconfig.address, next_idx.1, netconfig.network) {
                    Ok(address) => address,
                    Err(e) => {
                        error!("Failed to derive address from xpub: {}", e);
                        return HttpResponse::BadRequest().body("error");
                    }
                };

            // Lock #2: save the newly derived address
            {
                let db = match data.db.lock() {
                    Ok(g) => g,
                    Err(_p) => {
                        error!("DB mutex poisoned in echo_info (save phase)");
                        return HttpResponse::InternalServerError().body("error");
                    }
                };
                save_new_address(&db, next_idx.0, &derived.0, &derived.1, &remote_addr);
                debug!("save new address {} {}", derived.0, derived.1);
                trace!("next {} {}", next_idx.0, next_idx.1);
                derived.0
            } // lock released
        }
    };
    let info = InfoResponse {
        address,
        base_fee: netconfig.fixed_fee,
        chain: netconfig.network.to_string(),
        info: data.cfg.info.to_string(),
        version: VERSION.to_string(),
    };
    trace!("address: {:#?}", info);
    match serde_json::to_string(&info) {
        Ok(json_data) => {
            debug!("echo info reply: {}", json_data);
            HttpResponse::Ok().json(info)
        }
        Err(_err) => HttpResponse::InternalServerError().body("error"),
    }
}

async fn echo_stats(path: web::Path<String>, data: web::Data<AppState>) -> impl Responder {
    let param = path.into_inner();
    if !NETWORKS.contains(&param.as_str()) {
        return HttpResponse::NotFound().body("error");
    }
    info!("echo stats!!! {}", data.cfg.expose_stats);
    let netconfig = data.cfg.get_net_config(&param);
    if !netconfig.enabled {
        debug!("network disabled {}", param);
        return HttpResponse::BadRequest().body("error");
    }
    if !data.cfg.expose_stats {
        return HttpResponse::Forbidden().body("error");
    }
    let mut stats: Vec<StatsResponse> = vec![];
    let db = match data.db.lock() {
        Ok(g) => g,
        Err(_p) => {
            error!("DB mutex poisoned in echo_stats");
            return HttpResponse::InternalServerError().body("error");
        }
    };
    let mut stmt = match db.prepare(
        "SELECT report_date, chain, totals, waiting, sent, failed, waiting_profit, sent_profit, missed_profit, unique_inputs FROM tbl_stats WHERE chain = ?"
    ) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to prepare stats query: {}", e);
            return HttpResponse::InternalServerError().body("error");
        }
    };
    if let Err(e) = stmt.bind((1, Value::String(netconfig.name.clone()))) {
        error!("Failed to bind chain in stats query: {}", e);
        return HttpResponse::InternalServerError().body("error");
    }
    while let Ok(State::Row) = stmt.next() {
        let report_date = stmt.read("report_date").unwrap_or("0".to_string());
        let chain = stmt.read("chain").unwrap_or("?".to_string());
        let totals = stmt
            .read("totals")
            .unwrap_or("0".to_string())
            .parse::<i64>()
            .unwrap_or(0);
        let waiting = stmt
            .read("waiting")
            .unwrap_or("0".to_string())
            .parse::<i64>()
            .unwrap_or(0);
        let sent = stmt
            .read("sent")
            .unwrap_or("0".to_string())
            .parse::<i64>()
            .unwrap_or(0);
        let failed = stmt
            .read("failed")
            .unwrap_or("0".to_string())
            .parse::<i64>()
            .unwrap_or(0);
        let waiting_profit = stmt
            .read("waiting_profit")
            .unwrap_or("0".to_string())
            .parse::<i64>()
            .unwrap_or(0);
        let sent_profit = stmt
            .read("sent_profit")
            .unwrap_or("0".to_string())
            .parse::<i64>()
            .unwrap_or(0);
        let missed_profit = stmt
            .read("missed_profit")
            .unwrap_or("0".to_string())
            .parse::<i64>()
            .unwrap_or(0);
        let unique_inputs = stmt
            .read("unique_inputs")
            .unwrap_or("0".to_string())
            .parse::<i64>()
            .unwrap_or(0);
        stats.push(StatsResponse {
            report_date,
            chain,
            totals,
            waiting,
            sent,
            failed,
            waiting_profit,
            sent_profit,
            missed_profit,
            unique_inputs,
        });
    }
    debug!("echo stats reply for chain: {}", netconfig.name);
    HttpResponse::Ok().json(stats)
}

async fn echo_search(body: Bytes, data: web::Data<AppState>) -> impl Responder {
    info!("echo search!!!");
    let strbody = match std::str::from_utf8(&body) {
        Ok(s) => s,
        Err(_) => {
            return HttpResponse::BadRequest().body("error");
        }
    };
    info!("{}", strbody);

    if strbody.is_empty() || strbody.len() != 64 || !strbody.chars().all(|c| c.is_ascii_hexdigit())
    {
        return HttpResponse::BadRequest().body("error");
    }

    let db = match data.db.lock() {
        Ok(g) => g,
        Err(_p) => {
            error!("DB mutex poisoned in echo_search");
            return HttpResponse::InternalServerError().body("error");
        }
    };
    let mut statement = match db.prepare("SELECT * FROM tbl_tx WHERE txid = ? LIMIT 1") {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to prepare statement: {}", e);
            return HttpResponse::InternalServerError().body("error");
        }
    };
    if let Err(e) = statement.bind((1, strbody)) {
        error!("Failed to bind parameter: {}", e);
        return HttpResponse::InternalServerError().body("error");
    }

    if let Ok(State::Row) = statement.next() {
        let mut response_data = HashMap::new();
        match statement.read::<String, _>("status") {
            Ok(value) => {
                response_data.insert("status", value);
            }
            Err(e) => {
                error!("Error reading status: {}", e);
            }
        }
        match statement.read::<String, _>("tx") {
            Ok(value) => {
                response_data.insert("tx", value);
            }
            Err(e) => {
                error!("Error reading tx: {}", e);
            }
        }
        match statement.read::<String, _>("our_address") {
            Ok(value) => {
                response_data.insert("our_address", value);
            }
            Err(e) => {
                error!("Error reading address: {}", e);
            }
        }
        match statement.read::<String, _>("our_fees") {
            Ok(value) => {
                response_data.insert("our_fees", value);
            }
            Err(e) => {
                error!("Error reading fees: {}", e);
            }
        }
        match statement.read::<String, _>("reqid") {
            Ok(value) => {
                response_data.insert("time", value);
            }
            Err(e) => {
                error!("Error reading reqid: {}", e);
            }
        }
        match serde_json::to_string(&response_data) {
            Ok(json_data) => {
                debug!("echo search reply: {}", json_data);
                HttpResponse::Ok().json(&response_data)
            }
            Err(_) => HttpResponse::BadRequest().body("error"),
        }
    } else {
        HttpResponse::BadRequest().body("error")
    }
}

/// Holds a transaction that has already been parsed and validated outside the DB lock.
#[derive(Clone)]
struct ParsedTx {
    txid: String,
    wtxid: String,
    ntxid: String,
    raw_hex: String, // the original line
    locktime: String,
    inputs: Vec<(String, String)>,      // (in_txid, in_vout)
    outputs: Vec<(usize, String, u64)>, // (idx, script_pubkey, amount_sat)
}

/// Parse all transactions from the request body **without** needing the DB lock.
/// Skips transactions that don't have a valid willexecutor output.
fn parse_request_transactions(
    strbody: &str,
    _req_time: i64,
    netconfig: &NetConfig,
    known_addresses: &HashSet<String>,
) -> Vec<(ParsedTx, String, u64)> {
    let mut result: Vec<(ParsedTx, String, u64)> = Vec::new();

    for line in strbody.split('\n') {
        if line.is_empty() {
            continue;
        }
        let raw_hex = line.to_string();
        let raw_tx = match Vec::<u8>::from_hex(line) {
            Ok(v) => v,
            Err(e) => {
                error!("rawtx error: {} for line {}", e, line);
                continue;
            }
        };
        if raw_tx.is_empty() {
            continue;
        }
        let tx: Transaction = match consensus::deserialize(&raw_tx) {
            Ok(t) => t,
            Err(e) => {
                error!("Deserialize error: {} for line {}", e, line);
                continue;
            }
        };

        let txid = tx.compute_txid().to_string();
        let ntxid = tx.compute_ntxid();
        let wtxid = tx.compute_wtxid();
        let locktime = tx.lock_time.to_string();

        // Collect inputs
        let mut inputs: Vec<(String, String)> = Vec::with_capacity(tx.input.len());
        for input in tx.input {
            inputs.push((
                input.previous_output.txid.to_string(),
                input.previous_output.vout.to_string(),
            ));
        }

        // Collect outputs and find which one is ours + its amount
        let mut outputs: Vec<(usize, String, u64)> = Vec::with_capacity(tx.output.len());
        let mut found = false;
        let mut our_address = String::new();
        let mut our_fees = 0u64;

        for (idx, output) in tx.output.into_iter().enumerate() {
            let script = output.script_pubkey.to_string();
            let amount = output.value.to_sat();
            outputs.push((idx, script.clone(), amount));

            let address = match bitcoin::Address::from_script(
                output.script_pubkey.as_script(),
                netconfig.network,
            ) {
                Ok(addr) => addr.to_string(),
                Err(_) => continue, // skip un-decodable outputs
            };

            let expected_ours = if netconfig.xpub {
                if known_addresses.contains(&address) {
                    address.clone()
                } else {
                    continue;
                }
            } else {
                netconfig.address.clone()
            };

            if address == expected_ours && amount >= netconfig.fixed_fee {
                our_address = expected_ours;
                our_fees = amount;
                found = true;
                trace!("address and fees are correct {}: {}", our_address, our_fees);
            }
        }

        if netconfig.fixed_fee == 0 {
            found = true;
        }

        if !found {
            trace!("willexecutor output not found for tx {}, skipping", txid);
            continue;
        }
        result.push((
            ParsedTx {
                txid,
                wtxid: wtxid.to_string(),
                ntxid: ntxid.to_string(),
                raw_hex,
                locktime,
                inputs,
                outputs,
            },
            our_address,
            our_fees,
        ));
    }

    result
}

async fn echo_push(
    body: Bytes,
    path: web::Path<String>,
    data: web::Data<AppState>,
) -> HttpResponse {
    trace!("echo_push");
    let strbody = match std::str::from_utf8(&body) {
        Ok(s) => s,
        Err(_) => {
            return HttpResponse::BadRequest().body("error");
        }
    };

    let param = path.into_inner();
    if !NETWORKS.contains(&param.as_str()) {
        return HttpResponse::NotFound().body("error");
    }
    let netconfig = data.cfg.get_net_config(&param);
    if !netconfig.enabled {
        trace!("network not enabled {}", &netconfig.name);
        return HttpResponse::BadRequest().body("error");
    }
    let req_time = match Utc::now().timestamp_nanos_opt() {
        Some(t) => t,
        None => {
            error!("Invalid timestamp");
            return HttpResponse::BadRequest().body("error");
        }
    };

    // ===== PHASE 1: parse all transactions WITHOUT the DB lock =====
    let known_addresses: HashSet<String> = {
        let db = match data.db.lock() {
            Ok(g) => g,
            Err(_p) => {
                error!("DB mutex poisoned acquiring addresses in echo_push");
                return HttpResponse::InternalServerError().body("error");
            }
        };
        if netconfig.xpub {
            match get_all_addresses_by_xpub(&db, &netconfig.address) {
                Ok(addrs) => addrs,
                Err(e) => {
                    error!("Failed to load addresses from xpub: {}", e);
                    return HttpResponse::InternalServerError().body("error");
                }
            }
        } else {
            HashSet::new()
        }
    }; // lock released here

    // Parse all transactions (CPU-bound, no DB needed)
    let parsed = parse_request_transactions(strbody, req_time, netconfig, &known_addresses);
    if parsed.is_empty() {
        return HttpResponse::BadRequest().body("error");
    }

    let all_txids: Vec<String> = parsed.iter().map(|(p, _, _)| p.txid.clone()).collect();

    // ===== PHASE 2: check duplicates in a single batch query =====
    let duplicates = {
        let db = match data.db.lock() {
            Ok(g) => g,
            Err(_p) => {
                error!("DB mutex poisoned in echo_push duplicate check");
                return HttpResponse::InternalServerError().body("error");
            }
        };
        match check_duplicate_txids(&db, &all_txids) {
            Ok(dups) => dups,
            Err(e) => {
                error!("Duplicate check failed: {}", e);
                return HttpResponse::InternalServerError().body("error");
            }
        }
    }; // lock released here

    let all_present = all_txids.iter().all(|t| duplicates.contains(t));
    if all_present {
        return HttpResponse::Ok().body("already present");
    }

    // ===== PHASE 3: build insert statements and execute (single DB lock, minimal time) =====
    {
        let db = match data.db.lock() {
            Ok(g) => g,
            Err(_p) => {
                error!("DB mutex poisoned in echo_push insert phase");
                return HttpResponse::InternalServerError().body("error");
            }
        };

        let sqltxshead = "INSERT INTO tbl_tx (txid, wtxid, ntxid, tx, locktime, reqid, network, our_address, our_fees)".to_string();
        let mut sqltxs = String::new();
        let sqlinpshead = "INSERT INTO tbl_inp (txid, in_txid, in_vout )".to_string();
        let mut sqlinps = String::new();
        let sqloutshead = "INSERT INTO tbl_out (txid, vout, script_pubkey, amount )".to_string();
        let mut sqlouts = String::new();
        let mut union_tx = true;
        let mut union_inps = true;
        let mut union_outs = true;

        let mut ptx: Vec<(usize, Value)> = vec![];
        let mut pinps: Vec<(usize, Value)> = vec![];
        let mut pouts: Vec<(usize, Value)> = vec![];
        let mut linenum = 1usize;
        let mut lineinp = 1usize;
        let mut lineout = 1usize;

        for (parsed, our_address, our_fees) in &parsed {
            if duplicates.contains(&parsed.txid) {
                continue;
            }

            if !union_tx {
                sqltxs.push_str(" UNION ALL");
            } else {
                union_tx = false;
            }
            sqltxs.push_str(" SELECT ?, ?, ?, ?, ?, ?, ?, ?, ?");
            ptx.push((linenum, Value::String(parsed.txid.clone())));
            ptx.push((linenum + 1, Value::String(parsed.wtxid.clone())));
            ptx.push((linenum + 2, Value::String(parsed.ntxid.clone())));
            ptx.push((linenum + 3, Value::String(parsed.raw_hex.clone())));
            ptx.push((linenum + 4, Value::String(parsed.locktime.clone())));
            ptx.push((linenum + 5, Value::String(req_time.to_string())));
            ptx.push((linenum + 6, Value::String(netconfig.name.clone())));
            ptx.push((linenum + 7, Value::String(our_address.clone())));
            ptx.push((linenum + 8, Value::String(our_fees.to_string())));
            linenum += 9;

            for (in_txid, in_vout) in &parsed.inputs {
                if !union_inps {
                    sqlinps.push_str(" UNION ALL");
                } else {
                    union_inps = false;
                }
                sqlinps.push_str(" SELECT ?, ?, ?");
                pinps.push((lineinp, Value::String(parsed.txid.clone())));
                pinps.push((lineinp + 1, Value::String(in_txid.clone())));
                pinps.push((lineinp + 2, Value::String(in_vout.clone())));
                lineinp += 3;
            }

            for (idx, script, amount) in &parsed.outputs {
                if !union_outs {
                    sqlouts.push_str(" UNION ALL");
                } else {
                    union_outs = false;
                }
                sqlouts.push_str(" SELECT ?, ?, ?, ?");
                pouts.push((lineout, Value::String(parsed.txid.clone())));
                pouts.push((
                    lineout + 1,
                    Value::Integer(i64::try_from(*idx).unwrap_or(-1)),
                ));
                pouts.push((lineout + 2, Value::String(script.clone())));
                pouts.push((
                    lineout + 3,
                    Value::Integer(i64::try_from(*amount).unwrap_or(0)),
                ));
                lineout += 4;
            }
        }

        if sqltxs.is_empty() {
            return HttpResponse::Ok().body("already present");
        }

        let sqltxs = format!("{}{};", sqltxshead, sqltxs);
        let sqlinps = format!("{}{};", sqlinpshead, sqlinps);
        let sqlouts = format!("{}{};", sqloutshead, sqlouts);

        if let Err(err) = execute_insert(&db, sqltxs, ptx, sqlinps, pinps, sqlouts, pouts) {
            error!("execute_insert failed: {}", err);
            return HttpResponse::BadRequest().body("error");
        }
    } // lock released

    HttpResponse::Ok().body("thx")
}

fn parse_env(data: &MyConfig) -> MyConfig {
    let mut cfg = data.clone();
    if let Ok(value) = env::var("BAL_SERVER_DB_FILE") {
        debug!("BAL_SERVER_DB_FILE: {}", value);
        cfg.db_file = value;
    }
    if let Ok(value) = env::var("BAL_SERVER_BIND_ADDRESS") {
        debug!("BAL_SERVER_BIND_ADDRESS: {}", value);
        cfg.bind_address = value;
    }
    if let Ok(value) = env::var("BAL_SERVER_BIND_PORT") {
        debug!("BAL_SERVER_BIND_PORT: {}", value);
        if let Ok(v) = value.parse::<u16>() {
            cfg.bind_port = v;
        }
    }
    if let Ok(value) = env::var("BAL_SERVER_PUB_KEY_PATH") {
        debug!("BAL_SERVER_PUB_KEY_PATH: {}", value);
        cfg.pub_key_path = value;
    }
    if let Ok(value) = env::var("BAL_SERVER_INFO") {
        debug!("BAL_SERVER_INFO: {}", value);
        cfg.info = value;
    }
    parse_env_netconfig(&mut cfg, "regtest");
    parse_env_netconfig(&mut cfg, "signet");
    parse_env_netconfig(&mut cfg, "testnet");
    parse_env_netconfig(&mut cfg, "testnet4");
    parse_env_netconfig(&mut cfg, "bitcoin");

    cfg
}

fn parse_env_netconfig(cfg: &mut MyConfig, chain: &str) {
    let c = match chain {
        "regtest" => &mut cfg.regtest,
        "signet" => &mut cfg.signet,
        "testnet" => &mut cfg.testnet,
        "testnet4" => &mut cfg.testnet4,
        _ => &mut cfg.mainnet,
    };
    if let Ok(value) = env::var(format!("BAL_SERVER_{}_ADDRESS", chain.to_uppercase())) {
        debug!("BAL_SERVER_{}_ADDRESS: {}", chain.to_uppercase(), value);
        c.address = value;
        if c.address.len() > 5 && &c.address[1..4] == "pub" {
            c.xpub = true;
            trace!("is_xpub");
        }
        c.enabled = true;
    }
    if let Ok(value) = env::var(format!("BAL_SERVER_{}_FIXED_FEE", chain.to_uppercase())) {
        debug!("BAL_SERVER_{}_FIXED_FEE: {}", chain.to_uppercase(), value);
        if let Ok(v) = value.parse::<u64>() {
            c.fixed_fee = v;
        }
    }
}

fn init_network(db: &Connection, cfg: &MyConfig) {
    for network in NETWORKS {
        let netconfig = cfg.get_net_config(network);
        insert_xpub(db, &netconfig.name.to_string(), &netconfig.address);
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init();
    let cfg = MyConfig::default();
    let actix_cfg = parse_actix_config();

    let cfg = parse_env(&cfg);
    let db = match open_db(&cfg.db_file) {
        Ok(c) => c,
        Err(e) => {
            return Err(std::io::Error::other(e));
        }
    };

    // Create database tables
    create_database(&db);

    // Initialize networks
    init_network(&db, &cfg);

    let data = web::Data::new(AppState {
        db: Mutex::new(db),
        cfg: cfg.clone(),
    });

    let bind_address = data.cfg.bind_address.clone();
    let bind_port = data.cfg.bind_port;

    // Use a single global rate limiter with the most conservative settings (1 req/sec)
    // Per-endpoint rate limiting requires advanced configuration with explicit types
    let governor_conf = GovernorConfigBuilder::const_default()
        .seconds_per_request(actix_cfg.rate_limit_pushtxs.0) // Most restrictive: 1 req/sec
        .burst_size(actix_cfg.rate_limit_pushtxs.1) // Burst: 3
        .finish()
        .unwrap();

    println!("Starting server on http://{}:{}", bind_address, bind_port);

    HttpServer::new(move || {
        App::new()
            .app_data(web::PayloadConfig::default().limit(actix_cfg.max_body_size))
            .app_data(data.clone())
            .wrap(middleware::Logger::default())
            .wrap(middleware::Compress::default())
            .wrap(Governor::new(&governor_conf))
            .service(web::resource("/").route(web::get().to(echo_home)))
            .service(web::resource("/.pub_key.pem").route(web::get().to(echo_pub_key)))
            .service(web::resource("/version").route(web::get().to(echo_version)))
            .service(web::resource("/{network}/info").route(web::get().to(echo_info)))
            .service(web::resource("/{network}/stats").route(web::get().to(echo_stats)))
            .service(web::resource("/{network}/pushtxs").route(web::post().to(echo_push)))
            .service(web::resource("/searchtx").route(web::post().to(echo_search)))
            // UMBREL packaging layer: dashboard endpoints (txlist, txdetail,
            // extended stats, backup, branding, settings). Single hook — keeps
            // the upstream router above pristine for easy future syncs.
            .configure(bal_server::umbrel_api::configure)
    })
    .workers(actix_cfg.workers)
    .max_connections(actix_cfg.max_connections)
    .bind((bind_address, bind_port))?
    .run()
    .await
}
