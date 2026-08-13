/// Database access layer for Bitcoin After Life.
///
/// All SQLite operations go through this module.
/// Tables:
///   tbl_tx      — pre-signed transactions waiting to be broadcast
///   tbl_inp     — inputs of each stored transaction (dedup detection)
///   tbl_out     — outputs of each stored transaction (fee verification)
///   tbl_xpub    — extended public keys registered per network
///   tbl_address — derived addresses mapped to remote IPs (for xpub mode)
///   tbl_stats   — per-network aggregate statistics updated on each block
use log::{error, info, trace};
use sqlite::{Connection, Error, State, Value};

/// Open the SQLite database with sane concurrency settings.
///
/// Both bal-server and bal-pusher access the same file. Without WAL and a
/// busy timeout, concurrent writers can hit "database is locked" errors.
/// WAL lets readers and one writer proceed concurrently; busy_timeout makes
/// a contender wait instead of failing immediately; synchronous=NORMAL is
/// the recommended companion for WAL (still crash-safe).
/// (Ported from upstream bal-server 0.3.0 src/db.rs::open_db.)
pub fn open_db(path: &str) -> Result<Connection, Error> {
    let conn = sqlite::open(path)?;
    // busy_timeout BEFORE journal_mode: the WAL switch itself may contend.
    let _ = conn.execute("PRAGMA busy_timeout = 5000;");
    let _ = conn.execute("PRAGMA journal_mode = WAL;");
    let _ = conn.execute("PRAGMA synchronous = NORMAL;");
    Ok(conn)
}

/// Create all database tables if they do not already exist.
///
/// This function is idempotent — safe to call on every startup.
/// ALTER TABLE statements silently fail if the column already exists,
/// which is intentional for forward-compatible schema migrations.
pub fn create_database(db: &Connection) {
    info!("database sanity check");

    // --- Main transaction table ---
    // status: 0=waiting, 1=sent, 2=failed
    let _ = db.execute(
        "CREATE TABLE IF NOT EXISTS tbl_tx (
            txid PRIMARY KEY,
            date_creation TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            date_update   TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            wtxid, ntxid, tx,
            locktime      integer,
            network,
            network_fees,
            reqid,
            our_fees,
            our_address,
            status        integer DEFAULT 0
        );",
    );

    // Add push_err column for storing broadcast error messages.
    // This ALTER TABLE will silently fail if the column already exists —
    // that is intentional and matches the original code's migration strategy.
    let _ = db.execute("ALTER TABLE tbl_tx ADD COLUMN push_err TEXT");

    // Ensure network_fees column exists (legacy databases may lack it)
    let _ = db.execute("ALTER TABLE tbl_tx ADD COLUMN network_fees TEXT");

    // Add confirmations column for blockchain confirmation tracking.
    // Updated by bal-pusher on each new block via getrawtransaction RPC.
    let _ = db.execute("ALTER TABLE tbl_tx ADD COLUMN confirmations INTEGER NOT NULL DEFAULT 0");

    // --- Transaction inputs (used to detect double submissions) ---
    let _ = db.execute("CREATE TABLE IF NOT EXISTS tbl_inp(id, txid, in_txid, in_vout);");
    let _ = db.execute("CREATE UNIQUE INDEX IF NOT EXISTS idx_inp ON tbl_inp(txid,in_txid,in_vout);");

    // --- Transaction outputs (used to verify fee payment) ---
    let _ = db.execute(
        "CREATE TABLE IF NOT EXISTS tbl_out(id, txid, script_pubkey, amount, vout);",
    );
    let _ = db.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_out ON tbl_out(txid, script_pubkey, amount, vout);",
    );
    let _ = db.execute("ALTER TABLE tbl_out ADD COLUMN address TEXT");

    // --- Extended public keys (xpub mode) ---
    let _ = db.execute(
        "CREATE TABLE IF NOT EXISTS tbl_xpub (
            id          INTEGER PRIMARY KEY,
            network     TEXT,
            xpub        TEXT,
            date_create TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            path_idx    INTEGER DEFAULT -1
        );",
    );
    let _ = db.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_xpub ON tbl_xpub (network, xpub)",
    );

    // --- Derived addresses (maps IP → address for xpub mode) ---
    let _ = db.execute(
        "CREATE TABLE IF NOT EXISTS tbl_address (
            address        TEXT PRIMARY_KEY,
            path           TEXT NOT NULL,
            date_create    TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            xpub           INTEGER,
            remote_address TEXT
        );",
    );

    // --- Statistics table ---
    // BUG FIX: The original code uses this table in bal-pusher's
    // calculate_stats() but never creates it. Added here so the INSERT
    // in calculate_stats succeeds from the very first block.
    // The UNIQUE constraint on 'chain' is required by ON CONFLICT(chain).
    let _ = db.execute(
        "CREATE TABLE IF NOT EXISTS tbl_stats (
            chain           TEXT UNIQUE,
            report_date     TEXT,
            totals          INTEGER DEFAULT 0,
            waiting         INTEGER DEFAULT 0,
            sent            INTEGER DEFAULT 0,
            failed          INTEGER DEFAULT 0,
            waiting_profit  INTEGER DEFAULT 0,
            sent_profit     INTEGER DEFAULT 0,
            missed_profit   INTEGER DEFAULT 0,
            unique_inputs   INTEGER DEFAULT 0
        );",
    );

    // Data migration: rename legacy 'mainnet' network label to 'bitcoin'
    // to match the Bitcoin library's canonical name.
    let _ = db.execute("UPDATE tbl_tx set network='bitcoin' where network='mainnet');");
}

