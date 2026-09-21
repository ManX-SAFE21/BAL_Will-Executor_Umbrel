# Bitcoin After Life Will Executor — Installation Guide

Complete Umbrel app package. Installation is just: **copy -> configure -> build -> start**.

---

## Table of Contents

- [Prerequisites](#prerequisites)
- [Step 1 — Copy files to Umbrel](#step-1--copy-files-to-umbrel)
- [Step 2 — Configure](#step-2--configure)
- [Step 3 — Build and start](#step-3--build-and-start)
- [Step 4 — Auto-start on reboot](#step-4--auto-start-on-reboot)
- [Step 5 — Open dashboard](#step-5--open-dashboard)
- [Quick verification](#quick-verification)
- [Where data is stored](#where-data-is-stored)
- [Updating](#updating)
- [Useful commands](#useful-commands)
- [Uninstallation](#uninstallation)
- [Troubleshooting](#troubleshooting)

---

## Prerequisites

- Umbrel OS (tested on Umbrel 0.5+)
- **Bitcoin Node** app installed and **100% synced**
- SSH access to Umbrel

> **Windows users:** Use PowerShell (built-in `ssh`) or PuTTY (`pscp`/`plink`).

---

## Step 1 — Copy files to Umbrel

From your PC, copy the entire project folder to Umbrel:

```bash
# Via SCP
scp -r bitcoin-after-life-will-executor/ umbrel@umbrel.local:~/umbrel/home/bitcoin-after-life-will-executor/
```

Or ZIP it first for faster transfer:

```bash
zip -r bal-will.zip bitcoin-after-life-will-executor/
scp bal-will.zip umbrel@umbrel.local:~/
ssh umbrel@umbrel.local
sudo apt install unzip -y
unzip -o bal-will.zip
sudo mv bitcoin-after-life-will-executor ~/umbrel/home/bitcoin-after-life-will-executor
cd ~/umbrel/home/bitcoin-after-life-will-executor
```

---

## Step 2 — Configure

Auto-detect all Bitcoin parameters (IP, RPC, ZMQ port):

```bash
sudo bash scripts/configure-umbrel.sh
```

Then set your Bitcoin address to receive fees:

```bash
nano .env
```

Edit these lines:

```
BAL_BITCOIN_ADDRESS=bc1q...   # YOUR Bitcoin address (or xpub/zpub)
BAL_BITCOIN_FEE=30000         # Minimum fee in satoshis
BAL_PUBLIC_URL=               # Leave empty, set later for Cloudflare
```

Save: `Ctrl+O`, Enter, `Ctrl+X`.

> If `BAL_BITCOIN_ADDRESS` is empty, the server accepts transactions for free (testing only).

---

## Step 3 — Build and start

```bash
sudo docker compose up -d --build
```

**First build times:**
- Raspberry Pi 4: ~15-25 minutes
- PC/server x86: ~3-5 minutes

Follow the logs to verify everything starts:

```bash
sudo docker compose logs -f
```

When you see:
```
[bal_pusher] connected
[bal_pusher] zmq listening on: tcp://10.21.21.8:28334
[bal_pusher] waiting new blocks..
```

...everything is working. `Ctrl+C` to exit logs.

---

## Step 4 — Auto-start on reboot

Without this step, containers stop after every Umbrel reboot:

```bash
sudo bash scripts/install-service.sh
```

Verify:

```bash
sudo systemctl status bal-will.service
# Should show: Active: active (exited) — normal for Type=oneshot
```

---

## Step 5 — Open dashboard

```
http://umbrel.local:9140
```

Check:
- **ZMQ connected** (green banner) — Bitcoin Core notifies new blocks
- **Status: Online** — server is active
- **Stats** — data appears when the first block arrives

> If ZMQ banner shows "Restart Required": go to Settings > Bitcoin Node > Restart.

---

## Quick verification

```bash
# All containers running
sudo docker compose ps

# Server info
curl -s http://umbrel.local:9140/bitcoin/info

# ZMQ status
curl -s http://umbrel.local:9140/zmq-status.json

# Settings
curl -s http://umbrel.local:9140/settings

# Auto-start status
sudo systemctl status bal-will.service
```

---

## Where data is stored

```
~/umbrel/app-data/bal-will/data/
├── bal.db                 # SQLite database (transactions)
├── settings.json          # Payment address, fee, description
├── backup/                # Weekly automatic backups
│   └── bal-YYYYMMDD.db
├── keys/
│   ├── private_key.pem    # Ed25519 key (signed stats)
│   └── public_key.pem     # Public key
└── zmq-status.json        # ZMQ status (ok/restart_required/error)
```

All data survives reboots, updates, and container recreations.

---

## Updating

```bash
# 1. Copy new files to Umbrel
scp -r bitcoin-after-life-will-executor/ umbrel@umbrel.local:~/umbrel/home/bitcoin-after-life-will-executor/

# 2. SSH and rebuild
ssh umbrel@umbrel.local
cd ~/umbrel/home/bitcoin-after-life-will-executor
sudo docker compose up -d --build
```

Persistent data (`settings.json`, `bal.db`, `keys/`, `backup/`) is preserved.

---

## Useful commands

```bash
# Live logs
sudo docker logs -f bal-will-server
sudo docker logs -f bal-will-pusher
sudo docker logs -f bal-will-ui

# Container status
sudo docker compose ps

# Boot auto-start hook (installed at a persistent path)
ls -l /home/umbrel/umbrel/custom-hooks/pre-start
cat /home/umbrel/umbrel/app-data/bal-umbrel/autostart.log

# Restart containers
sudo docker compose restart

# Rebuild and restart
sudo docker compose up -d --build

# Stop containers
sudo docker compose down
```

> Auto-start is NOT a systemd unit: umbrelOS runs `/` on an overlay that is
> discarded on reboot, so units written to `/etc` disappear. See [AGENTS.md](AGENTS.md).

---

## Uninstallation

```bash
# 1. Stop and remove auto-start
sudo docker compose down
sudo rm -f /home/umbrel/umbrel/custom-hooks/pre-start

# 2. Remove containers and images
cd ~/umbrel/home/bitcoin-after-life-will-executor
sudo docker compose down --rmi all

# 3. Remove app files
cd ~
sudo rm -rf ~/umbrel/home/bitcoin-after-life-will-executor

# 4. (Optional) Delete all data — WARNING: loses ALL transactions
sudo rm -rf ~/umbrel/app-data/bal-will

# 5. (If using manual cloudflared)
sudo docker rm -f cloudflared
```

---

## Troubleshooting

### Pusher: HTTP 401 / auth failed
The cookie file is the most robust method. Verify `APP_BITCOIN_DATA_DIR` in `.env` points to the directory containing `.cookie`.

### Pusher: "zmq listening on: tcp://127.0.0.1:28332"
ZMQ variable not being read. Check `BAL_ZMQ_PORT=28334` in `.env`, then:
```bash
sudo docker compose up -d --force-recreate
```

### Pusher cannot reach Bitcoin Core
The pusher must be on the `umbrel_main_network` network. Verify:
```bash
sudo docker inspect bal-will-pusher | grep 10.21
```

### Red ZMQ banner but logs say "ok"
Browser cache. `Ctrl+F5`.
If persistent: check the shared volume:
```bash
sudo docker exec bal-will-ui cat /app-data/zmq-status.json
```

### Settings on public domain — "Network error: Failed to fetch"
The browser sends a preflight OPTIONS before POST. Make sure you have nginx.conf with OPTIONS handling (returns 204). Then rebuild:
```bash
sudo docker compose up -d --build ui
```
For settings, access via LAN (`http://umbrel.local:9140`).

### "Network not enabled" when submitting transaction
`BAL_BITCOIN_ADDRESS` is not set or not valid in `.env`.

### Double-slash not working (404)
Rebuild: `sudo docker compose up -d --build`. Test:
```bash
curl -s http://umbrel.local:9140//bitcoin/info
```

### Cloudflare Tunnel: "Unable to reach the origin service"
The UI container must be on `umbrel_main_network`. Add it:
```bash
sudo docker network connect umbrel_main_network bal-will-ui
```

### Build fails
- Does the container have internet access? Cargo downloads crates on first build.
- Enough memory? RPi4 needs at least 2 GB free.
