# Third-party notices

drtagger for Linux is MIT licensed (see [LICENSE](LICENSE)) except for the
components below, which keep their own licenses. The Flatpak installs this
file, the license texts and a list of every Rust crate with its license in
`/app/share/licenses/us.priet.drtagger/`.

| Component | Where | License | Notes |
|---|---|---|---|
| FFmpeg 8.0 (`ffmpeg`, `ffprobe`) | `/app/bin` in the Flatpak; `Vendor/ffmpeg/` in a development build | LGPL 2.1 or later | Built from the unmodified release tarball (sha256 pinned in the manifest and in `scripts/build-ffmpeg.sh`) with a minimal configuration: only the audio demuxers, decoders, encoders, muxers and filters the app uses; no GPL, non-free, network or video parts. The build fails if the result reports `--enable-gpl`. License texts: `ffmpeg/COPYING.LGPLv2.1`, `ffmpeg/LICENSE.md`. |
| dstkit | `crates/dstkit/` | LGPL 2.1 or later | A port of FFmpeg's `libavcodec/dstdec.c` (DST decoder), through the Mac app's Swift port. See `crates/dstkit/LICENSE`. |
| Chromaprint 1.5.1 | `crates/chromaprintkit/vendor/` | MIT; LGPL 2.1 for the FFmpeg parts it includes | Copyright (C) 2010-2016 Lukas Lalinsky, vendored unmodified and compiled with KissFFT. See its `LICENSE.md`. |
| KissFFT | `crates/chromaprintkit/vendor/chromaprint-1.5.1/kissfft/` | BSD 3-Clause | Copyright (c) 2003-2010 Mark Borgerding. See its `COPYING`. |
| zxing-cpp 2 | statically linked (the `zxing-cpp` crate builds the bundled sources) | Apache 2.0 | Barcode reading. |
| ONNX Runtime 1.31 | statically linked (pyke's build for the `ort` crate) | MIT | Copyright (c) Microsoft Corporation. |
| PaddleOCR PP-OCRv5 mobile detection and recognition models | `/app/share/drtagger/models/ocr/` | Apache 2.0 | PaddlePaddle's official ONNX exports, pinned by revision and sha256. |
| DINOv2 ViT-S/14 | `/app/share/drtagger/models/cover/` | Apache 2.0 | Meta AI's model, ONNX export by onnx-community, pinned by revision and sha256. |
| SQLite | statically linked (`rusqlite`'s bundled build) | Public domain | |
| ICU, GTK, libadwaita, GLib | the GNOME runtime | Unicode / LGPL 2.1 or later | Linked dynamically from the runtime, not shipped by this app. |
| Rust crates | statically linked | MIT, Apache 2.0 and other permissive licenses | `CRATE-LICENSES.txt` lists each crate, version and license. |

The app links dstkit and Chromaprint statically and runs FFmpeg as
separate helper processes. The LGPL is satisfied because the whole
application is open source: anyone can rebuild it, or the Flatpak, with a
modified dstkit, Chromaprint or FFmpeg (the manifest is
`build-aux/us.priet.drtagger.json`).

Services the app talks to (MusicBrainz, Cover Art Archive, AcoustID,
Discogs, CUETools DB, iTunes Search, Deezer, fanart.tv) have their own
terms; the app identifies itself with its own user agent and respects the
published rate limits.
