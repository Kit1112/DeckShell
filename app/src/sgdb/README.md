# sgdb - native SteamGridDB cover changer

Rust/Slint replacement of the Decky plugin `decky-steamgriddb` (nothing of the plugin runs: no Python backend, no JS; its settings file
is only read as a seed for filters / remembered game matches). Entry: game context menu "Сменить картинку..." (before "Свойства...") on the
game page START menu and the Home/Library tile menu.

## Files
- `api.rs` SGDB API v2 over curl (key via stdin config, never argv), plugin-identical query (`getApiParams` port), filters, settings.
- `cache.rs` `~/.cache/deckshell/sgdb/{json,thumbs,full,undo}`; thumbs+full share an LRU byte budget (`DECKSHELL_SGDB_CACHE_MB`, 160).
- `apply.rs` Steam calls exactly as the plugin: `ClearCustomArtworkForApp` -> 500 ms -> `SetCustomArtworkForApp(appid,b64,"png",eAssetType)`,
  shortcut logo setup; Steam-app icon via librarycache. DRYRUN only logs.
- `hero_crop.rs` hero (banner) crop per app: zoom 1..3 and pan -1..1, `~/.config/deckshell/hero_crop.json` (`DECKSHELL_HERO_CROP`
  overrides), one undo level per app, atomic save. Steam has no hero crop call, so nothing is sent to Steam.
- `logo_layout.rs` per-app logo zoom 0.25..2 and pan -1..1, `~/.config/deckshell/logo_layout.json` (`DECKSHELL_LOGO_LAYOUT` overrides), with atomic save and one undo level. Existing Steam logo-position metadata is not imported.
- `mod.rs` state, input (gamepad/touch/keyboard), worker pool, preview, batch. `ui/screens/sgdb.slint` view.
- `ui/widgets/hero_img.slint` `CroppedImage` / `HeroFrame`: the one place the crop transform is drawn (backdrop and editor use it).

## API key
The plugin's built-in key is private (its source says reuse gets banned) and is NOT used. Put your own key (steamgriddb.com/profile/preferences/api)
in `~/.config/deckshell/sgdb.json` `{"api_key":"..."}`, `DECKSHELL_SGDB_KEY`, or type it: START -> "Ключ API". No key: offline cache only.
Test overrides: `DECKSHELL_SGDB_API` (base URL), `DECKSHELL_SGDB_CONFIG`, `DECKSHELL_SGDB_CACHE`.

## Features
Tabs capsule / wide / hero / logo / icon; filters (styles, dimensions, mimes, static/animated, adult/humor/epilepsy/untagged) saved per type;
Steam appid lookup, name search + game picker for non-Steam (saved per app); infinite paging; side-by-side "now | candidate" preview in context
(tile, hero with the real logo position and the saved crop, logo over hero); animated candidates (APNG/WebP always, WebM needs ffmpeg) play at source fps
(>=16 ms/frame), only the focused visible tile, textures pre-converted once, timer stops when offscreen/covered; logo position editor;
hero position editor (below);
undo (one level per app+type, persisted; undo of undo swaps); reset to default; invisible image; batch "apply to all missing" (B cancels);
offline cache; after apply steam-bridge re-resolves art and every screen reloads at once.
Pad: D-pad move, A apply, X filters, Y name search, START actions, LB/RB tabs, B back. Touch: tap selects, second tap applies.

## Hero position (banner crop)
On the Hero tab, START -> "Положение баннера…" opens the editor. Its frame has the same aspect as the game page banner
(`Tokens.game-hero-h` over the window width) and the real logo on top, drawn by the same `CroppedImage` as the backdrop.
Layout (1280x800): frame on the left, readout (zoom, pan X/Y in %) and the buttons on the right, between the top bar and the footer.
Pad: the D-pad moves the focus over the on-screen buttons (rows: ← ↑ ↓ →; − +; Сброс Отмена Сохранить; left/right stay in a row,
up/down keep the column). A presses the focused button: arrows pan the picture by 0.1 (the picture follows the arrow), − / + zoom,
Сброс resets the edit, Отмена leaves, Сохранить stores. The left stick pans directly (0.03 per repeat, focus stays). LB/RB zoom,
X resets, B cancels. Touch: tapping a button presses it. The stick is told apart from the D-pad by `input::analog_dir()`.
Stored per app (appid, non-Steam too) in `hero_crop.rs`, not in Steam. Applied to the game page and Home backdrop at draw time
(`app.rs` `update_bg` sets `BackdropState.bg-a/b-crop`): no image reload, so no flicker. START on the Hero tab also offers
"Отменить положение баннера" (one level, swaps like the picture undo) and "Сбросить положение баннера" (itself undoable).
Not covered: the wide card fallback (`app.rs` `card`, Hero used when Steam has no header image) stays uncropped because its
aspect differs.
Geometry: the cover fit overflows the frame in one axis, so the pan slides the image at zoom 1 as well. `placement` and `source_rect`
in `hero_crop.rs` define it (tests pin both); `CroppedImage` places the image with the same formula. The fixture
`hero_editor_pan_and_focus_redraw` (fixture_gallery) checks that pan, zoom and focus redraw the editor.

