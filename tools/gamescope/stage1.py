#!/usr/bin/env python3
"""Stage 1 (safe default): DeckShell as a normal Non-Steam game.  Zero system changes.
Talks to the RUNNING desktop Steam through its CEF debug port (127.0.0.1:8080), pure stdlib.

  stage1.py add      add the "DeckShell" shortcut (idempotent)         -> prints the appid
  stage1.py remove   remove it again
  stage1.py status   print the shortcut appid if present
Steam must be running (desktop mode).  Nothing is written by us: Steam itself edits shortcuts.vdf."""
import base64, json, os, socket, struct, sys, urllib.request

HOME = os.path.expanduser("~")
EXE = os.path.join(os.environ.get("XDG_DATA_HOME", HOME + "/.local/share"), "deckshell/bin/deckshell-run")
NAME = "DeckShell"

def ws_eval(expr):
    tabs = json.load(urllib.request.urlopen("http://127.0.0.1:8080/json", timeout=5))
    t = next((t for t in tabs if t.get("title") == "SharedJSContext"), None)
    if not t: sys.exit("SharedJSContext not found (is Steam running with .cef-enable-remote-debugging?)")
    u = t["webSocketDebuggerUrl"].split("://", 1)[1]; hostport, path = u.split("/", 1)
    host, port = hostport.split(":")
    s = socket.create_connection((host, int(port)), timeout=10)
    key = base64.b64encode(os.urandom(16)).decode()
    s.sendall(("GET /%s HTTP/1.1\r\nHost: %s\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: %s\r\nSec-WebSocket-Version: 13\r\n\r\n" % (path, hostport, key)).encode())
    buf = b""
    while b"\r\n\r\n" not in buf: buf += s.recv(4096)
    rest = buf.split(b"\r\n\r\n", 1)[1]
    def send(obj):
        d = json.dumps(obj).encode(); mask = os.urandom(4)
        hdr = bytes([0x81]); n = len(d)
        hdr += bytes([0x80 | n]) if n < 126 else bytes([0x80 | 126]) + struct.pack(">H", n) if n < 65536 else bytes([0x80 | 127]) + struct.pack(">Q", n)
        s.sendall(hdr + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(d)))
    state = {"buf": rest}
    def need(n):
        while len(state["buf"]) < n: state["buf"] += s.recv(65536)
        out, state["buf"] = state["buf"][:n], state["buf"][n:]; return out
    def recv():
        while True:
            b0, b1 = need(2); n = b1 & 0x7F
            if n == 126: n = struct.unpack(">H", need(2))[0]
            elif n == 127: n = struct.unpack(">Q", need(8))[0]
            data = need(n)
            if b0 & 0x0F == 1: return json.loads(data)
    send({"id": 1, "method": "Runtime.evaluate", "params": {"expression": expr, "returnByValue": True, "awaitPromise": True}})
    while True:
        m = recv()
        if m.get("id") == 1:
            s.close()
            r = m.get("result", {})
            if "exceptionDetails" in r: sys.exit("JS error: %s" % json.dumps(r["exceptionDetails"])[:400])
            return r.get("result", {}).get("value")

FIND = """(()=>{ try{ for(const x of (appStore.allApps||[])){ if(x.app_type===1073741824 && (x.display_name===%s || x.display_name==="deckshell-run")) return x.appid; } }catch(e){} return null; })()"""

def find():
    return ws_eval(FIND % json.dumps(NAME))

cmd = sys.argv[1] if len(sys.argv) > 1 else "status"
if cmd == "status":
    print(find() or "none")
elif cmd == "add":
    if not os.access(EXE, os.X_OK): sys.exit("missing %s (tools/gamescope/install-package.sh)" % EXE)
    cur = find()
    if cur: print("already present, appid", cur); sys.exit(0)
    appid = ws_eval("SteamClient.Apps.AddShortcut(%s,%s,%s,'')" % (json.dumps(NAME), json.dumps(EXE), json.dumps(os.path.dirname(EXE))))
    ws_eval("SteamClient.Apps.SetShortcutName(%d,%s)" % (int(appid), json.dumps(NAME)))
    print("added, appid", appid, "(name %s)" % NAME)
elif cmd == "remove":
    cur = find()
    if not cur: print("not present"); sys.exit(0)
    ws_eval("SteamClient.Apps.RemoveShortcut(%d)" % int(cur)); print("removed appid", cur)
else:
    sys.exit(__doc__)
