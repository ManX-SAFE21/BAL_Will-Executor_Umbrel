# Bug Fixes Applied to Source Code

Technical documentation explaining every bug found during real Umbrel
installation and how it was fixed. Keep for reference or for upstream PRs.

---

## 1. ZMQ — wrong port and ignored environment variable

**Symptoms:** pusher logs "zmq listening on: tcp://127.0.0.1:28332" and never
receives blocks; UI banner stays on "ZMQ not detected".

**Causes:**
- On Umbrel the `hashblock` topic port is **28334**, not 28332
- The Rust binary ignored the `BAL_PUSHER_BITCOIN_ZMQ_HASHBLOCK` env var
- In `parse_env_netconfig()`, the variable was saved to `rpc_pass` instead of `zmq_listener`

**Fix (bal-pusher.rs):**
- `parse_env_netconfig()`: `BAL_PUSHER_*_ZMQ_HASHBLOCK` now populates `cfg.zmq_listener`
- In `main()`: ZMQ address resolved with priority: `network_params.zmq_listener` → `cfg.zmq_listener`

**Fix (config):** `BAL_ZMQ_PORT=28334` and explicit `BAL_PUSHER_BITCOIN_ZMQ_HASHBLOCK` in `.env`.

---

## 2. RPC — HTTP 401 authentication failure

**Symptoms:** `JsonRpc(Transport(HttpErrorCode(401)))` → RPC connection fails.

**Cause:** `bitcoincore-rpc` v0.19 fails HTTP-Basic auth with Umbrel's auto-generated
RPC password (contains base64 chars like `=` and `+`). Same request via `nc`/raw HTTP
works (200 OK) — library bug. Fallback to cookie file worked but cookie was not mounted.

**Fix (bal-pusher.rs, `get_client()`):**
- Try cookie file first if configured and existing
- Ordered fallback: cookie → user/pass → cookie (last attempt)

**Fix (entrypoint-pusher.sh):**
- Auto-detect cookie at `/bitcoin-data/.cookie` and alternatives

---

## 3. Docker network — pusher cannot reach Bitcoin Core

**Symptoms:** Port 8332/28334 unreachable from pusher container.

**Cause:** Bitcoin Core is on `umbrel_main_network` (10.21.0.0/16), but pusher
was only on the app's internal networks.

**Fix (docker-compose.yml):**
- Added `umbrel_main_network` (as external network) to pusher's networks

---

## 4. Crash on startup instead of retry

**Symptoms:** Pusher panics if Bitcoin Core is not immediately reachable,
entering crash-loop and never connecting.

**Fix (bal-pusher.rs):**
- `main_result()` returns `Result<(), Box<dyn StdError>>` instead of panicking
- Retries up to 10 times with 6s delay before entering ZMQ loop

---

## 5. ZMQ banner — file written in wrong container

**Symptoms:** Logs show `status=ok` but UI continues showing red banner.

**Cause:** `check-zmq.sh` (runs in pusher container) wrote `zmq-status.json` to
the pusher's local filesystem, while the UI container reads from its own filesystem.

**Fix (check-zmq.sh):**
- Write `zmq-status.json` to the shared `/data` volume as well

**Fix (nginx.conf):**
- Serve `/zmq-status.json` from `/app-data` (shared volume mounted read-only in UI)
- Fallback `{"status":"pending"}` if file is missing

---

## 6. APP_BITCOIN_* variables not injected into container

**Symptoms:** Pusher entrypoint sees `APP_BITCOIN_NODE_IP` as empty.

**Cause:** In compose, `${APP_BITCOIN_NODE_IP}` was only used for interpolation,
NOT passed as container environment variables.

**Fix (docker-compose.yml):**
- Listed explicitly: `APP_BITCOIN_NODE_IP`, `APP_BITCOIN_RPC_PORT`,
  `APP_BITCOIN_RPC_USER`, `APP_BITCOIN_RPC_PASS`, `APP_BITCOIN_DATA_DIR`,
  `BAL_ZMQ_PORT` in pusher's environment section