## Logo position
On the Logo or Hero tab, START → "Положение логотипа…" uses the same controls as the banner editor. The image stays proportional and within the game-page logo frame. The saved layout is local to DeckShell, including for non-Steam games; reset and undo are per game. The preview and game page share `PositionedLogo`. Existing Steam logo-position metadata is not applied or changed by the editor.

Stock position and readout (task 361). A game without a saved layout draws its logo at `zoom 1, pan_x -1, pan_y 0.7`
(`DEFAULT_PAN_Y`): the place the task-359 editor showed as "+70%". Reset (X / Сброс / "Сбросить положение логотипа") gives the same.
The editor's Y readout is on the stock scale: `Y% = pan_y / 0.7 * 100`, so stock reads **+100%**, the centre 0%, the frame
bottom +143%, the top −143%. One button press moves Y by 10 on the readout (0.07 stored), the stick by 3. X is unchanged (stored pan × 100).
Chosen reading (the most conservative one): only the default and the readout changed. Stored values keep their meaning
(fraction of the frame travel), `logo_layout.json` stays version 1 and is neither migrated nor rewritten on load, so every
saved logo stays on the same pixels and an older build reads the file as before. Consequences:
- positions below stock stay reachable and read above +100 (a logo saved at old +80 now reads +114, old +100 reads +143);
- a layout that was explicitly saved at the old default (bottom) stays at the bottom, it is not treated as "no setting";
- undo of a layout first saved by the 359 build returns to the old bottom default stored in `prev`, not to the new stock.
Tests: `stock_position_is_old_plus_70_and_reads_plus_100`, `saved_layouts_from_the_old_build_keep_their_position` (`logo_layout.rs`).

## Limits
Icons of non-Steam shortcuts need a shortcuts.vdf edit (not done). Local-file picker not implemented. Icon "reset" unsupported.
Tested against a local mock API (no real key available): live SteamGridDB responses unverified.

## Animations
Every visible animated tile plays (not only the focused one). A tile's SteamGridDB `.webm` thumb is turned by ONE worker job into an on-disk clip
(`media::open_clip`, ffmpeg at the SOURCE timing: pts from `showinfo`, no fps filter, lz4 per frame, `~/.cache/deckshell/clips`, pruned to 512 MB)
and its first frame is the still. Playback keeps only the current frame: one shared timer, absolute per-tile schedule, frame = pread + lz4
straight into a Slint pixel buffer, only for tiles inside the viewport; the timer stops when nothing animates / a panel covers the grid.
"Animated only" (static box off) sends neither `dimensions` nor `mimes` (an APNG is `image/png`, odd sizes are still animations) and splits
capsule / wide client side by aspect (`AType::fits_tab`).
`DECKSHELL_SGDB_AUTOOPEN=<appid>[,<tab>[,animonly]]` opens the picker by itself (live checks).

## Leftovers / unmeasured
- Hero crop on a real Deck NOT yet verified in Gaming Mode after the task 302 changes (owner test pending). Tests cover the
  geometry, the focus moves and the store; the editor layout was checked with the software renderer only.
- CPU/RSS of the final shared-clock build NOT measured. Previous build (per-tile timers, 8 tiles, 30 fps) used ~50% of one core, RSS ~205 MB: redraws were
  per tile, hence the shared clock (`media::Clip::at`); expected ~10-15%. Check with DECKSHELL_ARTLOG ([sgdb-tick] lines).
- app.rs still plays home/library covers from RAM frames (ANIM_BYTES 32 MB): at source fps only ~1-2 animations fit. Needs Clip-based playback
  (media::Clip + frame_into) there. loader.rs ANIM_STEP_MS is now 16 (app.rs idle pulse `kb_tick % 5` becomes ~12 Hz instead of ~9.5 Hz).
- Animated hero/wide/logo tiles in the picker use the same clip path; icons have no animated assets on SGDB.

Task360 used an upward `pan_y=-0.7` default. Task361 supersedes that interpretation with the former +70 position (`pan_y=0.7`), as described under "Logo position" above. Saved layouts retain their stored values.
