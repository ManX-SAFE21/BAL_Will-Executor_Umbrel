# Releasing the Bitcoin After Life Will Executor (Umbrel)

The fixed procedure for cutting a **signed, verifiable** release. Follow it
every time; it always produces the same set of assets.

The signing key for this project is **SAFE21dev `<info@safe21.io>`**,
fingerprint `33E3393DFB10F4C45AE6F1E8206C20114CA96172`
(short key id `206C20114CA96172`). Its public key lives in the repository as
[`SAFE21dev.asc`](SAFE21dev.asc) and is uploaded with every release.

## What each release publishes

For version `X.Y.Z-umbrel`, the GitHub Release carries these assets:

| File | What it is |
|------|------------|
| `bal-will-executor_vX.Y.Z-umbrel.zip` | the full app source (Umbrel builds it from source) |
| `bal-will-executor_vX.Y.Z-umbrel.zip.sha256` | SHA-256 checksum of the ZIP |
| `bal-will-executor_vX.Y.Z-umbrel.zip.asc` | GPG signature, armored (text) |
| `bal-will-executor_vX.Y.Z-umbrel.zip.sig` | GPG signature, binary |
| `SAFE21dev.asc` | the signing public key |

## Steps

### 1. Bump the version

`VERSION` (repo root) is the single source of truth for the **release** version.
Set the same value in:

- `VERSION`
- `umbrel-app.yml` (the `version:` field)
- `README.md` (the version badge)

The Rust binary reports its own version at `/api/version`, read from
`rust-src/Cargo.toml`; keep its `<upstream>-umbrel` base aligned with `VERSION`
(the trailing `.<n>` build counter on the crate is an internal packaging
increment and may differ between rebuilds of the same release).

Commit that change.

### 2. Build the ZIP + checksum (reproducible)

From the repository root:

```bash
python scripts/build_release.py
```

This writes `dist/bal-will-executor_v<VERSION>.zip` and its `.sha256`. The build
reads the committed content at `HEAD` (canonical LF, fixed timestamps), so
re-running it — on any OS — yields a byte-identical ZIP and the same hash.
It always packages `HEAD`, so **commit step 1 first**.

### 3. Sign the ZIP (release manager only)

Signing needs the private key and its passphrase, so it is done by hand, not by
any script. On Windows this is easiest in **PowerShell** (Gpg4win shows the
passphrase dialog):

```bash
cd dist
gpg --local-user 206C20114CA96172 --armor --detach-sign bal-will-executor_v<VERSION>.zip
gpg --local-user 206C20114CA96172 --detach-sign bal-will-executor_v<VERSION>.zip
```

The first command makes the armored `.asc`, the second the binary `.sig`.

### 4. Verify locally before publishing

```bash
cd dist
sha256sum -c bal-will-executor_v<VERSION>.zip.sha256
gpg --verify bal-will-executor_v<VERSION>.zip.asc bal-will-executor_v<VERSION>.zip
```

Expected: `bal-will-executor_v<VERSION>.zip: OK` and
`Good signature from "SAFE21dev <info@safe21.io>"`.

### 5. Tag the commit

```bash
git tag -a v<VERSION> -m "Bitcoin After Life Will Executor v<VERSION>"
git push origin v<VERSION>
```

### 6. Create the GitHub Release

`gh` CLI is not installed, so use the web UI:

1. Go to <https://github.com/ManX-SAFE21/BAL_Will-Executor_Umbrel/releases> →
   **Draft a new release**.
2. Choose the tag `v<VERSION>`.
3. Upload the five assets from the table above (the four `dist/` files plus
   `SAFE21dev.asc` from the repo root).
4. Paste the verification block below into the release notes.
5. Publish.

## Verification block (paste into the release notes)

````markdown
### Verify this release

```bash
# import the signing key once
gpg --import SAFE21dev.asc

# check the checksum and the signature
sha256sum -c bal-will-executor_vX.Y.Z-umbrel.zip.sha256
gpg --verify bal-will-executor_vX.Y.Z-umbrel.zip.asc bal-will-executor_vX.Y.Z-umbrel.zip
```

Expected: `… OK` and `Good signature from "SAFE21dev <info@safe21.io>"`
(fingerprint `33E3393DFB10F4C45AE6F1E8206C20114CA96172`).
````

## Notes

- `dist/` is git-ignored: release artifacts are build outputs, not source. Only
  `SAFE21dev.asc`, `scripts/build_release.py`, `VERSION` and this document live
  in the repository.
- Never commit or upload the private key. Only `SAFE21dev.asc` (public) is ever
  shared.
