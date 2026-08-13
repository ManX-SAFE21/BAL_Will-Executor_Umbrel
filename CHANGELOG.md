# Changelog — Bitcoin After Life Will Executor

All notable changes to this Umbrel app package are documented here.
Format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [Unreleased]

### Changed
- **Versioning scheme clarified**: `<upstream bal-server version>-umbrel.<packaging build>`.
  The first segment always tracks the upstream `bal-server` release this
  codebase is derived from — it only changes when we actually sync/port a
  newer upstream release (see "Upstream sync procedure" in `AGENTS.md`). The
  `-umbrel.N` build number increments for Umbrel-only packaging releases
  (new dashboard features, fixes, branding, etc.) that don't touch the
  upstream lineage. `rust-src/Cargo.toml`, `umbrel-app.yml` and `README.md`
  now all read `0.2.3-umbrel.13` — previously they had drifted into two
  unrelated numbering schemes (`0.2.3-umbrel` in the crate vs. a
  self-invented `0.2.13` counter everywhere else), so the `/version`
  endpoint and the UI badge reported a number that didn't mean anything
  in particular. `0.2.3-umbrel.13` is the 13th Umbrel-packaging build,
  still based on unsynced upstream `0.2.3`.

### Fixed
- **Duplicate version string removed** — `bal-pusher.rs` hard-coded its own
  `VERSION` constant, which could drift from the crate version. It now uses
  `env!("CARGO_PKG_VERSION")`, the same single source of truth as `bal-server.rs`.
  From now on, bumping `rust-src/Cargo.toml` updates both binaries at once.

## [0.2.13] — 2026-07-13

### Added
- **Custom branding** — upload logo image and set logo text via Settings UI,
  visible publicly via `/api/custom-logo` and `/<chain>/info` endpoint.
- `BAL_PUBLIC_URL` env variable — used to build the public `logo_url` in `/info`.
- `.env.example` — sanitized configuration template for public distribution.

### Fixed
- **All Unicode mojibake fixed** — em-dash (—), ellipsis (…), right-arrow (→)
  and emoji corrupted by CP850/CP437 encoding issues are now clean UTF-8
  across every file in the project.
- **Stray `?` before DOCTYPE removed** — caused a stray question mark to appear
  above the logo in the browser.
- **Tor emoji replaced with text** — `🧅` -> `<span class="tor-onion">TOR</span>`
  for reliable cross-platform display.

### Changed
- `umbrel-app.yml` updated with correct em-dash characters in tagline and description.
- Version bumped to 0.2.13 for Umbrel App Store submission.

## [0.2.12] — 2026-07-13

### Added
- Custom logo upload endpoint (`POST /api/upload-logo`, `POST /api/remove-logo`,
  `GET /api/custom-logo`) with magic-bytes validation (PNG/JPEG/WebP/SVG).
- Logo text field in settings with dedicated "Update Logo Text" button.
- `logo_text` and `logo_url` fields in `/<chain>/info` response.
- Client-side validation: format, size ≤200KB, dimensions 32–1024px.
- Branding card in Settings UI with live preview.

### Fixed
- Upload via base64 (pscp) instead of pipe through plink — eliminates Unicode
  corruption during file transfer to Umbrel.

## [0.2.11] — 2026-07-08

### Added
- Logo image upload and branding support (private preview).

## [0.2.10] — 2026-07-01

### Added
- **Backup & Restore** — download full SQLite database via `GET /api/backup`,
  upload and merge backups via `POST /api/merge` (INSERT OR IGNORE — never
  overwrites existing data). Includes nginx config with 500M body size limit.
- **Weekly automatic backups** — every Sunday via `VACUUM INTO`, 12-week retention.
  Runs in bal-pusher, saves to `{db_dir}/backup/bal-YYYYMMDD.db`.
- **Server Description** row in dashboard — third info row below Status,
  loads from `/api/settings`.
- **Network fee per transaction** — displays "—" when unavailable, with
  address decoding fallback for old entries.

### Fixed
- **Cloudflare Tunnel — origin unreachable**: UI container now connected to
  `umbrel_main_network` so cloudflared can resolve `bal-will-ui`.
- **POST /settings on public domain — "Failed to fetch"**: added CORS preflight
  (OPTIONS) handling in nginx settings location, returns 204 with CORS headers.
- **Nginx /api/ prefix stripping broken by double-slash fix**: replaced rewrites
  with `set` + `if` variable construct in settings location.

### Changed
- Container names: `bal-umbrel-*` -> `bal-will-*`
- Image names: `bal-umbrel-rust` -> `bal-will-rust`
- Systemd service: `bal-umbrel.service` -> `bal-will.service`
- App data directory: `app-data/bal-umbrel` -> `app-data/bal-will`
- All documentation rewritten in English
- Clean project structure ready for Umbrel App Store submission

## [0.2.9] — 2026-06-30

### Added
- Umbrel App Store manifest (`umbrel-app.yml`) with icon, category, dependencies
- App icon (`icon.svg`) — Bitcoin orange symbol with lock icon
- Screenshots directory for store gallery images

### Fixed
- ZMQ status not visible in UI due to data directory mismatch
- Settings panel missing after rebuild (outdated index.html on device)

## [0.2.8] — 2026-06-29

### Added
- Server Description field in Settings panel
- Double-slash URL normalization for Electrum plugin compatibility

## [0.2.7] — 2026-06-29

### Fixed
- Regex chain pattern in bal-server.rs — `[^/]?+` captured only 1 character,
  causing 404 on all Electrum requests with chain prefix. Fixed with `[^/]+`
  in an optional nested group.

## [0.2.6] — 2026-06-29

### Added
- Settings Panel in web dashboard (GET/POST /settings)
- Danger confirmation modal for address/fee changes
- `scripts/install-service.sh` — one-command systemd installer

### Fixed
- xpub borrow-after-move in load_settings_file()

### Changed
- Config refactored from `Arc<Mutex<>>` to `Arc<RwLock<>>`

## [0.2.5] — 2026-06-29

### Fixed
- Persistent DB: `APP_DATA_DIR` now defaults to standard Umbrel app-data path
- Auto-start after reboot: added systemd unit with `After=umbrel.service`

## [0.2.4] — 2026-06-28

### Fixed (verified on real Umbrel hardware)
- ZMQ port 28334 (not 28332), env variable properly read
- RPC auth via cookie file (more robust than HTTP-Basic on Umbrel)
- Docker network: pusher joins `umbrel_main_network` to reach Bitcoin Core
- Resilient startup: retry instead of panic
- ZMQ status banner on shared volume
- APP_BITCOIN_* vars explicitly passed into container environment

### Added
- `scripts/configure-umbrel.sh` — auto-detect Bitcoin config
- `.env.example` — configuration template
- FIXES.md — technical bug-fix documentation

## [0.2.3] — 2026-06-25

### Added
- Initial Umbrel OS packaging of bal-server v0.2.3
- Three-container Docker architecture
- Multi-stage Rust build
- Nginx reverse proxy with /api/* routing
- Ed25519 keypair auto-generation on first run
- ZMQ auto-detection and auto-configuration
- Dark web dashboard (vanilla JS, zero external dependencies)
- Tor address display
- tbl_stats creation fix in db.rs
- lib.rs for shared module imports
