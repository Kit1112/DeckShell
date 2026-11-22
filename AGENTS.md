# Notes for AI coding agents (Claude Code, Codex, Gemini CLI, Cursor, …)

Read [CONTRIBUTING.md](CONTRIBUTING.md) first. Short version:

- DeckShell = Rust + Slint shell for Steam Deck Gaming Mode; Steam stays the backend (talks to it over the local CDP bridge, `crates/steam-bridge`).
- Layout: `app/` UI + logic, `crates/` bridge/auth/data/system/media, `session/` Gaming-Mode hook + install/rollback, `tools/gamescope/` build + packaging.
- Build: needs a local Steam install (`DECKSHELL_STEAM`, default `~/.local/share/Steam`) and Node.js. Tests: `DECKSHELL_DRYRUN=1 cargo test --workspace` — never run tests without `DECKSHELL_DRYRUN=1`.
- Do not touch a real Deck unless the human explicitly asks; never call `steamos-session-select` directly; use `session/deck-session-restart.sh check` before any restart.
- Never commit personal data (IDs, names, IPs, MACs, avatars, Wi-Fi/Bluetooth names) or Valve/Steam client files. Use fake fixtures.
- Report honestly what was verified (unit test / PC run / real Deck). Many features are stubs — improving or documenting them is the most useful help.
- Keep changes small and focused; keep `session/rollback.sh` and the stock fallback working.
