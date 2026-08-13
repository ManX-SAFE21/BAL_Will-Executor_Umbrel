//! # bal_server — Bitcoin After Life Server Library
//!
//! This crate contains shared modules used by both binary targets:
//!   - `bal-server`  (HTTP API server)
//!   - `bal-pusher`  (block watcher and transaction broadcaster)
//!
//! Both binaries import from this library using the package name, e.g.:
//!   `use bal_server::db::create_database;`
//!   `use bal_server::xpub::new_address_from_xpub;`

/// Database operations: create schema, insert/query transactions and addresses.
/// Uses the `sqlite` crate (binds to system libsqlite3).
pub mod db;

/// Extended public key (xpub) utilities: BIP32 derivation, address generation,
/// descriptor construction. Supports xpub/ypub/zpub/tpub/vpub/upub prefixes.
pub mod xpub;
