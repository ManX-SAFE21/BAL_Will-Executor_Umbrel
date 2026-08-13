# Cloudflare Tunnel Setup Guide

Make **Bitcoin After Life Will Executor** accessible from anywhere via
Cloudflare Tunnel — no static IP, no port forwarding required.

---

## Prerequisites

1. A **domain** (e.g. `your-executor.com`) registered with any registrar
2. A **free Cloudflare account**

---

## Step 1 — Add your domain to Cloudflare

1. Go to https://dash.cloudflare.com → **Add a site**
2. Enter your domain, select the **Free** plan
3. Cloudflare will show you two nameservers (e.g. `dns1.ns.cloudflare.com`)
4. At your domain registrar, replace the current nameservers with Cloudflare's
5. Wait for propagation (minutes to a few hours)

---

## Step 2 — Create a Tunnel

1. Cloudflare dashboard → **Zero Trust** → create a team name (Free plan)
2. **Networks → Tunnels → Create a tunnel**
3. Select **Cloudflared** → name it (e.g. `umbrel-tunnel`)
4. Click **Save** and copy the tunnel **token** (starts with `eyJ...`)

---

## Step 3 — Install cloudflared on Umbrel

### Option A — Umbrel App Store (recommended)
1. Open Umbrel dashboard → **App Store**
2. Search for **"Cloudflare Tunnel"** → Install
3. Enter your tunnel token in the app settings

### Option B — Manual Docker container
```bash
sudo docker run -d --restart unless-stopped \
  --name cloudflared \
  --network umbrel_main_network \
  cloudflare/cloudflared:latest tunnel --no-autoupdate run \
  --token eyJ...   # your tunnel token
```

Verify:
```bash
sudo docker logs cloudflared --tail 5
# Should show: INF |  SUMMARY: Environment is healthy.
```

---

## Step 4 — Configure the public hostname

In the Cloudflare dashboard (Tunnel → your tunnel → **Public Hostname**):

| Field | Value |
|---|---|
| Subdomain | `executor` (your choice) |
| Domain | `your-executor.com` |
| Type | `HTTP` |
| URL | `bal-will-ui:80` |

**Important:** Both `cloudflared` and `bal-will-ui` must be on the
`umbrel_main_network` Docker network.

---

## Step 5 — Set the public URL

Edit the `.env` file on Umbrel:

```
BAL_PUBLIC_URL=https://executor.your-executor.com
```

Then recreate the containers:

```bash
sudo docker compose up -d
```

---

## Step 6 — Security considerations

- A **minimum fee** (`BAL_BITCOIN_FEE`) discourages spam transactions
- Cloudflare **Rate Limiting** (Free plan) protects your public hostname
- The **Settings button** is **hidden** on the public domain
- POST requests to `/settings` from the public domain return **403**
- Only expose `bal-will-ui:80`, never the Umbrel dashboard or Bitcoin RPC
- Consider enabling Cloudflare **WAF** rules and **Bot Fight Mode**

---

## Testing

```bash
# Normal request
curl -s https://executor.your-executor.com/bitcoin/info

# Double-slash (Electrum plugin compatibility)
curl -s https://executor.your-executor.com//bitcoin/info

# Settings (should return 403 on public domain)
curl -s -X POST https://executor.your-executor.com/settings \
  -H "Content-Type: application/json" \
  -d '{"address":"bc1q...","fee":30000}'
```

---

## Troubleshooting

### "Unable to reach the origin service"
cloudflared cannot resolve `bal-will-ui`. Ensure:
1. Both are on `umbrel_main_network`:
   ```bash
   sudo docker network connect umbrel_main_network bal-will-ui
   ```
2. cloudflared is also on the same network
3. Recreate containers after network changes:
   ```bash
   sudo docker compose up -d ui
   ```

### Tunnel shows "Healthy" but site is unreachable
Check that your domain's DNS is pointing to Cloudflare's nameservers.
Use `dig your-executor.com NS` to verify.
