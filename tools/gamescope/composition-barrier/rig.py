#!/usr/bin/env python3
"""Deterministic Xvfb rig: real Xlib requests, independent compositor model.

Each request is followed by XSync -> pipe ACK -> observer read -> next request.
Thus no request boundary can disappear through PropertyNotify coalescing. This
is an adversarial compositor schedule, NOT a measurement of actual display frames.
All five native SetComposition writes are permuted, with gaps 0..20 ms.
The bridge is intentionally absent. A monotonic timestamp accompanies every frame.
"""
import argparse
import ctypes as C
import itertools
import json
import os
from pathlib import Path
import random
import subprocess
import time

SHELL = 233594385
STEAM = 769
QUEUE = "GAMESCOPECTRL_BASELAYER_APPID"
PIN = "GAMESCOPECTRL_BASELAYER_WINDOW"
GRANT = "_DECKSHELL_EXPERIMENTAL_COMPOSITION_GRANT"
OVERLAY = "STEAM_OVERLAY"
INPUT = "STEAM_INPUT_FOCUS"
COMMITS = "_XWAYLAND_ALLOW_COMMITS"
NOTIFICATION = "STEAM_NOTIFICATION"


class X11:
    def __init__(self):
        self.x = C.CDLL("libX11.so.6")
        signatures = {
            "XOpenDisplay": (C.c_void_p, [C.c_char_p]),
            "XDefaultRootWindow": (C.c_ulong, [C.c_void_p]),
            "XInternAtom": (C.c_ulong, [C.c_void_p, C.c_char_p, C.c_int]),
            "XChangeProperty": (C.c_int, [C.c_void_p, C.c_ulong, C.c_ulong,
                C.c_ulong, C.c_int, C.c_int, C.c_void_p, C.c_int]),
            "XDeleteProperty": (C.c_int, [C.c_void_p, C.c_ulong, C.c_ulong]),
            "XGetWindowProperty": (C.c_int, [C.c_void_p, C.c_ulong, C.c_ulong,
                C.c_long, C.c_long, C.c_int, C.c_ulong, C.POINTER(C.c_ulong),
                C.POINTER(C.c_int), C.POINTER(C.c_ulong), C.POINTER(C.c_ulong),
                C.POINTER(C.c_void_p)]),
            "XSync": (C.c_int, [C.c_void_p, C.c_int]),
            "XFree": (C.c_int, [C.c_void_p]),
            "XCloseDisplay": (C.c_int, [C.c_void_p]),
        }
        for name, (result, args) in signatures.items():
            f = getattr(self.x, name)
            f.restype, f.argtypes = result, args
        self.d = self.x.XOpenDisplay(None)
        if not self.d:
            raise RuntimeError("cannot open isolated X display")
        self.root = self.x.XDefaultRootWindow(self.d)
        self.atoms = {}

    def atom(self, name):
        if name not in self.atoms:
            self.atoms[name] = self.x.XInternAtom(self.d, name.encode(), 0)
        return self.atoms[name]

    def put(self, window, name, values, mode=0):
        data = (C.c_ulong * len(values))(*values)
        self.x.XChangeProperty(self.d, window, self.atom(name), 6, 32, mode,
                               data, len(values))  # XA_CARDINAL = 6
        self.x.XSync(self.d, 0)

    def delete(self, window, name):
        self.x.XDeleteProperty(self.d, window, self.atom(name))
        self.x.XSync(self.d, 0)

    def get(self, window, name):
        kind, fmt, n, remaining = C.c_ulong(), C.c_int(), C.c_ulong(), C.c_ulong()
        data = C.c_void_p()
        rc = self.x.XGetWindowProperty(self.d, window, self.atom(name), 0, 64, 0,
            6, C.byref(kind), C.byref(fmt), C.byref(n), C.byref(remaining), C.byref(data))
        assert rc == 0 and remaining.value == 0
        try:
            if kind.value == 0:
                return []
            assert kind.value == 6 and fmt.value == 32
            return list(C.cast(data, C.POINTER(C.c_ulong))[:n.value])
        finally:
            if data.value:
                self.x.XFree(data)


