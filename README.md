# drtagger for Linux

A GNOME app (Rust, GTK4 + libadwaita) that identifies, splits and tags
audiophile rips — SACD ISOs, CD images with a CUE sheet and folders of
lossless tracks. It is the Linux port of
[drtagger for Mac](https://github.com/prietus/drtagger-mac), aiming at full
feature parity; see [DESIGN.md](DESIGN.md) for scope, decisions and phases.

**Status:** feature-complete against the Mac app (phases 0–10), plus a
watched rip inbox that the Mac app does not have yet. Packaged as a Flatpak
(phase 11).

## Installing

Download the bundle for your architecture (`drtagger-<version>-x86_64.flatpak`
or `-aarch64.flatpak`) and install it; the GNOME runtime comes from Flathub:

```sh
flatpak install --user drtagger-0.1.0-x86_64.flatpak
flatpak run us.priet.drtagger
```

To build the bundle yourself: `scripts/release.sh` (needs `flatpak-builder`;
it fetches the GNOME 50 SDK and builds offline from pinned sources).

## Building

Requirements: Rust stable, GTK ≥ 4.22, libadwaita ≥ 1.9, `blueprint-compiler`,
`cmake`, ICU, gettext; `scripts/build-ffmpeg.sh` for the ffmpeg the app
drives and `scripts/fetch-models.sh` for the recognition models.

```sh
export RUST_ICU_MAJOR_VERSION_NUMBER=$(pkg-config --modversion icu-i18n | cut -d. -f1)
cargo build -p drtagger -p drtagger-cli
glib-compile-schemas --targetdir=target/schemas data
GSETTINGS_SCHEMA_DIR=target/schemas target/debug/drtagger
```

`drtagger-cli` is the headless front end (`drtagger-cli --help`).

Developing from a Mac against a Linux VM: `scripts/vm.sh` syncs the
repository and runs cargo, the CLI or the app there.

Artwork OCR and cover comparison need the ONNX models, which are not in
git: `scripts/fetch-models.sh` downloads them into `models/` (118 MB,
pinned and checksummed). Without them barcodes are still read.

## Licence

MIT (see [LICENSE](LICENSE)). The DST decoder crate (`crates/dstkit`) is
LGPL-2.1-or-later, being a port of FFmpeg's `dstdec.c`; the Flatpak bundles
LGPL builds of FFmpeg and Chromaprint. `visionkit` links zxing-cpp
(Apache-2.0) and ONNX Runtime (MIT) statically and uses PaddleOCR's
PP-OCRv5 models and Meta's DINOv2 (both Apache-2.0).
