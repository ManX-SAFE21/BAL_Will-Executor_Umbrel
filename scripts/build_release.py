#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
Build a reproducible release ZIP of the Bitcoin After Life Will Executor
(Umbrel app) and its SHA-256 checksum.

Reproducible means: the same source commit always produces a byte-for-byte
identical ZIP, hence the same SHA-256 — so anyone can rebuild the archive and
confirm the published hash, independently of who built it or on which OS.

Two things make it reproducible:
  * content is read from the committed git blobs at HEAD (not the working
    tree), so line endings are canonical LF regardless of Windows autocrlf;
  * per-file timestamps and permissions are forced to fixed values.

The release therefore always packages HEAD — commit your changes first.

Usage (from the repository root):

    python scripts/build_release.py

Outputs, into ./dist :
    bal-will-executor_v<VERSION>.zip
    bal-will-executor_v<VERSION>.zip.sha256

VERSION is read from the repo-root ./VERSION file (single source of truth).
The GPG signatures (.asc / .sig) are produced separately by the release
manager with their own key + passphrase; see RELEASING.md.
"""

import hashlib
import os
import subprocess
import zipfile

# Directory that holds this script -> the repository root is its parent.
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DIST_DIR = os.path.join(ROOT, "dist")

# Fixed timestamp baked into every ZIP entry (year, month, day, h, m, s).
# Any constant >= 1980 works; keeping it fixed is what makes builds
# reproducible. It is NOT the release date and has no other meaning.
FIXED_TIME = (2020, 1, 1, 0, 0, 0)

PKG = "bal-will-executor"  # artifact + top-level folder name prefix


def read_version() -> str:
    with open(os.path.join(ROOT, "VERSION"), "r", encoding="utf-8") as f:
        return f.read().strip()


def head_files() -> list:
    """Tracked file paths at HEAD, sorted, excluding build outputs."""
    out = subprocess.run(
        ["git", "ls-tree", "-r", "--name-only", "HEAD"],
        cwd=ROOT, capture_output=True, text=True, check=True,
    ).stdout
    return sorted(p for p in out.splitlines() if p and not p.startswith("dist/"))


def blob_at_head(path: str) -> bytes:
    """Canonical committed content of `path` at HEAD (LF, no autocrlf)."""
    return subprocess.run(
        ["git", "show", f"HEAD:{path}"],
        cwd=ROOT, capture_output=True, check=True,
    ).stdout


def build_zip(zip_path: str, files: list, top: str) -> None:
    """Deterministic ZIP: fixed order, fixed timestamps, fixed permissions."""
    if os.path.exists(zip_path):
        os.remove(zip_path)
    with zipfile.ZipFile(zip_path, "w", compression=zipfile.ZIP_DEFLATED) as z:
        for rel in files:
            info = zipfile.ZipInfo(f"{top}/{rel}", date_time=FIXED_TIME)
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o644 << 16   # regular file, rw-r--r--
            z.writestr(info, blob_at_head(rel))


def sha256_of(path: str) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def main() -> None:
    version = read_version()
    dirty = subprocess.run(
        ["git", "status", "--porcelain"], cwd=ROOT, capture_output=True, text=True
    ).stdout.strip()
    if dirty:
        print("NOTE: uncommitted changes present. This packages HEAD (the last")
        print("      commit), NOT your working tree. Commit first for a real release.\n")

    os.makedirs(DIST_DIR, exist_ok=True)
    base = f"{PKG}_v{version}.zip"
    zip_path = os.path.join(DIST_DIR, base)
    top = f"{PKG}_v{version}"

    files = head_files()
    build_zip(zip_path, files, top)

    digest = sha256_of(zip_path)
    sha_path = zip_path + ".sha256"
    with open(sha_path, "w", encoding="utf-8", newline="\n") as f:
        f.write(f"{digest}  {base}\n")

    print(f"Built {len(files)} files into dist/{base}")
    print(f"SHA-256  {digest}")
    print(f"Wrote    dist/{base}.sha256")
    print()
    print("Next: sign it with your key (see RELEASING.md):")
    print("  cd dist")
    print(f"  gpg --local-user 206C20114CA96172 --armor --detach-sign {base}")
    print(f"  gpg --local-user 206C20114CA96172 --detach-sign {base}")


if __name__ == "__main__":
    main()
