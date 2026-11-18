#!/usr/bin/env python3
"""Compares `MacDump vision-scan` (Apple Vision) with `drtagger-cli
parity-vision-scan` (zxing-cpp + PaddleOCR) on the same images.

Per image and per album (what identification sees: the union over the
album's images) it reports how many of Vision's barcodes and catalog
numbers Linux also finds, and what Linux finds that Vision does not. With
--truth ROOT the albums' own BARCODE / CATALOGNUMBER tags (FLAC Vorbis
comments, read from ROOT/<album>) say which side found the real ones.

    compare_vision.py mac.json linux.json [--truth ROOT] [--list N]
"""

import argparse
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(__file__))
from cover_pairs import vorbis  # noqa: E402


def norm_cat(c):
    return re.sub(r"[^A-Z0-9]", "", c.upper())


def norm_code(c):
    c = re.sub(r"\D", "", c)
    return c.lstrip("0") or "0"


def album_truth(root, album):
    folder = os.path.join(root, album)
    for dirpath, _, files in os.walk(folder):
        for f in sorted(files):
            if f.lower().endswith(".flac"):
                t = vorbis(os.path.join(dirpath, f))
                cats = {norm_cat(c) for c in re.split(r"[;/]", t.get("CATALOGNUMBER", "")) if c.strip()}
                codes = {norm_code(c) for c in re.split(r"[;/]", t.get("BARCODE", "")) if c.strip()}
                return cats, codes
    return set(), set()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("mac")
    ap.add_argument("linux")
    ap.add_argument("--truth")
    ap.add_argument("--list", type=int, default=15)
    args = ap.parse_args()
    mac = {i["file"]: i for i in json.load(open(args.mac))["images"] if i}
    lin = {i["file"]: i for i in json.load(open(args.linux))["images"] if i}
    files = sorted(set(mac) & set(lin))
    print(f"{len(files)} image(s) on both sides ({len(mac)} Mac, {len(lin)} Linux)")

    def stats(kind, norm):
        both = only_mac = only_lin = imgs_mac = imgs_hit = 0
        examples_missed, examples_extra = [], []
        for f in files:
            m = {norm(x) for x in mac[f][kind]}
            l = {norm(x) for x in lin[f][kind]}
            both += len(m & l)
            only_mac += len(m - l)
            only_lin += len(l - m)
            if m:
                imgs_mac += 1
                imgs_hit += bool(m & l)
            if m - l:
                examples_missed.append((f, sorted(m - l), sorted(l)))
            if l - m:
                examples_extra.append((f, sorted(l - m), sorted(m)))
        return both, only_mac, only_lin, imgs_mac, imgs_hit, examples_missed, examples_extra

    for kind, norm in [("barcodes", norm_code), ("catalogNumbers", norm_cat)]:
        both, om, ol, im, ih, missed, extra = stats(kind, norm)
        total = both + om
        print(f"\n{kind} per image: Vision {total}, Linux finds {both} of them "
              f"({100 * both / max(1, total):.1f}%), Linux-only {ol}; images with a Vision hit {im}, Linux hits {ih}")
        for f, x, l in missed[: args.list]:
            print(f"  missed {x} in {f}  (linux: {l})")
        for f, x, m in extra[: args.list]:
            print(f"  extra  {x} in {f}  (vision: {m})")

    # Per album: the union, as identification sees it.
    albums = sorted({mac[f]["album"] for f in files})
    agg = {"barcodes": [0, 0, 0], "catalogNumbers": [0, 0, 0]}
    truth_rows = {"barcodes": [0, 0, 0], "catalogNumbers": [0, 0, 0]}
    for a in albums:
        fs = [f for f in files if mac[f]["album"] == a]
        truth = album_truth(args.truth, a) if args.truth else None
        for kind, norm, ti in [("barcodes", norm_code, 1), ("catalogNumbers", norm_cat, 0)]:
            m = {norm(x) for f in fs for x in mac[f][kind]}
            l = {norm(x) for f in fs for x in lin[f][kind]}
            agg[kind][0] += len(m & l)
            agg[kind][1] += len(m - l)
            agg[kind][2] += len(l - m)
            if truth and truth[ti]:
                t = truth[ti]
                truth_rows[kind][0] += 1
                truth_rows[kind][1] += bool(t & m)
                truth_rows[kind][2] += bool(t & l)
    print(f"\nper album ({len(albums)}):")
    for kind, (b, om, ol) in agg.items():
        print(f"  {kind}: Vision {b + om}, Linux finds {b} ({100 * b / max(1, b + om):.1f}%), Linux-only {ol}")
    if args.truth:
        print("against the albums' own tags (albums with the tag; found by Vision / by Linux):")
        for kind, (n, vm, vl) in truth_rows.items():
            print(f"  {kind}: {n} album(s); Vision {vm} ({100 * vm / max(1, n):.1f}%), Linux {vl} ({100 * vl / max(1, n):.1f}%)")
    tm = sum(mac[f]["ms"] for f in files) / 1000
    tl = sum(lin[f]["ms"] for f in files) / 1000
    print(f"\ntime per image (3 at once): Vision {tm / len(files):.2f} s, Linux {tl / len(files):.2f} s")


if __name__ == "__main__":
    main()
