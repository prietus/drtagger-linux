#!/usr/bin/env python3
"""Writes build-aux/cargo-sources.json: the crates.io packages of Cargo.lock
as Flatpak sources, vendored for an offline `cargo build` (Flathub builds
have no network).

Every registry package becomes an archive source unpacked into
cargo/vendor/<name>-<version> with the checksum Cargo.lock records, plus the
.cargo-checksum.json cargo wants beside it, and a cargo config that replaces
crates.io with that folder. Run it after every Cargo.lock change:

    scripts/cargo-sources.py
"""

import json
import os
import tomllib

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REGISTRY = "registry+https://github.com/rust-lang/crates.io-index"
VENDOR = "cargo/vendor"


def main():
    with open(os.path.join(ROOT, "Cargo.lock"), "rb") as f:
        lock = tomllib.load(f)
    sources = []
    for p in lock["package"]:
        source = p.get("source")
        if source is None:
            continue  # a workspace member
        if source != REGISTRY:
            raise SystemExit(f"unsupported source for {p['name']}: {source}")
        name, version, checksum = p["name"], p["version"], p["checksum"]
        dest = f"{VENDOR}/{name}-{version}"
        sources.append({
            "type": "archive",
            "archive-type": "tar-gzip",
            "url": f"https://static.crates.io/crates/{name}/{name}-{version}.crate",
            "sha256": checksum,
            "dest": dest,
        })
        sources.append({
            "type": "inline",
            "contents": json.dumps({"package": checksum, "files": {}}),
            "dest": dest,
            "dest-filename": ".cargo-checksum.json",
        })
    sources.append({
        "type": "inline",
        "contents": (
            '[source.crates-io]\nreplace-with = "vendored-sources"\n\n'
            f'[source.vendored-sources]\ndirectory = "{VENDOR}"\n'
        ),
        "dest": "cargo",
        "dest-filename": "config.toml",
    })
    out = os.path.join(ROOT, "build-aux/cargo-sources.json")
    with open(out, "w") as f:
        json.dump(sources, f, indent=1)
        f.write("\n")
    print(f"{len(sources) // 2} crates → {os.path.relpath(out, ROOT)}")


if __name__ == "__main__":
    main()
