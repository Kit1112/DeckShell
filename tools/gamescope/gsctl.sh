#!/usr/bin/env bash
# gsctl.sh - read/write gamescope X atoms on the base Xwayland (DISPLAY, normally :0).
# Needs only xprop/xwininfo.  `probe` is strictly read-only; run it over SSH inside a
# real gaming-mode session to verify context/archive/docs/gamescope-integration.md (section 7).
#
#   gsctl.sh probe                 dump GAMESCOPE_*/STEAM_*/GAMESCOPECTRL_* on root + every window
#   gsctl.sh windows               id, name, STEAM_GAME/OVERLAY/INPUT_FOCUS/... per window
#   gsctl.sh fps N                 GAMESCOPE_FPS_LIMIT = N (0 = off)           [root]
#   gsctl.sh baselayer APPID...    GAMESCOPECTRL_BASELAYER_APPID               [root]
#   gsctl.sh baselayer-clear
#   gsctl.sh claim-base WINID      STEAM_GAME=0x0DEC5E11 on window (DeckShell home screen)
#   gsctl.sh overlay on|off WINID  STEAM_OVERLAY=1 + STEAM_INPUT_FOCUS=1|0 (QAM-over-game)
#   gsctl.sh ext-overlay on|off WINID   GAMESCOPE_EXTERNAL_OVERLAY (display-only, no input)
#   gsctl.sh find NAME             window ids whose _NET_WM_NAME/WM_CLASS contains NAME
#   gsctl.sh shot OUT.png [TYPE]   gamescope compositor screenshot (GAMESCOPECTRL_REQUEST_SCREENSHOT: 0=full output, 1=game-targeted, 2=UI-inclusive, default 0)
#   gsctl.sh ctl ARGS...           gamescopectl ARGS (needs GAMESCOPE_WAYLAND_DISPLAY)
set -u
export DISPLAY="${GS_DISPLAY:-${DISPLAY:-:0}}"
PAT='GAMESCOPE|STEAM_|GAMESCOPECTRL'
cmd="${1:-}"; shift || true
need() { command -v "$1" >/dev/null || { echo "need $1" >&2; exit 1; }; }
case "$cmd" in ""|ctl|-h|--help) ;; *) need xprop; need xwininfo ;; esac

winids() { xwininfo -root -tree | awk '/^ +0x[0-9a-f]+ /{print $1}'; }

setwin() { xprop -id "$1" -f "$2" 32c -set "$2" "$3"; }
setroot() { local n="$1"; shift; local IFS=,; xprop -root -f "$n" 32c -set "$n" "$*"; }

case "$cmd" in
  probe)
    echo "== DISPLAY=$DISPLAY root =="; xprop -root | grep -E "$PAT" || echo "(none: not a gamescope X server?)"
    echo "== windows =="
    for w in $(winids); do
      p="$(xprop -id "$w" 2>/dev/null | grep -E "$PAT|_NET_WM_NAME|WM_CLASS|_NET_WM_PID")"
      [ -n "$p" ] && { echo "-- $w"; echo "$p"; }
    done ;;
  windows)
    for w in $(winids); do
      xprop -id "$w" STEAM_GAME STEAM_BIGPICTURE STEAM_OVERLAY STEAM_NOTIFICATION STEAM_INPUT_FOCUS \
        GAMESCOPE_EXTERNAL_OVERLAY _NET_WM_NAME 2>/dev/null | tr '\n' ' ' | sed "s/^/$w: /; s/(not found)//g"; echo
    done ;;
  fps)        setroot GAMESCOPE_FPS_LIMIT "${1:?N}" ;;
  baselayer)  setroot GAMESCOPECTRL_BASELAYER_APPID "$@" ;;
  baselayer-clear) xprop -root -remove GAMESCOPECTRL_BASELAYER_APPID ;;
  claim-base) setwin "${1:?WINID}" STEAM_GAME 233594385 ;;
  overlay)
    v=0; [ "${1:?on|off}" = on ] && v=1
    setwin "${2:?WINID}" STEAM_OVERLAY 1; setwin "$2" STEAM_INPUT_FOCUS "$v" ;;
  ext-overlay)
    v=0; [ "${1:?on|off}" = on ] && v=1
    setwin "${2:?WINID}" GAMESCOPE_EXTERNAL_OVERLAY "$v" ;;
  find)
    for w in $(winids); do
      xprop -id "$w" _NET_WM_NAME WM_CLASS 2>/dev/null | grep -qi -- "${1:?NAME}" && echo "$w"
    done ;;
  shot)
    out="${1:?OUT.png}"; type="${2:-0}"
    case "$type" in 0|1|2) ;; *) echo "screenshot TYPE must be 0, 1 or 2" >&2; exit 2 ;; esac
    rm -f /tmp/gamescope.png
    xprop -root -f GAMESCOPECTRL_REQUEST_SCREENSHOT 32c -set GAMESCOPECTRL_REQUEST_SCREENSHOT "$type" || exit 1
    for _ in 1 2 3 4 5 6 7 8; do sleep 0.5; [ -s /tmp/gamescope.png ] && break; done
    [ -s /tmp/gamescope.png ] || { echo "gamescope produced no screenshot (type=$type)" >&2; exit 1; }
    cp -- /tmp/gamescope.png "$out" && echo "saved $out (type=$type)" ;;
  ctl) need gamescopectl; exec gamescopectl "$@" ;;
  *) sed -n '2,19p' "$0"; exit 2 ;;
esac
