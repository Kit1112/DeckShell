#!/usr/bin/env python3
"""Summarize one raw MangoHud/MangoApp CSV with frametime in milliseconds."""

import argparse
import csv
import json
import math
from pathlib import Path
from statistics import mean


def frames(path):
    with path.open(newline='', encoding='utf-8-sig') as stream:
        rows = csv.reader(stream)
        for row in rows:
            names = [cell.strip().lower() for cell in row]
            if 'fps' in names and 'frametime' in names:
                index = names.index('frametime')
                break
        else:
            raise ValueError('missing fps,frametime header (supply the raw CSV)')
        rejected = 0
        times = []
        for row in rows:
            if not row:
                continue
            try:
                value = float(row[index])
            except (ValueError, IndexError):
                rejected += 1
                continue
            if value > 0 and math.isfinite(value):
                times.append(value)
            else:
                rejected += 1
    if not times:
        raise ValueError('no positive finite frametime samples')
    return sorted(times), rejected


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('csv', type=Path)
    parser.add_argument('--refresh-hz', type=float, default=60.0)
    args = parser.parse_args()
    if not math.isfinite(args.refresh_hz) or args.refresh_hz <= 0:
        parser.error('refresh-hz must be positive and finite')
    try:
        times, rejected = frames(args.csv)
    except (OSError, ValueError) as error:
        parser.exit(1, f'{error}\n')
    size = len(times)
    budget = 1000 / args.refresh_hz
    result = {
        'rows': size, 'rejected': rejected,
        'fps_from_mean_interval': 1000 / mean(times),
        'one_percent_low_fps': 1000 / mean(times[-math.ceil(size / 100):]),
        'frametime_ms': {f'p{p}': times[math.ceil(size * p / 100) - 1] for p in (50, 95, 99)},
        'budget_ms': budget, 'over_budget': sum(value > budget for value in times),
        'over_33_33_ms': sum(value > 1000 / 30 for value in times),
    }
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