class Writer:
    def __init__(self, binary, library, guarded):
        env = os.environ.copy()
        env.pop("LD_PRELOAD", None)
        if guarded:
            env["LD_PRELOAD"] = str(library)
        self.p = subprocess.Popen([str(binary)], env=env, text=True,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, bufsize=1)
        assert self.p.stdout.readline().strip() == "ready"
        self.window = int(self.command("new"))

    def command(self, text):
        self.p.stdin.write(text + "\n")
        self.p.stdin.flush()
        result = self.p.stdout.readline().strip()
        assert result, f"writer died: {self.p.poll()}"
        return result

    def write(self, name, values, root=False, mode=0):
        target = 0 if root else self.window
        assert self.command(f"{name} {target} {mode} {len(values)} " +
                            " ".join(map(str, values))) == "ok"

    def close(self):
        self.p.stdin.close()
        assert self.p.wait(timeout=5) == 0


class Observer:
    def __init__(self, x, trace):
        self.x, self.trace = x, trace
        self.frames = self.bad = 0

    def frame(self, writer, shell_exists, context):
        x, w = self.x, writer.window
        queue, pin = x.get(x.root, QUEUE), x.get(x.root, PIN)
        overlay, focus = x.get(w, OVERLAY), x.get(w, INPUT)
        commits, notification = x.get(w, COMMITS), x.get(w, NOTIFICATION)
        # SteamControlled: explicit XID precedes the available-app queue.
        # 413091 is deliberately absent, exactly as in seq 452/455.
        apps = {STEAM} | ({SHELL} if shell_exists else set())
        base = STEAM if pin == [w] else next((a for a in queue if a in apps), None)
        steam_overlay = bool(overlay and overlay[0])
        input_app = STEAM if steam_overlay and focus and focus[0] else base
        # Conservatively count an overlay even with commits=0: an older buffer
        # may already exist. ALLOW_COMMITS is not treated as a focus prohibition.
        bad = base == STEAM or input_app == STEAM or steam_overlay
        frame = dict(context, monotonic_ns=time.monotonic_ns(), base=base,
            input=input_app, queue=queue, pin=pin, overlay=overlay, focus=focus,
            commits=commits, notification=notification, unsafe=bad)
        if self.trace:
            self.trace.write(json.dumps(frame, separators=(",", ":")) + "\n")
        self.frames += 1
        self.bad += bad
        return bad


def reset(x, writer):
    x.put(x.root, QUEUE, [SHELL])
    x.put(x.root, PIN, [0])
    for name in (OVERLAY, INPUT, COMMITS, NOTIFICATION):
        x.put(writer.window, name, [0])


def matrix(args, x, trace, mode):
    guarded = mode == "xlib-guard"
    writer = Writer(args.writer, args.library, guarded)
    observer = Observer(x, trace)
    x.delete(x.root, GRANT)  # no controller / no bridge: deny from first request
    orders = list(itertools.permutations(range(5)))
    random.Random(args.seed).shuffle(orders)
    cases = 0
    try:
        for phase in ("before-ui-ready", "bridge-unavailable", "client-restart"):
            for delay in range(21):
                if phase == "client-restart":
                    writer.close()
                    writer = Writer(args.writer, args.library, guarded)
                for order in orders:
                    for profile in ("historical", "overlay-pin"):
                        reset(x, writer)
                        # Historical profile: overlay=0, notification=0, input=1,
                        # queue=[413091,769], pin=0. Also exercise interactive overlay
                        # and nonzero window pin in a separate adversarial profile.
                        writes = [(OVERLAY, [0 if profile == "historical" else 1], False),
                                  (NOTIFICATION, [0], False), (INPUT, [1], False),
                                  (QUEUE, [413091, STEAM], True),
                                  (PIN, [0 if profile == "historical" else writer.window], True)]
                        for step, index in enumerate(order):
                            name, values, root = writes[index]
                            # Delays are virtual by default: every prefix is observed,
                            # so all compositor placements inside each gap are covered.
                            # --wall-delays additionally exercises OS waits, once per case.
                            if args.wall_delays and profile == "historical" and step == 3:
                                time.sleep(delay / 1000)
                            writer.write(name, values, root)
                            context = dict(mode=mode, case=cases, phase=phase,
                                profile=profile, delay_ms=delay, virtual_ms=step * delay,
                                order=order, step=step, write=name)
                            unsafe = observer.frame(writer, phase != "before-ui-ready", context)
                            if guarded:
                                assert not unsafe, context
                            if mode == "property-repair" and root:
                                # Give the compositor its turn BEFORE notification repair.
                                x.put(x.root, QUEUE, [SHELL])
                                observer.frame(writer, phase != "before-ui-ready",
                                    dict(context, write="repair-APPID"))
                        # Attempt to re-enable commits after all composition writes.
                        writer.write(COMMITS, [1])
                        unsafe = observer.frame(writer, phase != "before-ui-ready",
                            dict(context, step=5, write=COMMITS))
                        if guarded:
                            assert not unsafe
                            assert x.get(writer.window, COMMITS) == [0]
                            assert x.get(writer.window, INPUT) == [0]
                        cases += 1
        assert (observer.bad == 0) == guarded
        return dict(mode=mode, cases=cases, frames=observer.frames,
                    unsafe_frames=observer.bad, seed=args.seed)
    finally:
        writer.close()


