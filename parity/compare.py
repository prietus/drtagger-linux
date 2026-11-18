#!/usr/bin/env python3
"""Diff the Mac (MacDump) and Linux (drtagger-cli parity) dumps.

    python3 -I parity/compare.py mac.json linux.json \
        --root ~/mactagger-samples=/home/alarm/samples/mactagger-samples

Each --root pairs a Mac folder with its Linux mirror; file URLs and absolute
paths under them become "$ROOTn/…". Strings are compared
in NFC, and collections are matched by key (album id, CUE path, name), so
ordering differences inside a set do not count. Exit status 1 on any
difference.
"""

import argparse
import json
import os
import sys
import unicodedata
from urllib.parse import unquote


def normalizer(pairs):
    labels = {}
    for i, (m, l) in enumerate(pairs):
        labels[m.rstrip("/")] = labels[l.rstrip("/")] = f"$ROOT{i}"
    roots = sorted(labels, key=len, reverse=True)

    def norm(v):
        if isinstance(v, dict):
            return {k: norm(x) for k, x in v.items()}
        if isinstance(v, list):
            return [norm(x) for x in v]
        if isinstance(v, str):
            s = v
            if s.startswith("file://"):
                s = unquote(s[len("file://") :])
                if len(s) > 1:
                    s = s.rstrip("/")
            for r in roots:
                if s == r or s.startswith(r + "/"):
                    s = labels[r] + s[len(r) :]
                    break
            return unicodedata.normalize("NFC", s)
        return v

    return norm


def keyed(items, key):
    out = {}
    for item in items:
        out[key(item)] = item
    return out


IGNORED = set()
TOLERANCE = [0.0]


def diff(a, b, path, out):
    if isinstance(a, dict) and isinstance(b, dict):
        for k in sorted((set(a) | set(b)) - IGNORED):
            if k not in a:
                out.append((f"{path}.{k}", "<missing>", b[k]))
            elif k not in b:
                out.append((f"{path}.{k}", a[k], "<missing>"))
            else:
                diff(a[k], b[k], f"{path}.{k}", out)
    elif isinstance(a, list) and isinstance(b, list):
        if len(a) != len(b):
            out.append((f"{path} (length)", len(a), len(b)))
        for i, (x, y) in enumerate(zip(a, b)):
            diff(x, y, f"{path}[{i}]", out)
    elif (
        isinstance(a, float) or isinstance(b, float)
    ) and isinstance(a, (int, float)) and isinstance(b, (int, float)) and abs(a - b) <= TOLERANCE[0]:
        return
    elif a != b:
        out.append((path, a, b))


def compare_section(name, mac, lin, key, out):
    m, l = keyed(mac, key), keyed(lin, key)
    for k in sorted(set(m) | set(l)):
        if k not in l:
            out.append((f"{name}[{k}]", "<present>", "<missing>"))
        elif k not in m:
            out.append((f"{name}[{k}]", "<missing>", "<present>"))
        else:
            diff(m[k], l[k], f"{name}[{k}]", out)
    return len(m), len(l)


def short(v):
    s = json.dumps(v, ensure_ascii=False)
    return s if len(s) <= 160 else s[:157] + "..."


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("mac")
    ap.add_argument("linux")
    ap.add_argument("--root", action="append", default=[], help="MAC_DIR=LINUX_DIR")
    ap.add_argument("--limit", type=int, default=40)
    ap.add_argument("--tolerance", type=float, default=0.0, help="absolute tolerance for floating-point values")
    ap.add_argument("--ignore", action="append", default=[], help="a key to skip wherever it appears (e.g. sha)")
    args = ap.parse_args()
    IGNORED.update(args.ignore)
    TOLERANCE[0] = args.tolerance

    pairs = [tuple(os.path.expanduser(x) for x in r.split("=", 1)) for r in args.root]
    norm = normalizer(pairs)
    mac = norm(json.load(open(args.mac)))
    lin = norm(json.load(open(args.linux)))

    total = 0
    if "files" in mac:
        sections = [("files", mac["files"], lin["files"], lambda f: f["path"])]
    elif "albums" in mac and "scan" not in mac:
        sections = [("albums", mac["albums"], lin["albums"], lambda a: a["folder"])]
    elif "discs" in mac:
        sections = [("discs", mac["discs"], lin["discs"], lambda d: d["iso"])]
    elif "splits" in mac:
        sections = [("splits", mac["splits"], lin["splits"], lambda s: (s["album"], s["disc"]))]
    else:
        sections = [
            ("albums", mac["scan"]["albums"], lin["scan"]["albums"], lambda a: a["derived"]["id"]),
            ("issues", mac["scan"]["issues"], lin["scan"]["issues"], lambda i: (i["url"], i["message"])),
            ("cues", mac["cues"], lin["cues"], lambda c: c["path"]),
            ("names", mac["names"], lin["names"], lambda n: n["name"]),
        ]
    for name, m, l, key in sections:
        out = []
        nm, nl = compare_section(name, m, l, key, out)
        total += len(out)
        status = "OK" if not out else f"{len(out)} differences"
        print(f"{name:8} mac {nm:5}  linux {nl:5}  {status}")
        for p, x, y in out[: args.limit]:
            print(f"  {p}\n    mac:   {short(x)}\n    linux: {short(y)}")
        if len(out) > args.limit:
            print(f"  … {len(out) - args.limit} more")
    if "scan" in mac:
        fm, fl = mac["scan"]["foldersVisited"], lin["scan"]["foldersVisited"]
        print(f"folders  mac {fm:5}  linux {fl:5}  {'OK' if fm == fl else 'differ'}")
        total += fm != fl
    sys.exit(1 if total else 0)


if __name__ == "__main__":
    main()
