# Contributing / Как помочь (EN · RU)

AI-assisted contributions are **very welcome** — use Claude Code, Codex, Gemini CLI, Cursor or anything else, or write by hand.
Вклад, сделанный с помощью ваших ИИ-агентов, **крайне приветствуется** — или пишите руками.

## Quick start
1. Fork, branch, make a focused change, open a pull request. Small PRs are easier to review.
2. Build/test on a PC (no Deck needed for most work):
   ```bash
   DECKSHELL_STEAM=~/.local/share/Steam cargo build -p deckshell     # needs Rust, Node.js and a local Steam install
   DECKSHELL_DRYRUN=1 cargo test --workspace                          # DECKSHELL_DRYRUN=1 is REQUIRED
   ```
   A SteamOS-compatible binary: `tools/gamescope/build.sh` (glibc ≤ 2.34).
3. Say in the PR what you actually verified: *tests only*, *PC run*, or *on a real Deck*. A green build does not prove Deck behaviour.

## Where to help
- Stubs/placeholders — `grep -rniE "stub|not implemented|placeholder" app crates`
- Settings pages: Display, Sound, Keyboard polish; other UI languages (strings come from Steam's own localization)
- Parity with stock Steam UI, controller-layout tools, account/QR flow on a real Deck
- Performance measurements DeckShell vs stock (`tools/perf/`)
- Docs, translations of this README, packaging for other SteamOS versions

## Rules (also for your AI agent — see [AGENTS.md](AGENTS.md))
- **Never switch a real Deck's Gaming Mode session automatically** and never run install/rollback scripts against someone's device without them asking. Use `session/deck-session-restart.sh check` first.
- **No personal data** in commits, tests, fixtures or screenshots: no real Steam IDs, account names, e-mails, IPs/MACs, Wi-Fi names, paired-device names, avatars. Use obviously fake values.
- **No Valve code or client binaries** and no dumps of the Steam client (e.g. `sp.js`, CEF caches) in the repository. Assets are read from the user's own Steam install at build time.
- Be honest in docs and PR text: mark unverified claims as unverified; do not claim performance wins without measurements.
- Keep the stock-UI fallback, the kill switch and `rollback.sh` working.

## License of contributions
By contributing you agree your contribution is licensed under the project license (PolyForm Noncommercial 1.0.0, see [LICENSE](LICENSE)).
