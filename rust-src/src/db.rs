use log::{error, info, trace, warn};
use sqlite::{Connection, Error, State, Value};
use std::collections::HashSet;
use std::path::Path;
use std::thread;
use std::time::Duration;

/// Check which txids are already present in the database in a single batch query.
/// Returns a HashSet of txids that already exist (duplicates).
/// This is O(1) per query regardless of the number of txids, replacing the N+1 pattern.
pub fn check_duplicate_txids(db: &Connection, txids: &[String]) -> Result<HashSet<String>, Error> {
    if txids.is_empty() {
        return Ok(HashSet::new());
    }

    // Build a single query with all txids using IN clause placeholders
    // SQLite supports up to 1000 parameters per statement, so we chunk for safety
    let mut duplicates = HashSet::new();
    let chunk_size = 500; // Safe chunk size for SQLite parameters

    for chunk in txids.chunks(chunk_size) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("SELECT txid FROM tbl_tx WHERE txid IN ({})", placeholders);
        let mut stmt = db.prepare(sql)?;

        for (i, txid) in chunk.iter().enumerate() {
            stmt.bind((i + 1, Value::String(txid.clone())))?;
        }

        while let Ok(State::Row) = stmt.next() {
            if let Ok(txid) = stmt.read::<String, _>("txid") {
                duplicates.insert(txid);
            }
        }
    }

    Ok(duplicates)
}

