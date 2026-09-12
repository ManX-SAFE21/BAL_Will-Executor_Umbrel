# AGENTS.md — Bitcoin After Life Will Executor (Umbrel OS edition)

This repo packages the BAL Will Executor as an **Umbrel OS app** (production: `we.safe21.io`).
It was forked/adapted from the upstream server at
`https://bitcoin-after.life/gitea/bitcoinafterlife/bal-server`.

## Repository layout

| Path | Purpose |
|---|---|
| `rust-src/` | Rust crate (workspace root for the two binaries) |
| `rust-src/src/db.rs` | **Vanilla upstream** DB layer (`open_db` WAL+busy_timeout, `create_database`, insert helpers). *One* Umbrel line: the `confirmations` column (marked `UMBREL:`). |
| `rust-src/src/validation.rs`, `xpub.rs`, `lib.rs` | **Vanilla upstream** (`lib.rs` has one extra line: `pub mod umbrel_api`). |
| `rust-src/src/bin/bal-server.rs` | **Vanilla upstream** HTTP API (Actix Web): pushtxs/searchtx/info/stats + DoS protection. *One* Umbrel line in `main()`: `.configure(umbrel_api::configure)`. |
| `rust-src/src/umbrel_api.rs` | **Umbrel layer** — all dashboard endpoints (txlist, txdetail, extended stats, backup/restore/merge, settings, branding). Self-contained Actix handlers; the ONLY file that carries our server-side additions. |
| `rust-src/src/bin/bal-pusher.rs` | **Vanilla upstream** block watcher + one clearly-delimited Umbrel diff block (`===== UMBREL confirmation tracking =====`). |
| `ui/index.html` | Dashboard (single-file, vanilla JS): tx list, confirmations, fee stats grid, settings |
| `ui/nginx.conf` | Reverse proxy: static UI + `/api/*` and `/<chain>/*` → `bal-server:9137`; `no-store` on `/zmq-status.json` |
| `docker/Dockerfile.rust` | Multi-stage build producing BOTH binaries (image `bal-umbrel-rust`) |
| `docker/Dockerfile.ui` | Nginx image with the UI |
| `scripts/` | Container entrypoints (`entrypoint-server.sh`, `entrypoint-pusher.sh`, `entrypoint-ui.sh`, `check-zmq.sh`) |
| `docker-compose.umbrel.yml` | **Production** compose on the Umbrel device (containers `bal-umbrel-*`, networks `bal-net` + `bal-v6` + `umbrel_main_network`) |
| `docker-compose.yml` | Dev compose (containers `bal-will-*`), includes a `tor` hidden service |
| `umbrel-app.yml`, `icon.svg` | Umbrel app-store packaging |

## Production deployment (Umbrel)

Device: `umbrel@umbrel.local` (SSH, key `~/.ssh/id_ed25519_umbrel_claude`).
App dir: `/home/umbrel/umbrel/app-data/bal-umbrel/bal-umbrel/`.
Data dir (SQLite + Ed25519 keys + backups): `/home/umbrel/umbrel/app-data/bal-umbrel/data/`.
Docker requires `sudo`. The pusher reuses the image built by `bal-server` (no separate build).

**Normal deploy** = the scripted, backed-up flow (see "Deploy" under the sync procedure):
stage `staged-src.tar.gz` to `/home/umbrel/`, then `sudo bash deploy-update.sh` on the box.
It rebuilds both images, recreates the 3 containers, smoke-tests, and prints a rollback
timestamp. Manual one-offs still work:

```bash
sudo docker compose --env-file .env -f docker-compose.yml build bal-server bal-ui
sudo docker compose --env-file .env -f docker-compose.yml up -d --force-recreate bal-server bal-pusher bal-ui
sudo docker logs -f bal-umbrel-pusher
```

**RULE: never hot-patch production without committing to git.** All fixes are
developed/committed here first, then deployed. (On 16-17 Jul 2026 several hotfixes lived
only on the device and were almost lost when the local working copy was deleted.)

