7
L10-31 Reference and verification
L32-37 What this is (zero context)
L38-80 Rollback steps
L81-89 Verify rollback succeeded
L90-106 Where everything lives
L107-119 Remove ALL of it
L120-124 If Steam itself looks broken (rare)

## Reference and verification

Historical reference; details not checked against current code are unverified. Current [rules](../rules.md) take precedence.

# DeckShell: ROLLBACK to stock SteamOS gaming mode

**ONE-LINER (stable copy, independent of the source tree; idempotent, also removes the Stage-1 shortcut):**
```
bash ~/.local/share/deckshell/session/rollback.sh          # user-level + marker;  `sudo bash ...` additionally removes the /etc hook
```
Other ways (run on the Steam Deck host, user `deck`; pick the variant that was installed, or run both, they are idempotent):

```
# user-level install (no root):
bash ~/.local/share/deckshell/session/uninstall.sh --purge
# system-level install (root):
sudo bash ~/.local/share/deckshell/session/uninstall-root.sh --purge
```

Fastest panic button (no scripts, one command, takes effect at next gaming-mode start):
`touch ~/.config/deckshell/disable`   (system-wide variant: `sudo touch /etc/deckshell/disable`)

## What this is (zero context)
DeckShell is a custom Steam UI. When "installed" it replaces ONLY the command that the systemd *user* unit
`steam-launcher.service` runs in gaming mode, via a drop-in file. gamescope, SDDM, Steam itself, SteamOS (A/B image)
are never modified. Removing the drop-in file (+ `systemctl --user daemon-reload`) restores stock behaviour completely.
If anything looks wrong, the session script itself falls back to stock; this document is for when it does not.

## Rollback steps
### A. From desktop mode (works any time)
```
rm -f ~/.config/systemd/user/steam-launcher.service.d/deckshell.conf        # user-level hook
sudo rm -f /etc/systemd/user/steam-launcher.service.d/deckshell.conf        # system-level hook (if present)
systemctl --user daemon-reload
touch ~/.config/deckshell/disable
```
Then return to gaming mode (desktop icon "Return to Gaming Mode") and verify (section Verify).

### B. From SSH (sshd is DISABLED by default on this Deck: enable it BEFORE testing: `sudo systemctl enable --now sshd`, needs the deck password; set one with `passwd` if none)
```
ssh deck@<deck-ip>
touch ~/.config/deckshell/disable
rm -f ~/.config/systemd/user/steam-launcher.service.d/deckshell.conf
sudo rm -f /etc/systemd/user/steam-launcher.service.d/deckshell.conf
export XDG_RUNTIME_DIR=/run/user/$(id -u)
systemctl --user daemon-reload
systemctl --user restart steam-launcher.service     # live session returns to the stock Steam UI (about 20 s)
```
If the screen stays black: `steamosctl switch-to-desktop-mode plasma` or `steamos-session-select plasma`
then `sudo systemctl restart sddm` (last resort; kills the session).

### C. From a TTY (needs a keyboard: USB/Bluetooth; the Deck on-screen keyboard cannot send Ctrl+Alt)
1. Press Ctrl+Alt+F3 (F2 also usable). Login: `deck` + password.
2. Run exactly:
```
touch ~/.config/deckshell/disable
rm -f ~/.config/systemd/user/steam-launcher.service.d/deckshell.conf
sudo rm -f /etc/systemd/user/steam-launcher.service.d/deckshell.conf
XDG_RUNTIME_DIR=/run/user/$(id -u) systemctl --user daemon-reload
steamos-session-select plasma        # next boot/login = desktop mode (safe fallback)
sudo systemctl restart sddm          # restart the display manager: session restarts, stock UI (or desktop)
```
3. Ctrl+Alt+F1 returns to the graphical session if it is running (gaming mode is on tty1 or tty2 depending on boot).

### D. Without any keyboard
1. Wait about 15 s: the watchdog starts stock Steam by itself if DeckShell is not on screen within 10 s (and writes `~/.config/deckshell/disable`).
2. Reboot (hold power), then hold **VOLUME-DOWN (or B)** for 2-3 s right after the SteamOS logo: stock Steam UI for that boot only
   (raw evdev, works in lizard mode; log line `[held-button] TRIGGERED`).
