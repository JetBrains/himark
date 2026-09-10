#!/usr/bin/env python3
"""Summarizes perf.csv: recent values per (profile, bench, metric) against
the median of prior history, flagging drift. Run before committing —
`git diff perf.csv` shows the raw rows, this shows what they mean.

    tools/perf-trend.py            # summary, flag persistent >35% drift
    tools/perf-trend.py --strict   # exit 1 when anything is flagged

A single hot sample (parallel builds, thermals) does not flag: drift
counts only when the last TWO runs both exceed tolerance over the prior
median, and the absolute delta clears a noise floor.
"""

import csv
import pathlib
import statistics
import sys

TOLERANCE = 0.35
NOISE_FLOOR_MS = 0.05
PERSISTENCE = 2

rows = list(csv.DictReader(pathlib.Path("perf.csv").open()))
series: dict[tuple[str, str, str], list[float]] = {}
for row in rows:
    key = (row["profile"], row["bench"], row["metric"])
    series.setdefault(key, []).append(float(row["value_ms"]))

flagged = []
for (profile, bench, metric), values in sorted(series.items()):
    latest = values[-1]
    recent = values[-PERSISTENCE:]
    history = values[: -len(recent)]
    if not history:
        print(f"  new    {profile:7} {bench}.{metric} = {latest:.3f}ms (n={len(values)})")
        continue
    reference = statistics.median(history)
    delta = (latest - reference) / reference if reference > 0 else 0.0
    drifted = len(values) > PERSISTENCE and all(
        value > reference * (1.0 + TOLERANCE) and value - reference > NOISE_FLOOR_MS
        for value in recent
    )
    marker = "!" if drifted else " "
    print(
        f"  {marker} {profile:7} {bench}.{metric}: "
        f"median {reference:.3f}ms -> latest {latest:.3f}ms ({delta:+.0%}, n={len(values)})"
    )
    if drifted:
        flagged.append(f"{profile}/{bench}.{metric}")

if flagged:
    print(f"\nDRIFT ({len(flagged)}): " + ", ".join(flagged))
    if "--strict" in sys.argv:
        sys.exit(1)
else:
    print("\nno drift")