---

## 7. Database not persistent — data lost on container recreation

**Symptoms:** After `docker compose down && docker compose up`, bal.db is empty.

**Cause:** `APP_DATA_DIR` was not set, so compose volume defaulted to `./data`
(project directory). Risky if project folder is deleted.

**Fix (docker-compose.yml):**
- Default volume changed to `/home/umbrel/umbrel/app-data/bal-will/data`

**Fix (.env.example):**
- Documented `APP_DATA_DIR` variable

---

## 8. Containers do not restart after Umbrel reboot

**Symptoms:** After `sudo reboot`, containers are gone; app unreachable until
manual `docker compose up -d`.

**Cause:** Containers started manually have no init dependency. When Umbrel
restarts, it cleans its managed containers but does not restart external ones.

**Fix:**
- Created systemd unit at `/etc/systemd/system/bal-will.service`:
  - `After=umbrel.service docker.service` — starts after Umbrel is ready
  - `Type=oneshot` + `RemainAfterExit=yes` — correct pattern for `docker compose up -d`
  - `ExecStop=docker compose down` — clean shutdown
- `systemctl enable bal-will.service` — enabled on boot

---

## 9. Settings Panel — xpub borrow-after-move in load_settings_file

**Symptoms:** Rust compiler error "borrow of moved value: `s.address`".

**Cause:** `cfg_write.mainnet.address = s.address;` (move) before
`if s.address[1..4] == *"pub"` (borrow of moved value).

**Fix (bal-server.rs):**
- Check xpub BEFORE the move:
  ```
  let is_xpub = s.address[1..4] == *"pub";
  cfg_write.mainnet.address = s.address;
  if is_xpub { cfg_write.mainnet.xpub = true; }
  ```

---

## 10. Double-slash URL normalization

**Symptoms:** Electrum plugin cannot reach server. It calls `//bitcoin/info`
instead of `/bitcoin/info` (base URL stored with trailing slash).

**Fix (nginx.conf):**
- Added `rewrite ^ $uri break;` before each `proxy_pass` in regex locations
  to force nginx to use the normalized URI ($uri) for backend requests

---

## 11. Regex chain — `[^/]?+` captures only 1 character

**Symptoms:** `POST /bitcoin/pushtxs` returns 404. `[^/]?+` (possessive
quantifier) captures only "b", then fails to match the rest.

**Fix (bal-server.rs):**
- Replaced `[^/]?+` with `[^/]+` in an optional nested group:
  ```
  r"^/?((?P<param>[^/]+)/)?(info|stats|pushtxs)$"
  ```
- `match_uri()` now returns `Some("")` when param group is absent

---

## Files modified

| File | Change |
|---|---|
| `rust-src/src/bin/bal-pusher.rs` | ZMQ env fix, cookie-first auth, retry, no-panic |
| `rust-src/src/bin/bal-server.rs` | Arc<RwLock>, GET/POST /settings, settings.json, xpub fix, regex fix, backup/merge |
| `scripts/entrypoint-pusher.sh` | Auto-detect cookie, export ZMQ hashblock var |
| `scripts/check-zmq.sh` | Write status to shared /data volume |
| `ui/nginx.conf` | Serve zmq-status.json from /app-data, double-slash fix, backup/merge routes, CORS preflight |
| `ui/index.html` | Settings panel, danger modal, backup/restore UI, description row |
| `docker-compose.yml` | Explicit env vars, umbrel_main_network, cookie mount, APP_DATA_DIR default, container naming |
| `.env.example` | Configuration template |
| `scripts/configure-umbrel.sh` | Auto-configuration script |
| `scripts/install-service.sh` | Systemd service installer |
| `/etc/systemd/system/bal-will.service` | Auto-start on boot (created on host by install-service.sh) |