**Boot resilience — `bal-umbrel.service`:** this app is deployed manually, NOT through
Umbrel's official App Store install flow, so it is not in `umbreld`'s own app registry.
On 12 Sep 2026, a device reboot left every officially-installed app running fine, but the
`bal-umbrel-*` containers were fully removed (not just stopped) and never recreated —
`umbreld` only reconciles apps it knows about; Docker's own `restart: unless-stopped`
wasn't enough because the containers didn't merely stop, they were gone.
Fix: `sudo bash scripts/install-service.sh` (run from the real app directory — it derives
`APP_DIR` from its own path, so running a copy staged elsewhere, e.g. `/home/umbrel/`,
silently writes the wrong `WorkingDirectory` and the unit fails) installs
`/etc/systemd/system/bal-umbrel.service`, which runs
`docker compose --env-file .env -f docker-compose.yml up -d` after `umbrel.service` +
`docker.service` on every boot — independent of `umbreld`. Verify after any reinstall:
`systemctl is-enabled bal-umbrel.service` → `enabled`. This should already be installed;
if a future reboot causes another full outage, check this service first
(`systemctl status bal-umbrel.service`, `journalctl -xeu bal-umbrel.service`) before
assuming a code regression.

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
- `tbl_tx.status`: 0=waiting, 1=sent(broadcast), 2=failed. `confirmations`: -1 = evicted from
  mempool. The dashboard collapses these into one **State**: WAITING / IN MEMPOOL / CONFIRMED /
  DONE ELSEWHERE (every status=2 — live data shows they are all "won by a competing executor",
  not genuine rejections). Logic lives in `resolveState()` in `ui/index.html`.
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

## Architecture: vanilla upstream + isolated Umbrel layer

Since **0.3.2-umbrel** the Rust tree tracks upstream's **Actix** architecture. (Before
that we were on a forked Hyper server that had diverged heavily; the 0.3.2 sync re-based us
onto upstream with a clean isolation boundary so future syncs are cheap.)

Design goal: **trivial future syncs.** Upstream `src/` files stay as close to vanilla as
possible; everything Umbrel-specific lives in ONE new file plus a handful of marked lines.

**The complete list of Umbrel touch-points in the Rust tree — this IS the sync checklist:**

| File | Umbrel change | Marker |
|---|---|---|
| `src/umbrel_api.rs` | entire file — all dashboard endpoints | (new file) |
| `src/lib.rs` | `#[cfg(feature="server")] pub mod umbrel_api;` | comment above it |
| `src/bin/bal-server.rs` | `.configure(bal_server::umbrel_api::configure)` in `main()` | `UMBREL packaging layer` |
| `src/db.rs` | `ALTER TABLE tbl_tx ADD COLUMN confirmations …` | `UMBREL:` |
| `src/bin/bal-pusher.rs` | confirmation-tracking block in `main_result()` | `===== UMBREL confirmation tracking =====` |
| `Cargo.toml` | `version = "<upstream>-umbrel.<build>"` | — |

Nothing else in `src/` diverges. The UI (`ui/`) and packaging (`docker/`, `scripts/`,
compose, `umbrel-app.yml`) are separate layers that never touch upstream code.

