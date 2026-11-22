# Composition barrier laboratory (task 415)

PC-only, separate Xvfb, fake Steam, independent fake compositor. Nothing here
installs a library into Steam, modifies Steam files, or changes the session.
The release package deliberately does **not** include or enable the experimental
interposer. The real session still has task 414's post-write recovery.

Run through the Dispatcher test wrapper:

```sh
dispatch-test -- tools/gamescope/composition-barrier/run.sh
dispatch-test -- tools/gamescope/composition-barrier/run.sh --wall-delays --seed 415415
```

Requirements: C compiler, Python 3, Xlib and XCB headers/libraries, Xvfb. Set
`XVFB=/absolute/path/to/Xvfb` if unpacked in a user cache. No root is required.
The runner allocates its own display with `-displayfd`, a random MIT cookie,
and no TCP listener; it never uses the caller's DISPLAY. It builds both C files
in a temporary directory. Traces and summaries stay outside Git, under
`~/.cache/deckshell/composition-barrier/` (override `BARRIER_RESULTS`).

## What the rig proves

`fake-steam.c` is a separate executable linked to real libX11. It creates/maps
a `STEAM_GAME=769` window and issues CARDINAL/32 ChangeProperty requests to a
real X server. The five SetComposition writes identified in task 414 are:

1. window `STEAM_OVERLAY`;
2. window `STEAM_NOTIFICATION`;
3. window `STEAM_INPUT_FOCUS`;
4. root `GAMESCOPECTRL_BASELAYER_APPID`;
5. root `GAMESCOPECTRL_BASELAYER_WINDOW`.

The historical profile uses overlay=0, input=1, queue=[413091,769], pin=0:
these values were re-read passively from PID 21266's session.log.old seq 452/455.
The notification value is **not in those snapshots**; notification=0 is the
test's opaque-composition input, consistent with the native SetComposition
path described in 414, not a claimed measurement of that property in seq 455.
An additional profile uses overlay=1 and pin=Steam XID. A sixth request tries
`_XWAYLAND_ALLOW_COMMITS=1`; its historical value during the race is unknown.

Every profile runs all 120 permutations, all delays 0..20 ms, and three phases
(shell not yet available, shell available without a bridge, client restart).
This is 15,120 cases per mode. The bridge is absent throughout. The restart
phase actually replaces the client process/connection/window for each delay.
No real Steam or controller is involved; these phase names are fixture states,
not a simulation of the full DeckShell/Steam lifecycle.

The writer waits for instructions after **each** request and `XSync`. The
observer then reads every relevant property over its own connection and records
a frame before permitting the next write. No PropertyNotify coalescing can hide
a request boundary. This forces an adversarial compositor turn after every
publication. A frame means a complete property-state checkpoint, **not a video
frame or a report from actual Gamescope**. The fake compositor models explicit
window priority, skipping unavailable appids, base/input ownership and Steam
overlay selection. A nonzero overlay counts as unsafe even when commits=0:
there could already be an older buffer. Noninteractive notifications are allowed.

Default delays use a deterministic virtual clock: all prefixes are read, and
waiting inside a prefix cannot produce another property state. `--wall-delays`
additionally sleeps the requested real 0..20 ms before the fourth write of each
historical case. It does not promise an exact Linux scheduling duration or
exhaust every relative scheduling possibility. The same prefixes are checked
in both runs; monotonic timestamps record actual observation times.

Three modes must satisfy differential assertions:

- **no-barrier** must expose Steam;
- **property-repair** lets the compositor observe first, then restores APPID
  (the best possible immediate schedule of 414's root recovery); it must expose
  Steam too. It deliberately does not claim to execute DeckShell's worker;
- **xlib-guard** must have zero unsafe frames at every checkpoint.

Separate boundary checks cover delete/replace/prepend/append, empty data,
malformed grants, notification passthrough, explicit delegation, revocation
without reclaim, and a **mandatory successful XCB bypass**. The last two are
counterexamples to claiming an absolute production barrier.

## Experimental mechanism and its boundary

`xlib-guard.c` interposes XChangeProperty/XDeleteProperty **before** publication.
Without an exact CARDINAL/32 `[1]` in root
`_DECKSHELL_EXPERIMENTAL_COMPOSITION_GRANT`, it omits both root-control requests
and normalises the overlay/input/commit properties to CARDINAL/32 `[0]`.
It leaves notification and game-identity writes intact. Missing, deleted or
malformed grants deny from the first request, before a bridge/controller exists.
The controller must preclaim the desired root queue before launching the client.

For each protected operation, a short X server grab serialises the grant read
and request against revocation by another connection. This is not an idle grab;
the hook releases and flushes it before returning. It can still stall the whole
X server if the guarded process stalls inside the hook. No production latency
or Steam-runtime compatibility has been established.

This is a **process-local Xlib mechanism**, not X11 access control. The same
preloaded client can use raw XCB and publish a forbidden root queue, as the
boundary test demonstrates. LD_PRELOAD may also be removed by Steam's launcher
or runtime; 32-bit processes require a separate compatible library. Existing
clients/connections are not retroactively guarded. Any direct writer, Wayland
path, or unguarded child is outside the proof. Another authorised X11 client
can also change the grant. No security isolation is claimed.

Revoking a grant prevents later intercepted writes but cannot retract state
already accepted under the grant, or frames already cached by a real
compositor. Reclaim must restore both root atoms and clear the native window's
flags, and would still require a compositor acknowledgement before a strict
post-reclaim guarantee. The rig intentionally demonstrates the unsafe state
between revoke and reclaim.

Consequently **do not enable this in the real session based on the zero alone**.
An absolute barrier covering arbitrary Steam writers requires enforcement in
the compositor/X server or a complete protocol mediator. A normal byte-stream
X11 proxy would also need correct DRI3/SCM_RIGHTS handling and game/runtime
compatibility; it is not a drop-in safe replacement for DISPLAY.

See [the race report](../../../docs/steam-start-race-2026-11-18.md) for results,
remaining exposure, rejected alternatives and the owner's checklist.
