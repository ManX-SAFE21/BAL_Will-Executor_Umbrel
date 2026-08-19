# Bitcoin After Life Will Executor — Umbrel App

A **Will Executor (Dead Man's Switch)** for Bitcoin: store pre-signed transactions with a future locktime and broadcast them automatically when the time comes.

> **v0.3.2-umbrel.2** — Rebased onto upstream's Actix 0.3.2 core (security hardening + DoS protection) with the SAFE21 dashboard isolated in a single `umbrel_api.rs` layer for easy future syncs. Full package: web dashboard, Settings UI, Backup & Restore, weekly automatic backups, Cloudflare-ready, Umbrel App Store ready, custom branding.
>
> Version scheme: `<upstream bal-server version>-umbrel.<packaging build>`. The
> first part tracks the upstream release this code is derived from (currently
> `0.3.2` — see [AGENTS.md](AGENTS.md)); the build
> number after `-umbrel.` increments with each Umbrel-packaging release that
> doesn't correspond to a new upstream sync.

---

## What it does

1. **You** create a Bitcoin transaction with a future locktime (block height or Unix timestamp) and sign it offline using your wallet.
2. **You** submit the raw hex to this server (with a small fee output to the server's address).
3. **The server** stores it and watches every new Bitcoin block via ZMQ.
4. **When the locktime is reached**, the server broadcasts your transaction to the network.

Use cases: inheritance transfers (will), time-locked payments, dead-man's switches, recurring payments.

---

## Architecture

Three Docker containers, all sharing a persistent SQLite database:

| Container | Role | Port |
|---|---|---|
| `bal-will-server` | HTTP API (Rust/Hyper) | 9137 (internal) |
| `bal-will-pusher` | Block watcher daemon (Rust/ZMQ) | — |
| `bal-will-ui` | Web dashboard (Nginx reverse proxy) | 9140 (Umbrel) |

---

## Features

- **Web dashboard** — real-time per-network stats, transaction list, search
- **Settings panel** — change payment address, minimum fee, and server description at runtime (no rebuild needed). Danger confirmation modal protects against accidental edits.
- **Submit transactions** — paste raw hex via dashboard or HTTP API
- **Search transactions** — lookup by txid with status badges
- **Backup & Restore** — download the full SQLite database, merge backups from other instances (INSERT OR IGNORE — never overwrites existing data)
- **Weekly automatic backups** — every Sunday via VACUUM INTO, 12-week retention
- **Network fee per transaction** — with address decoding fallback for old entries
- **Tor address** display for private access
- **ZMQ auto-detection** — probes and optionally configures bitcoin.conf
- **Cloudflare Tunnel ready** — Settings button hidden on public domain
- **Electrum plugin compatible** — double-slash URL normalization in nginx
- **Systemd auto-start** — survives Umbrel OS reboots
- **Persistent data** — SQLite database on the Umbrel app-data volume

---

## Quick Start

```bash
# 1. Copy files to Umbrel
scp -r bitcoin-after-life-will-executor/ umbrel@umbrel.local:~/umbrel/home/bitcoin-after-life-will-executor/

# 2. SSH and configure
ssh umbrel@umbrel.local
cd ~/umbrel/home/bitcoin-after-life-will-executor
sudo bash scripts/configure-umbrel.sh
nano .env   # set BAL_BITCOIN_ADDRESS=bc1q... and BAL_BITCOIN_FEE=30000

# 3. Build and start
sudo docker compose up -d --build

# 4. Enable auto-start after reboot
sudo bash scripts/install-service.sh

# 5. Open dashboard
open http://umbrel.local:9140
```

Full guide: [INSTALL.md](INSTALL.md)

---

## API Reference

All endpoints accessible via the dashboard at `/api/*` and directly at `/*`:

| Method | Path | Description |
|---|---|---|
| `GET` | `/version` | Server version string |
| `GET` | `/.pub_key.pem` | Ed25519 public key (PEM) |
| `GET` | `/<net>/info` | Payment address + fee + description |
| `GET` | `/<net>/stats` | Aggregate statistics |
| `POST` | `/<net>/pushtxs` | Submit raw transactions (one hex per line) |
| `POST` | `/searchtx` | Look up a transaction by txid |
| `GET` | `/settings` | Current address + fee + description (JSON) |
| `POST` | `/settings` | Update address, fee, description |
| `GET` | `/backup` | Download full database backup (SQLite) |
| `POST` | `/merge` | Upload and merge a backup database |
| `GET` | `/zmq-status.json` | ZMQ connection status |

`<net>` is one of: `bitcoin`, `testnet`, `testnet4`, `signet`, `regtest`.

---

## Project Structure

```
bitcoin-after-life-will-executor/
├── docker-compose.yml      # 3-container service definition
├── .env.example            # Configuration template
├── umbrel-app.yml          # Umbrel App Store manifest
├── README.md               # This file
├── INSTALL.md              # Full install guide (English, with Cloudflare)
├── APP-STORE.md            # Umbrel App Store submission guide
├── CLOUDFLARE.md           # Cloudflare Tunnel setup guide
├── CHANGELOG.md            # Release history
├── icon.svg                # App icon
│
├── rust-src/               # Rust source code
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs          # Library entry
│       ├── db.rs           # SQLite layer
│       ├── xpub.rs         # BIP32 address derivation
│       └── bin/
│           ├── bal-server.rs   # HTTP API + backup/merge endpoints
│           └── bal-pusher.rs   # ZMQ block watcher + weekly backups
│
├── docker/
│   ├── Dockerfile.rust     # Multi-stage Rust build
│   ├── Dockerfile.ui       # Nginx container
│   ├── Dockerfile.tor      # Tor onion service
│   └── torrc               # Tor configuration
│
├── ui/
│   ├── index.html          # Web dashboard (vanilla JS)
│   └── nginx.conf          # Reverse proxy configuration
│
└── scripts/
    ├── entrypoint-server.sh    # Key generation + server launch
    ├── entrypoint-pusher.sh    # Credential mapping + pusher launch
    ├── entrypoint-ui.sh        # Tor address + nginx launch
    ├── check-zmq.sh            # ZMQ probe + auto-config
    ├── configure-umbrel.sh     # Auto-detect Bitcoin config
    └── install-service.sh      # Systemd auto-start installer
```

---

## License

Umbrel packaging: MIT
Rust source: original repository at https://bitcoin-after.life