**How the layer plugs in:** `umbrel_api::configure(cfg)` registers our routes under
non-colliding paths (`/txlist`, `/txdetail`, `/txstats/{net}`, `/backup*`, `/merge`,
`/restore/{f}`, `/settings`, `/*-logo`). Handlers are **self-contained**: they read config
from env (`BAL_SERVER_DB_FILE`) and open their own SQLite connection per request, so they
never depend on the binary's private `AppState`. The dashboard reaches them via nginx `/api/*`.
The extended stats live at `/txstats/{net}` (not upstream's `/{net}/stats`) so the upstream
router is untouched — the UI calls `/api/txstats/<net>`.

**Settings flow:** the UI writes `settings.json` (next to the DB) via `POST /settings`.
Upstream config is immutable at runtime, so `entrypoint-server.sh` translates
`settings.json` → `BAL_SERVER_BITCOIN_ADDRESS/FIXED_FEE/INFO` at startup (needs `jq`; the
server auto-enables a network when its `_ADDRESS` env is set). A settings change therefore
applies **on the next container restart**.

**Rate limiting:** upstream's Actix global limiter keys on peer IP; behind nginx every
request shares one bucket, so the strict default (1 r/s) starves the dashboard's load burst.
`entrypoint-server.sh` raises it (`BAL_SERVER_ACTIX_PUSHTXS_PER_SEC=50`, `BURST=100`). Keep
the nginx limits on `pushtxs`/`searchtx` (they see the real `CF-Connecting-IP`) as the
primary DoS control; the Actix limiter is secondary.

**Contributions sent upstream** (as `SAFE21.io`): PR
[#1](https://bitcoin-after.life/gitea/bitcoinafterlife/bal-server/pulls/1) — IPv6 pinning
for welist reports. As of 0.3.2 upstream's own pusher handles the welist/IPv6 logic, so we
no longer carry a local `resolve_preferring_ipv6`; verify the IPv6 route still works after
each sync (see networking notes).

## Upstream sync procedure (for future releases)

Divergence is now a fixed, tiny set of touch-points, so a sync is mostly mechanical.

Remotes/branches: `upstream` remote → `…/bal-server.git`; `upstream-mirror` branch mirrors
`upstream/main`; `v*` tags mirrored.

```bash
git fetch upstream --tags
git log --oneline v0.3.2..upstream/main        # what's new since our current base
git push origin upstream/main:upstream-mirror --tags
```

Adopt a new version `vX.Y.Z`:

1. On a work branch, pull the vanilla files wholesale from the tag (map into `rust-src/`):
   ```bash
   for f in src/db.rs src/validation.rs src/xpub.rs src/lib.rs \
            src/bin/bal-server.rs src/bin/bal-pusher.rs Cargo.toml Cargo.lock; do
     git show vX.Y.Z:$f > rust-src/$f
   done
   ```
2. Re-apply the touch-points from the table above — they're small and marked. `git diff`
   against the previous umbrel commit shows exactly what to re-add. `umbrel_api.rs` usually
   needs NO change unless upstream altered the `tbl_tx` schema or the Actix handler API.
3. Check the Dockerfile deps against upstream's Dockerfile (e.g. new system libs) and set
   the Rust base image to upstream's version. Set `Cargo.toml` version to `X.Y.Z-umbrel.1`;
   mirror in `umbrel-app.yml` + README badge.
4. Compile-check (below), fix any drift, deploy, smoke-test, monitor one block, merge to `main`.

### Build / compile-check without touching production

The dev box has no working Rust toolchain and Docker needs `sudo`, so compile-check in a
throwaway container with a **persistent target cache** (first run ~2 min, later runs seconds):

```bash
# stage rust-src/ to /home/umbrel/verify/rust-src, then:
sudo docker run --rm -v /home/umbrel/verify/rust-src:/build -w /build rust:1.95-slim-bookworm \
  bash -c 'apt-get update -qq && apt-get install -y -qq pkg-config libzmq3-dev libssl-dev build-essential && cargo check --bins'
```

### Deploy (full-source cutover)

`scripts/deploy-update.sh` (run on the box) extracts a staged tarball
(`/home/umbrel/staged-src.tar.gz` containing `rust-src/ ui/ scripts/ docker/`), backs up
data + the current source (`src-before.tar.gz`), rebuilds both images, recreates the 3
containers and smoke-tests. Rollback: `sudo bash rollback-update.sh <timestamp>`.

- **GOTCHA — line endings:** shell scripts in the tarball MUST be LF. A CRLF shebang
  (`#!/bin/bash\r`) makes the kernel look for `/bin/bash\r` → `exec … no such file or
  directory` → crash-loop. Normalize with `tr -d '\r'` when building the tarball.
- Do NOT put `docker-compose.yml` in the tarball: the device compose (`bal-umbrel-*`) is
  set up per-device and differs from the repo's dev compose (`bal-will-*`).
- The Dockerfile bakes the entrypoints in (`COPY scripts/`), so an entrypoint change needs a
  rebuild — but Docker layer cache makes it fast when `rust-src/` is unchanged.

### Smoke test (after every sync / deploy)

- [ ] `curl …/api/version` → `X.Y.Z-umbrel.N` (JSON/text, not the nginx HTML fallback — HTML
      means the server is down/crash-looping)
- [ ] `curl …/api/txlist?limit=1` → `{total, txs:[{…, confirmations}]}`
- [ ] `curl …/api/txstats/bitcoin` → array incl. `confirmed_profit` + `mempool_profit`
- [ ] `curl …/zmq-status.json` → `{"status":"ok"}`
- [ ] Dashboard: NETWORK STATS, fee grid, and PAYMENT ADDRESS all populated (no `?` / "Error
      loading info" → those mean the Actix rate limiter is starving the load burst)
- [ ] `docker logs bal-umbrel-pusher` — ZMQ `connected`, `blocks: N` ≈ tip; on a new block a
      welist report is sent
- [ ] `docker ps` — all 3 Up with stable uptime (not "Up 1 second" repeatedly = crash-loop)
