#!/usr/bin/env python3
"""Lists every vendored Rust crate with its version and license (from its
own Cargo.toml), for the licenses folder of the Flatpak:

    scripts/crate-licenses.py <vendor dir> > CRATE-LICENSES.txt
"""

import os
import sys
import tomllib


def main():
    vendor = sys.argv[1]
    rows = []
    for name in sorted(os.listdir(vendor)):
        manifest = os.path.join(vendor, name, "Cargo.toml")
        if not os.path.isfile(manifest):
            continue
        with open(manifest, "rb") as f:
            package = tomllib.load(f).get("package", {})
        license = package.get("license") or ("see " + package["license-file"] if package.get("license-file") else "?")
        rows.append((package.get("name", name), package.get("version", "?"), license))
    print("Rust crates linked into drtagger (name, version, license)\n")
    for name, version, license in rows:
        print(f"{name} {version}: {license}")


if __name__ == "__main__":
    main()
