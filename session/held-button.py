#!/usr/bin/env python3
"""Boot-time kill switch.  Exit 0 if VOLUME-DOWN or B (or Esc, B in lizard mode) was held for
HOLD seconds within the first WINDOW seconds; exit 1 otherwise.  Never raises, never blocks longer than WINDOW.

  held-button.py [--window 5] [--hold 2] [--test]       (--test: list devices + current key state, exit 1)

Reads RAW evdev (works in lizard mode and before Steam owns the controller; hidraw is not needed):
  /dev/input/by-path/*event-kbd    controller keyboard (lizard: B = Esc) + AT keyboard (volume keys)
  /dev/input/by-path/*event        gamepad-style event nodes (BTN_EAST = B)
Devices that cannot be opened yet (ACL/uaccess appears a moment after login, or the node appears late)
are retried every 0.25 s during the whole window.  A key already held when the script starts counts from t=0.
Env: DECKSHELL_KS_GLOB="/dev/input/by-path/*event*"  (tests: point it at a uinput device node symlink dir)"""
import fcntl, glob, os, select, struct, sys, time

KEYS = {114: "VOLUMEDOWN", 1: "ESC(B in lizard mode)", 305: "BTN_EAST(B)"}
EV_KEY = 1
EVIOCGKEY = 0x80604518          # _IOR('E', 0x18, 96 bytes)
EVSZ = struct.calcsize("llHHi")  # 24 on x86_64

def log(msg):
    sys.stderr.write("[held-button] %s\n" % msg)
    sys.stderr.flush()

def arg(name, default):
    if name in sys.argv:
        try: return float(sys.argv[sys.argv.index(name) + 1])
        except Exception: pass
    return default

def candidates():
    pat = os.environ.get("DECKSHELL_KS_GLOB", "/dev/input/by-path/*event*")
    out = {}
    for p in sorted(glob.glob(pat)):
        if "mouse" in p or "-spkr" in p or "lid" in p.lower() or "PWRBN" in p or "PNP0C0" in p:
            continue
        out[os.path.realpath(p)] = p
    return out

def snapshot(fd):
    buf = bytearray(96)
    try:
        fcntl.ioctl(fd, EVIOCGKEY, buf)
    except OSError:
        return set()
    return {k for k in KEYS if buf[k // 8] >> (k % 8) & 1}

def main():
    window, hold, test = arg("--window", 5.0), arg("--hold", 2.0), "--test" in sys.argv
    t0 = time.monotonic()
    fds = {}            # real path -> fd
    since = {}          # key code -> time first seen held
    last_scan = -1.0
    while True:
        now = time.monotonic()
        if now - last_scan >= 0.25:
            last_scan = now
            for real, shown in candidates().items():
                if real in fds:
                    continue
                try:
                    fd = os.open(real, os.O_RDONLY | os.O_NONBLOCK)
                except OSError:
                    continue
                fds[real] = fd
                held = snapshot(fd)
                log("opened %s (%s) held=%s" % (shown, real, sorted(KEYS[k] for k in held)))
                for k in held:
                    since.setdefault(k, t0)
        if test:
            time.sleep(0.2)
            if now - t0 > window:
                log("test done: opened %d device(s)" % len(fds)); return 1
        for k, t in list(since.items()):
            if now - t >= hold:
                log("TRIGGERED: %s held %.1fs" % (KEYS[k], now - t)); return 0
        if now - t0 > window:
            return 1
        if fds:
            try:
                r, _, _ = select.select(list(fds.values()), [], [], 0.05)
            except OSError:
                r = []
            for fd in r:
                try:
                    data = os.read(fd, EVSZ * 64)
                except OSError:
                    continue
                for off in range(0, len(data) - EVSZ + 1, EVSZ):
                    _s, _u, typ, code, val = struct.unpack_from("llHHi", data, off)
                    if typ == EV_KEY and code in KEYS:
                        if val:
                            since.setdefault(code, time.monotonic())
                        else:
                            since.pop(code, None)
        else:
            time.sleep(0.05)

if __name__ == "__main__":
    try:
        rc = main()
    except Exception as e:      # a broken kill switch must never block boot
        log("error: %r" % (e,)); rc = 1
    sys.exit(rc)
