#!/usr/bin/env python3
"""Diffs `MacDump identify` against `drtagger-cli parity-identify`.

Albums are matched by folder name. Everything is compared exactly (floats
within --tolerance) except the raw fingerprints, which are compared by the
share of equal bits: Mac computes them with vDSP's FFT, Linux with KissFFT.

    compare_identify.py mac-identify.json linux-identify.json [--tolerance 1e-9]
"""

import argparse
import json
import sys


def deep_diff(a, b, path, out, tol):
    if isinstance(a, bool) or isinstance(b, bool):
        if a is not b:
            out.append((path, a, b))
    elif isinstance(a, (int, float)) and isinstance(b, (int, float)):
        if abs(a - b) > tol * max(1.0, abs(a), abs(b)):
            out.append((path, a, b))
    elif isinstance(a, dict) and isinstance(b, dict):
        for k in sorted(set(a) | set(b)):
            if k not in a:
                out.append((f"{path}.{k}", "<absent>", b[k]))
            elif k not in b:
                out.append((f"{path}.{k}", a[k], "<absent>"))
            else:
                deep_diff(a[k], b[k], f"{path}.{k}", out, tol)
    elif isinstance(a, list) and isinstance(b, list):
        if len(a) != len(b):
            out.append((f"{path}[len]", len(a), len(b)))
        for i, (x, y) in enumerate(zip(a, b)):
            deep_diff(x, y, f"{path}[{i}]", out, tol)
    elif a != b:
        out.append((path, a, b))


def bits_equal(a, b):
    n = min(len(a), len(b))
    if n == 0:
        return 0.0
    errors = sum(bin((x ^ y) & 0xFFFFFFFF).count("1") for x, y in zip(a[:n], b[:n]))
    return 1 - errors / (32 * n)


def short(v):
    s = json.dumps(v, ensure_ascii=False)
    return s if len(s) <= 160 else s[:157] + "..."


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("mac")
    ap.add_argument("linux")
    ap.add_argument("--tolerance", type=float, default=1e-9)
    ap.add_argument("--limit", type=int, default=12)
    args = ap.parse_args()
    mac = json.load(open(args.mac))
    linux = json.load(open(args.linux))
    replay = linux.get("replay", {})
    misses = replay.get("misses", [])
    sims = replay.get("acoustidSimilarities", [])
    print(f"replay: {len(misses)} request(s) without a recording, {len(sims)} AcoustID lookup(s) matched by similarity"
          + (f" (min {min(sims):.4f})" if sims else ""))
    for m in misses[: args.limit]:
        print("  miss:", m.splitlines()[0])

    lin = {a["folder"]: a for a in linux["albums"]}
    total_diffs = 0
    fp_tracks = fp_identical = 0
    fp_min = 1.0
    for m in mac["albums"]:
        name = m["folder"]
        l = lin.pop(name, None)
        if l is None:
            print(f"✗ {name}: missing on Linux")
            total_diffs += 1
            continue
        diffs = []
        for key in ["toc", "ctdb", "ctdbError", "result", "tags", "artwork"]:
            deep_diff(m.get(key), l.get(key), key, diffs, args.tolerance)
        mr, lr = m.get("rawFingerprints") or [], l.get("rawFingerprints") or []
        if len(mr) != len(lr):
            diffs.append(("rawFingerprints[len]", len(mr), len(lr)))
        for i, (x, y) in enumerate(zip(mr, lr)):
            if (x is None) != (y is None):
                diffs.append((f"rawFingerprints[{i}]", x is None and "none", y is None and "none"))
                continue
            if x is None:
                continue
            for k in ["sampleRate", "channels", "duration"]:
                if x[k] != y[k]:
                    diffs.append((f"rawFingerprints[{i}].{k}", x[k], y[k]))
            fp_tracks += 1
            s = bits_equal(x["raw"], y["raw"])
            fp_identical += x["raw"] == y["raw"]
            fp_min = min(fp_min, s)
        res = m["result"]
        best = res["candidates"][0] if res["candidates"] else None
        what = (f"{best['candidate']['artist']} – {best['candidate']['title']} {best['confidence']} "
                f"{best['score']:.1f}") if best else "no candidates"
        n = len(res["candidates"])
        if diffs:
            total_diffs += len(diffs)
            print(f"✗ {name}: {len(diffs)} difference(s); Mac best {what}, {n} candidate(s)")
            for p, a, b in diffs[: args.limit]:
                print(f"    {p}\n      mac:   {short(a)}\n      linux: {short(b)}")
        else:
            print(f"✓ {name}: identical — {what}, {n} candidate(s), {len(res['log'])} log line(s)")
    for name in lin:
        print(f"✗ {name}: only on Linux")
        total_diffs += 1
    if fp_tracks:
        print(f"fingerprints: {fp_tracks} track(s), {fp_identical} bit-identical, lowest bit agreement {fp_min:.4f}")
    print("OK" if total_diffs == 0 and not misses else f"{total_diffs} difference(s)")
    sys.exit(0 if total_diffs == 0 and not misses else 1)


if __name__ == "__main__":
    main()
