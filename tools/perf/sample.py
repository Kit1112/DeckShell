#!/usr/bin/env python3
"""Emit JSONL snapshots of explicitly selected Linux processes and sensors."""

import argparse
import json
import math
import os
from pathlib import Path
import re
import time


PROC = Path('/proc')
SENSORS = (
    ('/sys/class/hwmon/hwmon*', r'(temp\d+_input|power\d+_(input|average))', 'name'),
    ('/sys/class/power_supply/*', r'(type|status|online|capacity|power_now|current_now|voltage_now)', 'type'),
    ('/sys/class/drm/card[0-9]*/device', r'(gpu_busy_percent|mem_info_.*_used|pp_dpm_sclk)', 'vendor'),
)


def read(path):
    try:
        return path.read_text().strip()
    except OSError:
        return None


def fields(contents):
    return dict(line.split(':', 1) for line in (contents or '').splitlines() if ':' in line)


def list_dir(path):
    try:
        return list(path.iterdir())
    except OSError:
        return []


def group_pid(value):
    name, sep, pid = value.partition('=')
    if not sep or not re.fullmatch(r'[A-Za-z][A-Za-z0-9_-]*', name) or not re.fullmatch(r'[1-9][0-9]*', pid):
        raise argparse.ArgumentTypeError('expected LABEL=positive_PID')
    return name, int(pid)


def stat(path):
    contents = read(path / 'stat')
    return None if contents is None else contents.rsplit(')', 1)[1].split()


def process(pid, group, identities, clients):
    path = PROC / str(pid)
    before = stat(path)
    if before is None:
        return {'pid': pid, 'group': group, 'status': 'unavailable'}
    start = int(before[19])
    if identities.setdefault(pid, start) != start:
        return {'pid': pid, 'group': group, 'status': 'pid_reused'}
    status = fields(read(path / 'status'))
    smaps = fields(read(path / 'smaps_rollup'))
    gpu = {}
    for fd in list_dir(path / 'fdinfo'):
        drm = {key: value.strip() for key, value in fields(read(fd)).items() if key.startswith('drm-')}
        if 'drm-client-id' in drm:
            device = drm.get('drm-pdev') or os.path.realpath(path / 'fd' / fd.name)
            gpu[(drm.get('drm-driver'), device, drm['drm-client-id'])] = drm
    after = stat(path)
    if after is None or before[19] != after[19]:
        return {'pid': pid, 'group': group, 'status': 'changed_during_sample'}
    for key, drm in gpu.items():
        client = clients.setdefault(key, {'device': key[1], 'owners': [], 'fdinfo': drm})
        client['owners'].append(pid)
    return {
        'pid': pid, 'group': group, 'status': 'ok', 'start_ticks': start,
        'cpu_ticks': int(before[11]) + int(before[12]),
        'rss_kib': kib(status.get('VmRSS')), 'pss_kib': kib(smaps.get('Pss')),
        'drm_clients': len(gpu) or None,
    }


def kib(value):
    return int(value.split()[0]) if value else None


def sensors():
    result = {}
    for glob, pattern, name in SENSORS:
        for directory in Path('/').glob(glob.lstrip('/')):
            source = {file.name: read(file) for file in list_dir(directory) if re.fullmatch(pattern, file.name)}
            if not source:
                continue
            source['source'] = read(directory / name)
            for channel in list(source):
                if channel.endswith('_input') or channel.endswith('_average'):
                    label = read(directory / (channel.split('_')[0] + '_label'))
                    if label is not None:
                        source[channel.split('_')[0] + '_label'] = label
            result[str(directory)] = source
    return result


def system():
    cpu = read(PROC / 'stat')
    memory = fields(read(PROC / 'meminfo'))
    return {
        'cpu_ticks': [int(value) for value in cpu.splitlines()[0].split()[1:]] if cpu else None,
        'mem_total_kib': kib(memory.get('MemTotal')),
        'mem_available_kib': kib(memory.get('MemAvailable')),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--pid', type=group_pid, action='append', default=[])
    parser.add_argument('--seconds', type=int, default=60)
    parser.add_argument('--interval', type=float, default=1.0)
    args = parser.parse_args()
    pids = [pid for _, pid in args.pid]
    if len(pids) != len(set(pids)):
        parser.error('each PID must appear once')
    if args.seconds <= 0 or not math.isfinite(args.interval) or not 0.1 <= args.interval <= args.seconds:
        parser.error('require 0.1 <= interval <= seconds')

    identities = {}
    start = time.monotonic()
    target = start
    ticks = os.sysconf('SC_CLK_TCK')
    while True:
        now = time.monotonic()
        clients = {}
        print(json.dumps({
            'monotonic_s': now, 'unix_s': time.time(), 'ticks_per_s': ticks,
            'processes': [process(pid, name, identities, clients) for name, pid in args.pid],
            'drm_clients': list(clients.values()), 'sensors': sensors(), 'system': system(),
        }), flush=True)
        if target >= start + args.seconds or time.monotonic() >= start + args.seconds:
            break
        target = min(start + args.seconds, max(target + args.interval, time.monotonic()))
        time.sleep(max(0, target - time.monotonic()))


if __name__ == '__main__':
    main()