def boundaries(args, x, trace):
    """Positive delegation and mandatory bypass counterexample; no inflated claims."""
    writer = Writer(args.writer, args.library, True)
    obs = Observer(x, trace)
    context = dict(mode="boundaries")
    try:
        reset(x, writer)
        x.delete(x.root, GRANT)
        # All write modes, zero-length values and deletes must remain harmless.
        for mode in (-1, 0, 1, 2):
            for name, root in ((QUEUE, True), (PIN, True), (OVERLAY, False),
                               (INPUT, False), (COMMITS, False)):
                writer.write(name, [STEAM] if root else [1], root, mode)
                assert not obs.frame(writer, True, dict(context, mode_value=mode, write=name))
        writer.write(INPUT, [])
        assert x.get(writer.window, INPUT) == [0]
        # Legitimate notifications and unprotected game identity pass unchanged.
        writer.write(NOTIFICATION, [1])
        assert x.get(writer.window, NOTIFICATION) == [1]
        writer.write("STEAM_GAME", [STEAM])
        assert x.get(writer.window, "STEAM_GAME") == [STEAM]
        # Invalid grants deny, including a prefix [1] followed by extra data.
        for grant in ([], [0], [2], [1, 0]):
            x.put(x.root, GRANT, grant)
            writer.write(QUEUE, [STEAM], True)
            assert not obs.frame(writer, True, dict(context, grant=grant))
        x.put(x.root, GRANT, [1])
        writer.write(QUEUE, [STEAM], True)
        assert obs.frame(writer, True, dict(context, write="explicit-delegation"))
        # Revocation alone cannot retract already-published state.
        x.delete(x.root, GRANT)
        assert obs.frame(writer, True, dict(context, write="revoke-without-reclaim"))
        reset(x, writer)
        writer.write(QUEUE, [STEAM], True)
        assert not obs.frame(writer, True, dict(context, write="after-reclaim"))
        # Same preloaded process, raw XCB: demonstrates that this is NOT an
        # absolute barrier. It must deterministically break the invariant.
        assert writer.command("xcb") == "ok"
        assert obs.frame(writer, True, dict(context, write="xcb-bypass"))
        return dict(delegation_pass=True, revoke_needs_reclaim=True,
                    xcb_bypass_reproduced=True)
    finally:
        writer.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--writer", type=Path, required=True)
    parser.add_argument("--library", type=Path, required=True)
    parser.add_argument("--seed", type=int, default=415)
    parser.add_argument("--trace", type=Path, required=True)
    parser.add_argument("--wall-delays", action="store_true")
    args = parser.parse_args()
    x = X11()
    try:
        with args.trace.open("w") as trace:
            for mode in ("no-barrier", "property-repair", "xlib-guard"):
                print(json.dumps(matrix(args, x, trace, mode)), flush=True)
            print(json.dumps(boundaries(args, x, trace)), flush=True)
    finally:
        x.x.XCloseDisplay(x.d)


if __name__ == "__main__":
    main()
