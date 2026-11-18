#!/usr/bin/env python3
"""Builds the cover-similarity corpus for the Vision vs DINOv2 benchmark.

For every album under ROOT whose FLACs carry MusicBrainz IDs, takes the
album's own front image and downloads, from Cover Art Archive, the front
images CoverCheck would compare it with (the `small` thumbnail, as the app
does) for:

  own        the album's release
  edition    other releases of the same release group
  sibling    releases of other release groups by the same artist (series)
  other      the album's own front against other albums' fronts

Writes OUT/images/… and OUT/pairs.tsv ("a<TAB>b<TAB>kind", paths relative
to the common root given to MacDump vision-pairs / parity-vision-pairs:
local files as "local/<path under ROOT>", downloads as "caa/<file>").

    cover_pairs.py ROOT OUT [--albums N] [--editions 3] [--siblings 3]
"""

import argparse
import json
import os
import random
import re
import struct
import sys
import time
import urllib.request

UA = "drtagger-parity/0.1 (+https://drtagger.priet.us)"
FRONT_NAMES = ["folder", "cover", "front", "frontal", "cover_front", "cover front", "portada"]
IMAGE_EXT = (".jpg", ".jpeg", ".png", ".tif", ".tiff", ".bmp", ".webp")
_last = [0.0]


def get(url, binary=False):
    wait = 1.05 - (time.time() - _last[0])
    if wait > 0 and "musicbrainz.org" in url:
        time.sleep(wait)
    _last[0] = time.time()
    req = urllib.request.Request(url, headers={"User-Agent": UA, "Accept": "application/json"})
    for attempt in range(3):
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                data = r.read()
                return data if binary else json.loads(data)
        except urllib.error.HTTPError as e:
            if e.code == 404:
                return None
            if e.code in (503, 502) and attempt < 2:
                time.sleep(2)
                continue
            return None
        except Exception:
            if attempt < 2:
                time.sleep(2)
                continue
            return None


def vorbis(path):
    """Vorbis comments of a FLAC file, upper-cased keys."""
    tags = {}
    with open(path, "rb") as f:
        if f.read(4) != b"fLaC":
            return tags
        while True:
            head = f.read(4)
            if len(head) < 4:
                break
            last, kind, size = head[0] & 0x80, head[0] & 0x7F, int.from_bytes(head[1:4], "big")
            if kind == 4:
                block = f.read(size)
                n = struct.unpack_from("<I", block, 0)[0]
                pos = 4 + n
                count = struct.unpack_from("<I", block, pos)[0]
                pos += 4
                for _ in range(count):
                    ln = struct.unpack_from("<I", block, pos)[0]
                    pos += 4
                    k, _, v = block[pos:pos + ln].decode("utf-8", "replace").partition("=")
                    tags.setdefault(k.upper(), v)
                    pos += ln
                break
            f.seek(size, 1)
            if last:
                break
    return tags


def albums(root):
    for dirpath, dirnames, filenames in os.walk(root):
        flacs = sorted(f for f in filenames if f.lower().endswith(".flac"))
        if not flacs:
            continue
        dirnames[:] = []
        tags = vorbis(os.path.join(dirpath, flacs[0]))
        release = tags.get("MUSICBRAINZ_ALBUMID")
        if not release:
            continue
        images = []
        for d2, _, files in os.walk(dirpath):
            images += [os.path.join(d2, f) for f in files if f.lower().endswith(IMAGE_EXT) and not f.startswith("._")]
        front = None
        for name in FRONT_NAMES:
            for img in sorted(images):
                stem = os.path.splitext(os.path.basename(img))[0].lower()
                if stem == name or (name in stem and "back" not in stem and len(stem) < 30):
                    front = img
                    break
            if front:
                break
        if front:
            yield dirpath, front, release, tags.get("MUSICBRAINZ_RELEASEGROUPID"), tags.get("MUSICBRAINZ_ALBUMARTISTID")


def caa_front(release, out_dir):
    """Downloads the release's first front image ('small'); its file name."""
    name = f"{release}.jpg"
    path = os.path.join(out_dir, "caa", name)
    if os.path.exists(path):
        return name if os.path.getsize(path) else None
    listing = get(f"https://coverartarchive.org/release/{release}")
    url = None
    for img in (listing or {}).get("images", []):
        if img.get("front"):
            th = img.get("thumbnails", {})
            url = th.get("small") or th.get("250") or img.get("image")
            break
    data = get(url, binary=True) if url else None
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(data or b"")
    return name if data else None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("root")
    ap.add_argument("out")
    ap.add_argument("--albums", type=int, default=10000)
    ap.add_argument("--editions", type=int, default=3)
    ap.add_argument("--siblings", type=int, default=3)
    ap.add_argument("--others", type=int, default=3)
    args = ap.parse_args()
    random.seed(7)
    found = list(albums(args.root))[: args.albums]
    print(f"{len(found)} album(s) with a front image and a release ID", file=sys.stderr)
    rows = []
    fronts = []
    for i, (folder, front, release, group, artist) in enumerate(found):
        local = "local/" + os.path.relpath(front, args.root)
        fronts.append(local)
        print(f"[{i + 1}/{len(found)}] {os.path.basename(folder)}", file=sys.stderr)
        own = caa_front(release, args.out)
        if own:
            rows.append((local, "caa/" + own, "own"))
        if group:
            rels = (get(f"https://musicbrainz.org/ws/2/release?release-group={group}&limit=100&fmt=json") or {}).get("releases", [])
            others = [r["id"] for r in rels if r["id"] != release]
            random.shuffle(others)
            n = 0
            for rid in others:
                if n >= args.editions:
                    break
                name = caa_front(rid, args.out)
                if name:
                    rows.append((local, "caa/" + name, "edition"))
                    n += 1
        if artist and artist != "89ad4ac3-39f7-470e-963a-56509c546377":  # Various Artists
            groups = (get(f"https://musicbrainz.org/ws/2/release-group?artist={artist}&type=album&limit=100&fmt=json") or {}).get("release-groups", [])
            groups = [g["id"] for g in groups if g["id"] != group]
            random.shuffle(groups)
            n = 0
            for gid in groups[: args.siblings * 3]:
                if n >= args.siblings:
                    break
                rels = (get(f"https://musicbrainz.org/ws/2/release?release-group={gid}&limit=5&fmt=json") or {}).get("releases", [])
                for r in rels:
                    name = caa_front(r["id"], args.out)
                    if name:
                        rows.append((local, "caa/" + name, "sibling"))
                        n += 1
                        break
    for a in fronts:
        for b in random.sample([f for f in fronts if f != a], min(args.others, len(fronts) - 1)):
            rows.append((a, b, "other"))
    with open(os.path.join(args.out, "pairs.tsv"), "w") as f:
        for r in rows:
            f.write("\t".join(r) + "\n")
    kinds = {}
    for r in rows:
        kinds[r[2]] = kinds.get(r[2], 0) + 1
    print(f"{len(rows)} pair(s): {kinds}", file=sys.stderr)


if __name__ == "__main__":
    main()
