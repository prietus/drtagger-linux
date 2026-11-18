#!/bin/bash
#
# Builds the minimal LGPL ffmpeg + ffprobe drtagger drives, for Linux: the
# same release, checksum and component list as the Mac app's
# scripts/build-ffmpeg.sh, so the FLAC encoder writes the same bytes. Only
# the audio demuxers, decoders, encoders, muxers and filters the app uses
# are compiled in; video, network, GPL and non-free parts are off, so the
# static binaries are redistributable under LGPL 2.1+.
#
# Usage:
#   scripts/build-ffmpeg.sh
#   FFMPEG_VERSION=8.0 FFMPEG_SHA256=… scripts/build-ffmpeg.sh
#
# Output: Vendor/ffmpeg/{ffmpeg,ffprobe,BUILD-INFO.txt,LICENSE.md,COPYING.LGPLv2.1}
# Requirements: clang, make, curl, xz. nasm is not needed (x86 asm off).

set -euo pipefail

FFMPEG_VERSION="${FFMPEG_VERSION:-8.0}"
FFMPEG_SHA256="${FFMPEG_SHA256:-b2751fccb6cc4c77708113cd78b561059b6fa904b24162fa0be2d60273d27b8e}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="${FFMPEG_WORK:-$ROOT/.build/ffmpeg}"
OUT="$ROOT/Vendor/ffmpeg"
JOBS="$(nproc)"
ARCH="$(uname -m)"

mkdir -p "$WORK" "$OUT"

TARBALL="$WORK/ffmpeg-$FFMPEG_VERSION.tar.xz"
if [ ! -f "$TARBALL" ]; then
    echo "==> Downloading ffmpeg $FFMPEG_VERSION"
    curl -fL --retry 3 "https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz" -o "$TARBALL.part"
    mv "$TARBALL.part" "$TARBALL"
fi
ACTUAL_SHA="$(sha256sum "$TARBALL" | cut -d' ' -f1)"
echo "==> ffmpeg-$FFMPEG_VERSION.tar.xz sha256 $ACTUAL_SHA"
if [ -n "$FFMPEG_SHA256" ] && [ "$ACTUAL_SHA" != "$FFMPEG_SHA256" ]; then
    echo "error: checksum mismatch (expected $FFMPEG_SHA256)" >&2
    exit 1
fi

SRC="$WORK/ffmpeg-$FFMPEG_VERSION"
[ -d "$SRC" ] || { echo "==> Extracting"; tar -xJf "$TARBALL" -C "$WORK"; }

# Same lists as the Mac build. DST decoding is there for probing only: the
# app's own decoder (dstkit) produces the lossless DSD bitstream.
DEMUXERS="ape,wv,tta,flac,wav,aiff,mov,dsf,iff,pcm_s16le,pcm_s24le,pcm_s32le,pcm_f32le"
DECODERS="ape,wavpack,tta,alac,flac,pcm_s16le,pcm_s16be,pcm_s24le,pcm_s24be,pcm_s32le,pcm_s32be,pcm_u8,pcm_s8,pcm_f32le,pcm_f32be,pcm_f64le,dsd_lsbf,dsd_msbf,dsd_lsbf_planar,dsd_msbf_planar,dst"
ENCODERS="flac,pcm_s16le,pcm_s24le,pcm_s32le,pcm_f32le"
MUXERS="flac,wav,pcm_s16le,pcm_s24le,pcm_s32le,pcm_f32le,null,md5,hash,framemd5"
FILTERS="aresample,aformat,anull,atrim,asetnsamples,ebur128,replaygain,volumedetect,astats"
PROTOCOLS="file,pipe"

FLAGS=(
    --disable-everything
    --disable-programs --enable-ffmpeg --enable-ffprobe
    --disable-doc --disable-debug
    --disable-network --disable-autodetect
    --disable-shared --enable-static --enable-pic
    --disable-avdevice
    --disable-gpl --disable-nonfree --disable-version3
    --enable-swresample --enable-avfilter
    "--enable-protocol=$PROTOCOLS"
    "--enable-demuxer=$DEMUXERS"
    "--enable-decoder=$DECODERS"
    "--enable-encoder=$ENCODERS"
    "--enable-muxer=$MUXERS"
    "--enable-filter=$FILTERS"
    "--prefix=$WORK/install-$ARCH"
    --cc=clang
    --extra-ldflags=-static
)
[ "$ARCH" = "x86_64" ] && FLAGS+=(--disable-x86asm)

BUILD="$WORK/build-$ARCH"
mkdir -p "$BUILD"
echo "==> Configuring $ARCH"
(cd "$BUILD" && "$SRC/configure" "${FLAGS[@]}" > configure.log 2>&1) \
    || { tail -40 "$BUILD/configure.log"; tail -40 "$BUILD/ffbuild/config.log" 2>/dev/null; exit 1; }
echo "==> Building with $JOBS jobs"
make -C "$BUILD" -j"$JOBS" > "$BUILD/make.log" 2>&1 || { tail -60 "$BUILD/make.log"; exit 1; }
make -C "$BUILD" install > "$BUILD/install.log" 2>&1

cp -f "$WORK/install-$ARCH/bin/ffmpeg" "$WORK/install-$ARCH/bin/ffprobe" "$OUT/"
strip "$OUT/ffmpeg" "$OUT/ffprobe"
chmod 755 "$OUT/ffmpeg" "$OUT/ffprobe"
cp -f "$SRC/LICENSE.md" "$SRC/COPYING.LGPLv2.1" "$OUT/"

if "$OUT/ffmpeg" -version | grep -q -- "--enable-gpl"; then
    echo "error: resulting ffmpeg reports --enable-gpl; refusing to ship it" >&2
    exit 1
fi
MISSING=""
for d in ape wavpack tta alac flac dst; do
    "$OUT/ffmpeg" -hide_banner -decoders 2>/dev/null | awk '{print $2}' | grep -qx "$d" || MISSING+=" decoder:$d"
done
"$OUT/ffmpeg" -hide_banner -encoders 2>/dev/null | awk '{print $2}' | grep -qx flac || MISSING+=" encoder:flac"
"$OUT/ffmpeg" -hide_banner -filters 2>/dev/null | awk '{print $2}' | grep -qx ebur128 || MISSING+=" filter:ebur128"
[ -z "$MISSING" ] || { echo "error: built ffmpeg is missing:$MISSING" >&2; exit 1; }

{
    echo "ffmpeg $FFMPEG_VERSION"
    echo "source  https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz"
    echo "sha256  $ACTUAL_SHA"
    echo "built   $(date -u +%Y-%m-%dT%H:%M:%SZ) on Linux $ARCH with $(clang --version | head -1)"
    echo "license LGPL 2.1 or later (no GPL / non-free components)"
    echo
    echo "configure ${FLAGS[*]//$ROOT/<repo>}"
} > "$OUT/BUILD-INFO.txt"

echo "==> Done"
ls -lh "$OUT/ffmpeg" "$OUT/ffprobe"
"$OUT/ffmpeg" -version | head -1
