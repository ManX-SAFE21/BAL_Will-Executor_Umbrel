# AGENTS.md — Bitcoin After Life Will Executor (Umbrel OS edition)

This repo packages the BAL Will Executor as an **Umbrel OS app** (production: `we.safe21.io`).
It was forked/adapted from the upstream server at
`https://bitcoin-after.life/gitea/bitcoinafterlife/bal-server`.

## Repository layout

| Path | Purpose |
|---|---|
| `rust-src/` | Rust crate (workspace root for the two binaries) |
| `rust-src/src/db.rs` | Shared DB layer (`open_db` with WAL+busy_timeout, `create_database`, insert helpers) |
| `rust-src/src/xpub.rs` | xpub address derivation |
| `rust-src/src/bin/bal-server.rs` | HTTP API (Hyper): receives pre-signed txs, validates fee output, stores in SQLite |
| `rust-src/src/bin/bal-pusher.rs` | Block watcher: ZMQ `hashblock` → broadcasts matured txs, computes stats, reports to welist |
| `ui/index.html` | Dashboard (single-file, vanilla JS): tx list, confirmations, fee stats grid, settings |
| `ui/nginx.conf` | Reverse proxy: static UI + `/api/*` and `/<chain>/*` → `bal-server:9137`; `no-store` on `/zmq-status.json` |
| `docker/Dockerfile.rust` | Multi-stage build producing BOTH binaries (image `bal-umbrel-rust`) |
| `docker/Dockerfile.ui` | Nginx image with the UI |
| `scripts/` | Container entrypoints (`entrypoint-server.sh`, `entrypoint-pusher.sh`, `entrypoint-ui.sh`, `check-zmq.sh`) |
| `docker-compose.umbrel.yml` | **Production** compose on the Umbrel device (containers `bal-umbrel-*`, networks `bal-net` + `bal-v6` + `umbrel_main_network`) |
| `docker-compose.yml` | Dev compose (containers `bal-will-*`), includes a `tor` hidden service |
| `umbrel-app.yml`, `icon.svg` | Umbrel app-store packaging |

## Production deployment (Umbrel)

Device: `umbrel@umbrel.local` (SSH). App dir: `/home/umbrel/umbrel/app-data/bal-umbrel/bal-umbrel/`.
Data dir (SQLite + Ed25519 keys + backups): `/home/umbrel/umbrel/app-data/bal-umbrel/data/`.
Docker requires `sudo`. Compose commands need `--env-file`:

```bash
sudo docker compose --env-file .env -f docker-compose.yml build bal-server   # builds shared rust image
sudo docker compose --env-file .env -f docker-compose.yml up -d --force-recreate bal-server bal-pusher
sudo docker logs -f bal-umbrel-pusher
```

The pusher reuses the image built by `bal-server` (no separate build step).

**RULE: never hot-patch production without committing to git.** All fixes must be
developed/committed here first, then copied to the device. (On 16-17 Jul 2026 several
hotfixes lived only on the device and were almost lost when the local working copy
was deleted — see commit "Sync production state from Umbrel".)

## Networking notes (important)

- The pusher must join `umbrel_main_network` to reach the Bitcoin node (`APP_BITCOIN_NODE_IP`, typically `10.21.21.8`).
- The pusher also joins `bal-v6`, an **IPv6-enabled bridge** (ULA `fd00:ba1:e21::/64`, Docker NAT66).
  Reason: the welist aggregator (`welist.bitcoin-after.life`) is **unreachable over IPv4**
  from the Umbrel network (connections stall after TCP handshake); IPv6 works.
  `bal-pusher.rs::resolve_preferring_ipv6` pins the welist connection to the AAAA address.
  Do NOT remove the `bal-v6` network or the resolver pin.
- Cloudflare tunnel (`cloudflared_connector_1`) routes `we.safe21.io` → `http://bal-umbrel-ui:80`
  and must be attached to `bal-umbrel_bal-net`.

## Key environment variables

| Var | Where | Meaning |
|---|---|---|
| `BAL_SEND_STATS` | `.env` → `BAL_PUSHER_SEND_STATS` | enable signed stats report to welist (`/ping`) |
| `BAL_PUBLIC_URL` | `.env` → `BAL_SERVER_URL` | public URL sent in the welist report (must match the registered listing, `https://we.safe21.io`) |
| `BAL_BITCOIN_ADDRESS` / `BAL_BITCOIN_FEE` | `.env` → server | mainnet fee address + fixed fee (sats) |
| `BAL_LOG_LEVEL` | `.env` → `RUST_LOG` | `trace`/`debug`/`info`/`warn`/`error` |
| `WELIST_SERVER_URL` | optional | override welist base URL (default `https://welist.bitcoin-after.life`) |

## Conventions

