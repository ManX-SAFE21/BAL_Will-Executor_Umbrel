# Umbrel App Store Submission Guide

This guide covers how to submit **Bitcoin After Life Will Executor** to the
[Umbrel Community App Store](https://github.com/getumbrel/umbrel-community-app-store).

---

## Prerequisites

1. A **GitHub account**
2. Fork the [umbrel-community-app-store](https://github.com/getumbrel/umbrel-community-app-store) repo
3. Your app files ready (this project)

---

## Step 1 — Prepare your app files

The `umbrel-app.yml` manifest is already included in the project. Verify:

- **id**: `bitcoin-after-life-will-executor`
- **name**: `Bitcoin After Life Will Executor`
- **version**: update if needed
- **port**: `9140`
- **dependencies**: includes `bitcoin`
- **icon**: provide a URL to your hosted icon (see Step 3)

---

## Step 2 — Create screenshots

Add PNG screenshots (1280x800 or 1920x1200 recommended) to the `screenshots/` directory:

1. `dashboard.png` — Main dashboard with server status, stats, network selector
2. `settings.png` — Settings panel with payment address, fee, and description
3. `submit-tx.png` — Submit transaction panel with hex input
4. `search-tx.png` — Search transaction panel with result display
5. `backup.png` — Backup & Restore panel

Then upload these screenshots to a public URL (GitHub raw URL, your own server,
or a hosting service) and update the `gallery:` field in `umbrel-app.yml`.

---

## Step 3 — Host the icon

The `icon.svg` file is included in the project. You need to host it at a
public URL. Options:

- Upload to a GitHub repository and use the raw URL
- Upload to your own web server
- Use an image hosting service

Update the `icon:` field in `umbrel-app.yml` with the public URL.

---

## Step 4 — Create the app directory structure

The community app store expects this structure:

```
umbrel-community-app-store/
└── bitcoin-after-life-will-executor/
    ├── umbrel-app.yml
    ├── icon.svg
    ├── docker-compose.yml
    ├── docker/
    │   ├── Dockerfile.rust
    │   ├── Dockerfile.ui
    │   ├── Dockerfile.tor
    │   └── torrc
    ├── rust-src/
    │   ├── Cargo.toml
    │   └── src/
    │       └── ...
    ├── ui/
    │   ├── index.html
    │   └── nginx.conf
    └── scripts/
        └── ...
```

---

## Step 5 — Submit a pull request

1. Fork the [community app store repo](https://github.com/getumbrel/umbrel-community-app-store)
2. Create a new branch: `git checkout -b add-bitcoin-after-life-will-executor`
3. Copy your app directory into the repo root
4. Commit and push
5. Open a pull request with:
   - Clear title: "Add Bitcoin After Life Will Executor"
   - Description explaining what the app does
   - Screenshots in the PR description

---

## Review checklist

Before submitting, verify:

| Check | Status |
|---|---|
| `umbrel-app.yml` has correct `id` (no spaces) | |
| `umbrel-app.yml` has correct `name` (display name) | |
| `version` follows semver | |
| `port` does not conflict with other apps | |
| `dependencies` includes `bitcoin` | |
| `icon` points to publicly accessible URL | |
| `gallery` contains 3-5 screenshot URLs | |
| `docker-compose.yml` uses `umbrel_main_network` | |
| Services expose correct ports | |
| No hardcoded secrets or credentials | |
| All documentation is in English | |
| `.env` file is NOT included (only `.env.example`) | |

---

## Resources

- [Umbrel Community App Store](https://github.com/getumbrel/umbrel-community-app-store)
- [Umbrel App Store Guidelines](https://github.com/getumbrel/umbrel-community-app-store/blob/main/CONTRIBUTING.md)
- [Umbrel App Manifest Reference](https://github.com/getumbrel/umbrel-community-app-store#app-manifest)
