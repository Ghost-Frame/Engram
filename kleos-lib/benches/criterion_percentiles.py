#!/usr/bin/env python3
"""Compute per-iteration p50/p95/p99 from criterion sample.json files.

Criterion's Linear sampling runs iters[i] iterations per sample i and
records total wall time times[i]. Per-iter time for sample i is
times[i] / iters[i]. This script walks a criterion target dir and
prints the p50/p95/p99 triplet for every benchmark.

Usage:
    python3 criterion_percentiles.py <criterion_dir>

Example:
    python3 criterion_percentiles.py target/criterion
"""
from __future__ import annotations

import json
import sys
from pathlib import Path


def fmt_ns(ns: float) -> str:
    if ns < 1_000:
        return f"{ns:.1f} ns"
    if ns < 1_000_000:
        return f"{ns / 1_000:.2f} us"
    if ns < 1_000_000_000:
        return f"{ns / 1_000_000:.2f} ms"
    return f"{ns / 1_000_000_000:.2f} s"


def percentile(values: list[float], p: float) -> float:
    n = len(values)
    if n == 0:
        return float("nan")
    idx = min(n - 1, int(p * n / 100))
    return values[idx]


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: criterion_percentiles.py <criterion_dir>", file=sys.stderr)
        return 2
    root = Path(sys.argv[1])
    if not root.is_dir():
        print(f"not a directory: {root}", file=sys.stderr)
        return 2
    rows: list[tuple[str, int, float, float, float]] = []
    for sample_path in sorted(root.rglob("new/sample.json")):
        try:
            with sample_path.open() as f:
                s = json.load(f)
        except (OSError, json.JSONDecodeError) as e:
            print(f"skip {sample_path}: {e}", file=sys.stderr)
            continue
        iters = s.get("iters", [])
        times = s.get("times", [])
        if not iters or len(iters) != len(times):
            continue
        per = sorted(t / i for t, i in zip(times, iters) if i > 0)
        if not per:
            continue
        rel = sample_path.relative_to(root).parent.parent
        rows.append(
            (
                str(rel),
                len(per),
                percentile(per, 50),
                percentile(per, 95),
                percentile(per, 99),
            )
        )
    if not rows:
        print("no sample.json files found", file=sys.stderr)
        return 1
    name_w = max(len(r[0]) for r in rows)
    for name, n, p50, p95, p99 in rows:
        print(
            f"{name:<{name_w}}  n={n:<3}  p50={fmt_ns(p50):<10}  "
            f"p95={fmt_ns(p95):<10}  p99={fmt_ns(p99)}"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
