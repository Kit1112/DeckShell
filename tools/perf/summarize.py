#!/usr/bin/python3 -I
"""Summarize perf-compare results. Local only; stdlib only.
  summarize.py proc  FILE.csv [--skip N]      per-process CPU/RSS and GPU/power stats from proc-sampler.sh
  summarize.py mango FILE.csv                 FPS / frametime percentiles from a MangoHud log (output_folder csv)
  summarize.py frames FILE.txt                frametime (ms, one per line) percentiles
"""
import csv, sys, statistics as st

def pct(v, p):
    v = sorted(v); return v[min(len(v) - 1, int((len(v) - 1) * p + 0.5))] if v else float("nan")

def line(name, v):
    if not v: return f"{name}: no data"
    return f"{name}: n={len(v)} mean={st.fmean(v):.2f} p50={pct(v,.5):.2f} p95={pct(v,.95):.2f} p99={pct(v,.99):.2f} max={max(v):.2f}"

def ft(v):
    if not v: print("no frames"); return
    print(line("frametime ms", v)); m = st.fmean(v)
    print(f"avg fps={1000/m:.1f}  1% low fps={1000/pct(v,.99):.1f}  0.1% low fps={1000/pct(v,.999):.1f}  stutter>2x median={sum(x>2*pct(v,.5) for x in v)}")

def proc(path, skip):
    rows = [r for r in csv.DictReader(open(path)) if int(r["t"]) >= skip]
    by = {}
    for r in rows: by.setdefault(r["proc"], {}).setdefault(r["t"], []).append(r)
    for name, ts in sorted(by.items()):
        cpu = [sum(float(x["cpu_pct"]) for x in v) for v in ts.values()]   # sum over pids of same name
        rss = [sum(float(x["rss_mb"] or 0) for x in v) for v in ts.values()]
        print(line(f"{name} cpu%core", cpu)); print(line(f"{name} rss MB", rss))
    seen = {}
    for r in rows: seen[r["t"]] = r
    for col in ("gpu_busy_pct", "gpu_power_w", "gpu_temp_c", "mem_used_mb"):
        v = [float(r[col]) for r in seen.values() if r[col] not in ("", None)]
        if v: print(line(col, v))

def mango(path):
    with open(path) as f:
        lines = f.read().splitlines()
    i = next(k for k, l in enumerate(lines) if l.startswith("fps,"))  # skip system-info header
    rows = list(csv.DictReader(lines[i:]))
    ft([float(r["frametime"]) for r in rows])      # MangoHud frametime column is ms
    for c in ("cpu_load", "gpu_load", "cpu_power", "gpu_power", "cpu_temp", "gpu_temp", "ram", "swap"):
        if rows and c in rows[0]:
            v = [float(r[c]) for r in rows if r[c] not in ("", "nan")]
            if v: print(line(c, v))

if __name__ == "__main__":
    a = sys.argv[1:]
    if len(a) >= 2 and a[0] == "proc":
        proc(a[1], int(a[a.index("--skip") + 1]) if "--skip" in a else 0)
    elif len(a) >= 2 and a[0] == "mango": mango(a[1])
    elif len(a) >= 2 and a[0] == "frames": ft([float(x) for x in open(a[1]) if x.strip()])
    else: print(__doc__); sys.exit(2)