/// Validates and opens the SQLite database, enforcing security best practices:
/// - Path must not contain `..` (directory traversal).
/// - Absolute paths must not target known system directories.
/// - If the file exists, it must be a regular file (not a symlink or device).
/// - WAL journal mode is enabled for safe concurrent access.
/// - Synchronous is set to NORMAL for performance with safety.
///
/// Returns `Err` on validation failure or open error to prevent panics.
pub fn open_db(path: &str) -> Result<Connection, String> {
    let p = Path::new(path);

    // Prevent directory traversal
    for component in p.components() {
        if component == std::path::Component::ParentDir {
            return Err("Database path may not contain '..'".to_string());
        }
    }

    // If absolute, block known sensitive system directories
    if p.is_absolute() {
        let path_str = p.to_str().unwrap_or("");
        let forbidden = [
            "/etc", "/proc", "/sys", "/dev", "/usr", "/bin", "/sbin", "/lib", "/opt",
        ];
        for prefix in &forbidden {
            if path_str.starts_with(prefix) {
                return Err(format!(
                    "Absolute database path under {} is forbidden",
                    prefix
                ));
            }
        }
    }

    // If file exists, must be a regular file (not a symlink, device, etc.)
    if p.exists() {
        if p.is_symlink() {
            return Err("Database path must not be a symlink".to_string());
        }
        let metadata = std::fs::metadata(p)
            .map_err(|e| format!("Cannot access database file metadata: {}", e))?;
        if !metadata.is_file() {
            return Err(
                "Database path must point to a regular file, not a directory or device".to_string(),
            );
        }
    }

    let conn = sqlite::open(path).map_err(|e| format!("Failed to open SQLite database: {}", e))?;

    // Set busy timeout BEFORE WAL mode so SQLite waits instead of failing immediately.
    // This handles the race where two processes (server + pusher) open the same DB
    // and both try to enable WAL mode concurrently.
    conn.execute("PRAGMA busy_timeout = 5000;")
        .map_err(|e| format!("Failed to set busy_timeout: {}", e))?;

    // Retry WAL mode up to 5 times (handles concurrent open from bal-pusher).
    let mut wal_ok = false;
    for attempt in 0..5 {
        match conn.execute("PRAGMA journal_mode = WAL;") {
            Ok(_) => {
                wal_ok = true;
                break;
            }
            Err(e) => {
                warn!(
                    "WAL mode attempt {}/5 failed: {}, retrying in 100ms...",
                    attempt + 1,
                    e
                );
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
    if !wal_ok {
        // WAL might already be enabled by another process; this is not fatal.
        warn!("Could not set WAL mode after retries — may already be enabled by another process");
    }

    conn.execute("PRAGMA synchronous = NORMAL;")
        .map_err(|e| format!("Failed to set synchronous NORMAL: {}", e))?;

    Ok(conn)
}

/// Loads all known addresses for a given xpub into a HashSet for fast
/// in-memory lookup during transaction validation (replaces N+1 query).
pub fn get_all_addresses_by_xpub(db: &Connection, xpub: &str) -> Result<HashSet<String>, Error> {
    let mut stmt = db.prepare(
        "SELECT a.address FROM tbl_address a JOIN tbl_xpub x ON a.xpub = x.id WHERE x.xpub = ?",
    )?;
    stmt.bind((1, Value::String(xpub.to_string())))?;
    let mut addresses = HashSet::new();
    while let Ok(State::Row) = stmt.next() {
        match stmt.read::<String, _>("address") {
            Ok(addr) => {
                addresses.insert(addr);
            }
            Err(e) => {
                error!("Failed to read address column: {}", e);
            }
        }
    }
    Ok(addresses)
}

pub fn create_database(db: &Connection) {
    info!("database sanity check");
    let _ = db.execute("CREATE TABLE IF NOT EXISTS tbl_tx      (txid PRIMARY KEY, date_creation TIMESTAMP DEFAULT CURRENT_TIMESTAMP, date_update TIMESTAMP DEFAULT CURRENT_TIMESTAMP, wtxid, ntxid, tx, locktime integer, network, network_fees, reqid, our_fees, our_address, status integer DEFAULT 0);");
    let _ = db.execute("ALTER TABLE tbl_tx ADD COLUMN push_err TEXT");
    // UMBREL: blockchain confirmation tracking used by the dashboard state model
    // (WAITING / IN MEMPOOL / CONFIRMED / DONE ELSEWHERE). Written by bal-pusher,
    // read by umbrel_api. Silently no-ops if the column already exists.
    let _ = db.execute("ALTER TABLE tbl_tx ADD COLUMN confirmations INTEGER NOT NULL DEFAULT 0");

    let _ = db.execute("CREATE TABLE IF NOT EXISTS tbl_inp(id, txid, in_txid, in_vout);");
    let _ = db.execute("CREATE UNIQUE INDEX ON tbl_inp(txid,in_txid,in_vout);");

    let _ =
        db.execute("CREATE TABLE IF NOT EXISTS tbl_out(id, txid, script_pubkey, amount, vout);");
    let _ = db.execute("CREATE UNIQUE INDEX ON tbl_out(txid, script_pubkey, amount, vout);");

    let _ = db.execute("CREATE TABLE IF NOT EXISTS tbl_xpub (id INTEGER PRIMARY KEY , network TEXT, xpub TEXT, date_create TIMESTAMP DEFAULT CURRENT_TIMESTAMP,path_idx INTEGER DEFAULT -1);");
    let _ = db.execute("CREATE UNIQUE INDEX idx_xpub ON tbl_xpub (network, xpub)");
    let _ = db.execute("CREATE TABLE IF NOT EXISTS tbl_address (address TEXT PRIMARY_KEY, path TEXT NOT NULL, date_create TIMESTAMP DEFAULT CURRENT_TIMESTAMP, xpub INTEGER,remote_address TEXT);");

    let _ = db.execute("CREATE TABLE IF NOT EXISTS tbl_stats (report_date TEXT, chain TEXT, totals INTEGER, waiting INTEGER, sent INTEGER, failed INTEGER, waiting_profit INTEGER, sent_profit INTEGER, missed_profit INTEGER, unique_inputs INTEGER);");
    // UNIQUE index required for ON CONFLICT(chain) DO UPDATE in calculate_stats
    let _ = db.execute("DROP INDEX IF EXISTS idx_stats_chain;");
    let _ = db.execute("CREATE UNIQUE INDEX IF NOT EXISTS idx_stats_chain ON tbl_stats(chain);");

    let _ = db.execute("UPDATE tbl_tx set network='bitcoin' where network='mainnet';");
}
/*
 pub fn get_xpub_id(db: &Connection, network: &String, xpub: &String) -> Option<i64>{
    let mut stmt = db.prepare("SELECT * FROM tbl_xpub where network = ? and xpub = ?;").unwrap();
    let _ = stmt.bind((1,Value::String(network.to_string()))).unwrap();
    let _ = stmt.bind((2,Value::String(xpub.to_string()))).unwrap();
    if let  Ok(State::Row) = stmt.next(){
        return Some(stmt.read::<i64, _>("id").unwrap());
    } else {
        return None;
    }
}
*/
pub fn insert_xpub(db: &Connection, network: &str, xpub: &str) {
    if !xpub.is_empty() {
        trace!("going to insert: {} xpub:{}", network, xpub);
        let mut stmt =
            match db.prepare("INSERT OR IGNORE INTO tbl_xpub(network,xpub) VALUES(?, ?);") {
                Ok(s) => s,
                Err(e) => {
                    error!("Failed to prepare xpub insert statement: {}", e);
                    return;
                }
            };
        if let Err(e) = stmt.bind((1, Value::String(network.to_string()))) {
            error!("Failed to bind network parameter for xpub insert: {}", e);
            return;
        }
        if let Err(e) = stmt.bind((2, Value::String(xpub.to_string()))) {
            error!("Failed to bind xpub parameter: {}", e);
            return;
        }
        if let Err(e) = stmt.next() {
            error!("Failed to insert xpub: {}", e);
        }
    }
}

pub fn get_last_used_address_by_ip(
    db: &Connection,
    network: &String,
    xpub: &String,
    address: &String,
) -> Option<String> {
    let mut stmt = match db.prepare("SELECT tbl_address.address FROM tbl_xpub join tbl_address on(tbl_xpub.id = tbl_address.xpub) where tbl_xpub.network = ? and tbl_address.remote_address = ? and tbl_xpub.xpub = ? ORDER BY tbl_address.date_create DESC LIMIT 1;") {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to prepare address query: {}", e);
            return None;
        }
    };
    if let Err(e) = stmt.bind((1, Value::String(network.to_string()))) {
        error!("Failed to bind network parameter: {}", e);
        return None;
    }
    if let Err(e) = stmt.bind((2, Value::String(address.to_string()))) {
        error!("Failed to bind address parameter: {}", e);
        return None;
    }
    if let Err(e) = stmt.bind((3, Value::String(xpub.to_string()))) {
        error!("Failed to bind xpub parameter: {}", e);
        return None;
    }
    if let Ok(State::Row) = stmt.next() {
        match stmt.read::<String, _>("address") {
            Ok(addr) => Some(addr),
            Err(e) => {
                error!("Failed to read address column: {}", e);
                None
            }
        }
    } else {
        None
    }
}
pub fn get_next_address_index(db: &Connection, network: &String, xpub: &String) -> (i64, i64) {
    let mut stmt = match db.prepare("UPDATE tbl_xpub SET path_idx = path_idx + 1 WHERE network = ? and xpub= ?  RETURNING path_idx,id;") {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to prepare xpub index update: {}", e);
            return (0, 0);
        }
    };
    if let Err(e) = stmt.bind((1, Value::String(network.to_string()))) {
        error!("Failed to bind network parameter: {}", e);
        return (0, 0);
    }
    if let Err(e) = stmt.bind((2, Value::String(xpub.to_string()))) {
        error!("Failed to bind xpub parameter: {}", e);
        return (0, 0);
    }
    match stmt.next() {
        Ok(State::Row) => match stmt.read::<i64, _>("path_idx") {
            Ok(next) => match stmt.read::<i64, _>("id") {
                Ok(id) => (id, next),
                Err(e) => {
                    error!("Failed to read id column: {}", e);
                    (0, 0)
                }
            },
            Err(e) => {
                error!("Failed to read path_idx column: {}", e);
                (0, 0)
            }
        },
        Err(e) => {
            error!("Failed to execute xpub index update: {}", e);
            (0, 0)
        }
        Ok(State::Done) => (0, 0),
    }
}
pub fn save_new_address(
    db: &Connection,
    xpub: i64,
    address: &String,
    path: &String,
    remote_addr: &String,
) {
    let mut stmt = match db.prepare(
        "INSERT INTO tbl_address(address,path,xpub,remote_address) VALUES(?,?,?,?);
",
    ) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to prepare address insert statement: {}", e);
            return;
        }
    };

    if let Err(e) = stmt.bind((1, Value::String(address.to_string()))) {
        error!("Failed to bind address parameter: {}", e);
        return;
    }
    if let Err(e) = stmt.bind((2, Value::String(path.to_string()))) {
        error!("Failed to bind path parameter: {}", e);
        return;
    }
    if let Err(e) = stmt.bind((3, Value::Integer(xpub))) {
        error!("Failed to bind xpub parameter: {}", e);
        return;
    }
    if let Err(e) = stmt.bind((4, Value::String(remote_addr.to_string()))) {
        error!("Failed to bind remote_addr parameter: {}", e);
        return;
    }

    if let Err(e) = stmt.next() {
        error!("Failed to insert address: {}", e);
    }
}
pub fn execute_insert(
    db: &Connection,
    sqltxs: String,
    ptx: Vec<(usize, Value)>,
    sqlinp: String,
    pinp: Vec<(usize, Value)>,
    sqlout: String,
    pout: Vec<(usize, Value)>,
) -> Result<(), Error> {
    let _ = db.execute("BEGIN TRANSACTION");
    let mut stmt = match db.prepare(sqltxs.as_str()) {
        Ok(s) => s,
        Err(err) => {
            error!("error preparing sqltxs: {}", err);
            let _ = db.execute("ROLLBACK");
            return Err(err);
        }
    };
    if let Err(err) = stmt.bind::<&[(_, Value)]>(&ptx[..]) {
        error!("error binding transaction parameters: {}", err);
        let _ = db.execute("ROLLBACK");
        return Err(err);
    }
    if let Err(err) = stmt.next() {
        error!("error inserting transactions {}", err);
        let _ = db.execute("ROLLBACK");
    } else {
        let mut stmt = match db.prepare(sqlinp.as_str()) {
            Ok(s) => s,
            Err(err) => {
                error!("error preparing sqlinp: {}", err);
                let _ = db.execute("ROLLBACK");
                return Err(err);
            }
        };
        if let Err(err) = stmt.bind::<&[(_, Value)]>(&pinp[..]) {
            error!("error binding inputs parameters {}", err);
            let _ = db.execute("ROLLBACK");
            return Err(err);
        }
        if let Err(err) = stmt.next() {
            error!("error inserting inputs {}", err);
            let _ = db.execute("ROLLBACK");
            return Err(err);
        } else {
            let mut stmt = match db.prepare(sqlout.as_str()) {
                Ok(s) => s,
                Err(err) => {
                    error!("error preparing sqlout: {}", err);
                    let _ = db.execute("ROLLBACK");
                    return Err(err);
                }
            };
            if let Err(err) = stmt.bind::<&[(_, Value)]>(&pout[..]) {
                error!("error binding outs parameters {}", err);
                let _ = db.execute("ROLLBACK");
                return Err(err);
            }
            if let Err(err) = stmt.next() {
                error!("error inserting outs {}", err);
                let _ = db.execute("ROLLBACK");
                return Err(err);
            }
        }
    }
    let _ = db.execute("COMMIT");
    Ok(())
}
pub fn get_total_transaction_number(db: Connection, network: &String) -> Result<i64, Error> {
    let mut stmt = db
        .prepare("SELECT COUNT(*) as total_number FROM tbl_tx where network = ?;")
        .map_err(|e| {
            error!("Failed to prepare statement: {}", e);
            e
        })?;
    if let Err(e) = stmt.bind((1, Value::String(network.to_string()))) {
        error!("Failed to bind network parameter: {}", e);
        return Err(e);
    }
    match stmt.next() {
        Ok(State::Row) => match stmt.read::<i64, _>("total_number") {
            Ok(val) => Ok(val),
            Err(e) => {
                error!("Failed to read total_number column: {}", e);
                Err(e)
            }
        },
        Ok(sqlite::State::Done) => Ok(0),
        Err(err) => {
            error!("Failed to execute query: {}", err);
            Err(err)
        }
    }
}