3. Hold "..." while powering on, pick the previous SteamOS image (partition A). The binary is built for glibc 2.34 and runs there too.
4. From stock Steam run the library shortcut "Возврат в рабочий стол" (appid 3046902940), then rollback.sh.

## Verify rollback succeeded
```
ls ~/.config/systemd/user/steam-launcher.service.d/ /etc/systemd/user/steam-launcher.service.d/ 2>&1   # no deckshell.conf
systemctl --user cat steam-launcher.service | grep -E '^(ExecStart|Exec.*=)'                          # ExecStart=/usr/lib/steamos/steam-launcher
pgrep -af 'deckshell|steam-launcher$|steam -'                                                          # steam ... -gamepadui, no deckshell
tail -n 20 ~/.local/state/deckshell/session.log                                                        # last line "STOCK: ..." means fallback worked
```
Expected after restart: stock Steam Big Picture with the usual STEAM/... buttons, `ps` shows `... -gamepadui`.

## Where everything lives
| Path | Owner | What | Created by |
|---|---|---|---|
| ~/.local/share/deckshell/bin/{deckshell,deckshell-run} | deck | binary + launcher | tools/gamescope/install-package.sh |
| ~/.local/share/deckshell/lib/ | deck | optional bundled libs (empty) | same |
| ~/.local/share/deckshell/session/ | deck | session script, gsctl.sh, held-button.py | session/install.sh |
| ~/.config/systemd/user/steam-launcher.service.d/deckshell.conf | deck | THE HOOK (user-level) | session/install.sh |
| ~/.config/deckshell/{mode,disable,disable-once} | deck | mode (stock/hybrid) + kill switches | install.sh / user |
| ~/.local/state/deckshell/{session.log,steam.log} | deck | logs | session script |
| ~/.local/share/applications/deckshell.desktop | deck | desktop entry (Non-Steam game source) | install-package.sh |
| ~/.cache/deckshell/ | deck | build target + art caches | cargo / app |
| /etc/systemd/user/steam-launcher.service.d/deckshell.conf | root | THE HOOK (system-level) | session/install-root.sh |
| /etc/deckshell/{mode,disable} | root | system mode + kill switch | install-root.sh |
| /var/lib/deckshell/{bin,lib,session,backup,install.manifest} | root | system copy, backups of overwritten files | install-root.sh |
| /var/log/deckshell-install.log | root | log of every root change | install-root.sh / uninstall-root.sh |
| Non-Steam shortcut "DeckShell" in Steam library | deck | step-1 test only; remove in Steam: right-click > Manage > Remove non-Steam game | user |

## Remove ALL of it
```
bash ~/.local/share/deckshell/session/uninstall.sh --purge
sudo bash ~/.local/share/deckshell/session/uninstall-root.sh --purge
rm -rf ~/.cache/deckshell
# remove the Non-Steam shortcut from the Steam library UI (do not edit shortcuts.vdf while Steam runs)
```
`--purge` deletes ~/.config/deckshell, ~/.local/share/deckshell, ~/.local/state/deckshell, the .desktop file
(user) and /etc/deckshell + /var/lib/deckshell (root). The root uninstaller restores any file it had overwritten
byte-identically from /var/lib/deckshell/backup (it only ever creates new files today; the log at
/var/log/deckshell-install.log says what was done). No file under /usr and no SteamOS image is ever modified;
a SteamOS update cannot break the rollback.

## If Steam itself looks broken (rare)
The OS short-session tracker (`/usr/lib/steamos/steam-short-session-tracker`) re-bootstraps Steam after 3 Steam runs
shorter than 60 s: ~/.steam is moved to ~/dot-steam.bak.<timestamp> (games are NOT deleted, they are under
~/.local/share/Steam/steamapps). The DeckShell hook disables that tracker while installed, so this should not
trigger; if it did, restore with `mv ~/dot-steam.bak.<ts>/* ~/.steam/` after closing Steam.