/// Insert or ignore an xpub entry for a given network.
///
/// Called on startup for every configured network.
/// If the xpub is empty (network not configured), does nothing.
pub fn insert_xpub(db: &Connection, network: &String, xpub: &String) {
    if xpub != "" {
        trace!("going to insert: {} xpub:{}", network, xpub);
        let mut stmt = db
            .prepare("INSERT INTO tbl_xpub(network,xpub) VALUES(?, ?);")
            .unwrap();
        let _ = stmt.bind((1, Value::String(network.to_string()))).unwrap();
        let _ = stmt.bind((2, Value::String(xpub.to_string()))).unwrap();
        let _ = stmt.next();
    }
}

/// Find the most recently assigned address for a given (network, xpub, client IP) tuple.
///
/// In xpub mode, the same client IP should always get the same address
/// (until a new session is started). This prevents address reuse tracking.
pub fn get_last_used_address_by_ip(
    db: &Connection,
    network: &String,
    xpub: &String,
    address: &String,
) -> Option<String> {
    let mut stmt = db.prepare(
        "SELECT tbl_address.address
         FROM tbl_xpub
         JOIN tbl_address ON (tbl_xpub.id = tbl_address.xpub)
         WHERE tbl_xpub.network = ?
           AND tbl_address.remote_address = ?
           AND tbl_xpub.xpub = ?
         ORDER BY tbl_address.date_create DESC
         LIMIT 1;"
    ).unwrap();
    let _ = stmt.bind((1, Value::String(network.to_string())));
    let _ = stmt.bind((2, Value::String(address.to_string())));
    let _ = stmt.bind((3, Value::String(xpub.to_string())));
    if let Ok(State::Row) = stmt.next() {
        let address = stmt.read::<String, _>("address").unwrap();
        return Some(address);
    } else {
        return None;
    }
}

/// Atomically increment the derivation path index for an xpub and return
/// the new (xpub_row_id, path_index) pair.
///
/// Uses a RETURNING clause (SQLite 3.35+) to get the new value atomically,
/// avoiding a separate SELECT after UPDATE.
pub fn get_next_address_index(db: &Connection, network: &String, xpub: &String) -> (i64, i64) {
    let mut stmt = db
        .prepare(
            "UPDATE tbl_xpub
             SET path_idx = path_idx + 1
             WHERE network = ? AND xpub = ?
             RETURNING path_idx, id;",
        )
        .unwrap();
    stmt.bind((1, Value::String(network.to_string()))).unwrap();
    stmt.bind((2, Value::String(xpub.to_string()))).unwrap();
    match stmt.next() {
        Ok(State::Row) => {
            let next = stmt.read::<i64, _>("path_idx").unwrap();
            let id = stmt.read::<i64, _>("id").unwrap();
            return (id, next);
        }
        Err(_) => {
            return (0, 0);
        }
        Ok(State::Done) => {
            return (0, 0);
        }
    };
}

/// Persist a newly derived address along with its derivation path and
/// the client IP address it was assigned to.
pub fn save_new_address(
    db: &Connection,
    xpub: i64,
    address: &String,
    path: &String,
    remote_addr: &String,
) {
    let mut stmt = db
        .prepare(
            "INSERT INTO tbl_address(address, path, xpub, remote_address)
             VALUES(?, ?, ?, ?);",
        )
        .unwrap();

    stmt.bind((1, Value::String(address.to_string()))).unwrap();
    stmt.bind((2, Value::String(path.to_string()))).unwrap();
    stmt.bind((3, Value::Integer(xpub))).unwrap();
    stmt.bind((4, Value::String(remote_addr.to_string())))
        .unwrap();

    let _ = stmt.next();
}

/// Execute a batch INSERT for a transaction and all its inputs/outputs
/// inside a single SQLite transaction for atomicity.
///
/// If any sub-insert fails, the whole operation is rolled back.
/// Returns Ok(()) on success or Err(sqlite::Error) on failure.
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

    // Insert transaction row
    let mut stmt = db
        .prepare(sqltxs.as_str())
        .expect("failed to prepare sqltxs");
    if let Err(err) = stmt.bind::<&[(_, Value)]>(&ptx[..]) {
        error!("error binding transaction parameters: {}", err);
        let _ = db.execute("ROLLBACK");
        return Err(err);
    }
    if let Err(err) = stmt.next() {
        error!("error inserting transactions {}", err);
        let _ = db.execute("ROLLBACK");
    } else {
        // Insert input rows
        let mut stmt = db
            .prepare(sqlinp.as_str())
            .expect("failed to prepare sqlinp");
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
            // Insert output rows
            let mut stmt = db
                .prepare(sqlout.as_str())
                .expect("failed to prepare sqlout");
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

/// Count all transactions for a given network. Used for reporting.
pub fn get_total_transaction_number(db: Connection, network: &String) -> Result<i64, Error> {
    let mut stmt = db
        .prepare("SELECT COUNT(*) as total_number FROM tbl_tx where network = ?;")
        .unwrap();
    stmt.bind((1, Value::String(network.to_string()))).unwrap();
    match stmt.next() {
        Ok(State::Row) => Ok(stmt.read::<i64, _>("total_number").unwrap()),
        Ok(sqlite::State::Done) => todo!(),
        Err(err) => Err(err),
    }
}