- Two locktime modes (Bitcoin consensus): `< 500_000_000` = block height, `>=` = unix MTP.
- `tbl_tx.status`: 0=waiting, 1=sent, 2=failed/double-spent (UI shows "OTHER EXECUTORS" badge).
- SQLite is shared between server and pusher: always open it via `open_db` (WAL + busy_timeout).
- Avoid `unwrap()` in request/block-handling paths — a single bad row must never crash-loop a container.
- Version string: `<upstream-version>-umbrel.<build>` in `rust-src/Cargo.toml`
  (single source of truth — both binaries read it via `env!("CARGO_PKG_VERSION")`).
  `<upstream-version>` tracks the upstream `bal-server` release this tree is
  derived from and only changes when we sync/port a newer upstream release
  (see "Upstream sync procedure" below); it does NOT track our own feature
  work. `<build>` is an Umbrel-packaging counter, bumped for every packaging
  release that isn't an upstream sync. Mirror the same string in
  `umbrel-app.yml` (`version:`) and the README badge.

## Relationship with upstream

Upstream (`bal-server`) released **0.3.0** (17 Jul 2026): Actix rewrite, rate limiting,
shared `db.rs` (WAL), `validation.rs`, batch duplicate checks, transactional inserts,
removed `backfill_network_fees`. Our tree predates it (Hyper server, own pusher features).
Local divergences we intentionally keep: UI dashboard, confirmations/double-spend tracking,
network-fee backfill (fixed to be crash-safe, runs once per process), weekly DB backups,
ZMQ health endpoint, IPv6 welist reporting, Umbrel packaging.
When syncing from upstream, port changes file-by-file; do NOT overwrite
`ui/`, `docker/`, `scripts/`, `docker-compose*.yml`, `umbrel-app.yml`.

**Contributions sent upstream** (as `SAFE21.io`):
- PR [#1](https://bitcoin-after.life/gitea/bitcoinafterlife/bal-server/pulls/1) (19 Jul 2026,
  pending review): `BAL_PUSHER_PREFER_IPV6` env-gated IPv6 pinning for welist reports +
  `send_stats_report` error logging. Our local `resolve_preferring_ipv6` in bal-pusher.rs
  is the same logic, always-on; if the PR merges, on the next upstream sync we switch to
  the upstream implementation + `BAL_PUSHER_PREFER_IPV6=true` in the compose file.
- Fork used for contributions: `SAFE21.io/bal-server` (local clone: `Desktop/bal-server-fork`).

**Rate limiting**: applied at the nginx layer (`ui/nginx.conf`) mirroring upstream 0.3.0
Actix values — `pushtxs` 1 r/s burst 3, `searchtx` 5 r/s burst 10, keyed on
`CF-Connecting-IP` (traffic arrives via the Cloudflare tunnel). If we migrate to the
upstream Actix server, drop the nginx limits to avoid double-throttling.

## Upstream sync procedure (for future releases)

Git layout prepared for tracking upstream releases:

- `upstream` remote → `https://bitcoin-after.life/gitea/bitcoinafterlife/bal-server.git`
- `upstream-mirror` branch (on origin) → tracks `upstream/main`, never commit onto it
- upstream tags `v*` are mirrored to origin

Check for new releases:

```bash
git fetch upstream --tags
git log --oneline upstream-mirror..upstream/main   # new upstream commits
git tag -l 'v*' --sort=-v:refname | head           # latest tags
```

Update the mirror after a fetch:

```bash
git push origin upstream/main:upstream-mirror --tags
```

When adopting a new upstream version:

1. Read its CHANGELOG/release notes and the diff:
   `git diff upstream-mirror..upstream/main --stat`
2. Create a work branch from the target upstream tag, e.g.
   `git checkout -b sync/v0.3.x v0.3.0`
3. Port our Umbrel layer onto it (see "Local divergences" list above — that list IS the
   porting checklist; keep it updated whenever we add one). Port file-by-file; never
   bulk-overwrite `ui/`, `docker/`, `scripts/`, `docker-compose*.yml`, `umbrel-app.yml`.
4. Commit each ported feature as a separate commit prefixed `[umbrel]`.
5. Run the smoke test below, deploy, monitor one block cycle, then merge to `main`.

### Smoke test (run after every upstream sync / deploy)

- [ ] `docker logs bal-umbrel-server` — no panics, DB opens (WAL files exist in data/)
- [ ] `docker logs bal-umbrel-pusher` — `connected`, `blocks: N` ≈ network tip, `waiting new blocks..`
- [ ] `curl https://we.safe21.io/zmq-status.json` → `{"status":"ok"}`
- [ ] `curl https://we.safe21.io/api/bitcoin/info` → address + base_fee + version
- [ ] `curl https://we.safe21.io/api/bitcoin/stats` → fresh `report_date`, fee fields present
- [ ] Dashboard loads, fee grid shows values, tx detail modal opens (network fee shown)
- [ ] Next new block: pusher log shows `Report to welist(...) Sent: "ok"`
- [ ] `docker ps` — all 3 containers Up, no restart loops
