#!/usr/bin/env python3
"""Calibrates DINOv2 cover distances onto Vision's feature-print scale and
measures how often the two agree at the thresholds the app uses.

Input: `MacDump vision-pairs` and `drtagger-cli parity-vision-pairs` over
the same pairs.tsv (cover_pairs.py; third column = pair kind). The mapping
is a monotone (isotonic) fit of Vision distance against the raw cosine
distance, reduced to a few piecewise-linear knots; agreement is measured
on held-out pairs (two-fold) and then on all of them with the final knots.

    compare_cover.py mac.json linux.json pairs.tsv [--knots 8]
"""

import argparse
import json
import random
from collections import defaultdict

# CoverCheck early stop, cover_points' 30 and 15 point bands, the warning.
THRESHOLDS = [0.3, 0.35, 0.6, 0.95]


def isotonic(xs, ys):
    """Pool-adjacent-violators: non-decreasing fit of ys over sorted xs."""
    order = sorted(range(len(xs)), key=lambda i: xs[i])
    blocks = []  # [sum_y, count, x_min, x_max]
    for i in order:
        blocks.append([ys[i], 1, xs[i], xs[i]])
        while len(blocks) > 1 and blocks[-2][0] / blocks[-2][1] > blocks[-1][0] / blocks[-1][1]:
            s, c, lo, hi = blocks.pop()
            blocks[-1][0] += s
            blocks[-1][1] += c
            blocks[-1][3] = hi
    return [((b[2] + b[3]) / 2, b[0] / b[1], b[1]) for b in blocks]


def knots_from(fit, n):
    """n knots at equal-weight quantiles of the isotonic fit, plus (0, 0)."""
    total = sum(c for _, _, c in fit)
    pts, acc, step = [(0.0, 0.0)], 0, total / n
    target = step
    for x, y, c in fit:
        acc += c
        if acc >= target:
            if x > pts[-1][0] and y >= pts[-1][1]:
                pts.append((x, y))
            target += step
    last = fit[-1]
    if last[0] > pts[-1][0]:
        pts.append((last[0], max(last[1], pts[-1][1])))
    return pts


def apply(knots, raw):
    prev = knots[0]
    for p in knots[1:]:
        if raw <= p[0]:
            t = (raw - prev[0]) / (p[0] - prev[0])
            return prev[1] + t * (p[1] - prev[1])
        prev = p
    return prev[1] + (raw - prev[0])


def agreement(rows, knots):
    out = {}
    for th in THRESHOLDS:
        same = sum((apply(knots, r["raw"]) >= th) == (r["vision"] >= th) for r in rows)
        out[th] = (same, len(rows))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("mac")
    ap.add_argument("linux")
    ap.add_argument("pairs")
    ap.add_argument("--knots", type=int, default=8)
    args = ap.parse_args()
    kinds = {}
    for line in open(args.pairs):
        a, b, k = line.rstrip("\n").split("\t")
        kinds[(a, b)] = k
    mac = {(p["a"], p["b"]): p["distance"] for p in json.load(open(args.mac))["pairs"]}
    lin = {(p["a"], p["b"]): p["raw"] for p in json.load(open(args.linux))["pairs"]}
    rows = [
        {"kind": kinds.get(k, "?"), "vision": mac[k], "raw": lin[k]}
        for k in mac
        if k in lin and mac[k] is not None and lin[k] is not None
    ]
    print(f"{len(rows)} pair(s) measured on both sides")
    by = defaultdict(list)
    for r in rows:
        by[r["kind"]].append(r)
    q = lambda v, f: sorted(v)[min(len(v) - 1, int(f * len(v)))]
    print("kind       n     Vision p10/p50/p90       DINOv2 raw p10/p50/p90")
    for k, rs in sorted(by.items()):
        v, d = [r["vision"] for r in rs], [r["raw"] for r in rs]
        print(f"{k:8} {len(rs):5}   {q(v, .1):.3f} {q(v, .5):.3f} {q(v, .9):.3f}        {q(d, .1):.3f} {q(d, .5):.3f} {q(d, .9):.3f}")

    random.seed(3)
    random.shuffle(rows)
    halves = [rows[::2], rows[1::2]]
    print("\nheld-out agreement (fit on one half, test on the other):")
    for i in (0, 1):
        train, test = halves[i], halves[1 - i]
        knots = knots_from(isotonic([r["raw"] for r in train], [r["vision"] for r in train]), args.knots)
        ag = agreement(test, knots)
        print("  " + "  ".join(f"≥{th}: {s}/{n} ({100 * s / n:.1f}%)" for th, (s, n) in ag.items()))

    knots = knots_from(isotonic([r["raw"] for r in rows], [r["vision"] for r in rows]), args.knots)
    print("\nknots (raw → Vision):")
    print("    " + ", ".join(f"({x:.4f}, {y:.4f})" for x, y in knots))
    ag = agreement(rows, knots)
    print("agreement on all pairs: " + "  ".join(f"≥{th}: {100 * s / n:.1f}%" for th, (s, n) in ag.items()))
    print("\ndisagreements at 0.95 (the warning), by kind:")
    dis = defaultdict(lambda: [0, 0])
    for r in rows:
        m, v = apply(knots, r["raw"]) >= 0.95, r["vision"] >= 0.95
        if m != v:
            dis[r["kind"]][0 if v else 1] += 1
    for k, (missed, extra) in sorted(dis.items()):
        print(f"  {k}: Vision warns, Linux not {missed}; Linux warns, Vision not {extra}")
    print(json.dumps({"knots": knots}))


if __name__ == "__main__":
    main()
