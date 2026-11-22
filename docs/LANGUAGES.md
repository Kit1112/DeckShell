# DeckShell languages

DeckShell automatically selects its language from Steam at runtime. Steam's `russian` selects Russian (`ru`); `english`, an unsupported Steam language, a blank language, or a missing `registry.vdf` selects English (`en`). DeckShell checks `~/.steam/registry.vdf` first, then `registry.vdf` under the Steam installation. A blank value in the first file lets the second file supply the language.

To override Steam, set `DECKSHELL_LANG` or put the locale on one line in `~/.config/deckshell/lang`. The environment wins over the file. Both accept a registered locale code (`ru`, `en`) or its Steam name (`russian`, `english`). Spaces, capitalization, and the final newline are ignored. Empty or unrecognized overrides let selection continue to the next source. Restart DeckShell after changing the language. The installer and release tools require no language setting.

Settings → Power is the reference example. English and Russian are complete for the UI; a few rarely seen messages may still be untranslated — please report them.

## Translating UI text

- Use `steam_assets::i18n::tr("feature.message")` for DeckShell text. The catalogs are embedded with `include_str!`. Lookup tries the selected catalog, then English, then returns the key and emits a DEBUG message in debug builds. Use `tr_ref(key)` when an existing view or footer API takes a borrowed string and the message has no placeholders.
- Prefer a verified Steam token when available. In the app, use `crate::assets::t_or("#SteamToken", &i18n::tr("feature.message"))` so a missing token has a translated fallback. Steam token lookup uses the effective locale's bundle, with an English bundle fallback. The Power page keeps its previously pinned Russian labels and durations so that Russian copy and wrapping stay stable across Steam client versions. Its rail label continues to prefer the Steam token in both languages.
- Pass named placeholders with `tr_args("feature.message", &[("name", display_name)])`. Put `{name}` in each translation and move it as the grammar requires. Values are inserted verbatim in a single pass. For plurals, use a JSON object such as `{"one":"{count} item","other":"{count} items"}` and call `tr_plural("feature.items", count, &[])`. The formatter supplies `{count}`; use the argument slice for other placeholders. Russian requires `one`, `few`, `many`, and `other`. English requires `one` and `other`. Each plural form keeps the same placeholders as the English entry.
- Translate in Rust and pass the resulting `SharedString` to Slint. For repeated or dynamic text, populate the row model before assigning it to the view. For a fixed label, declare an `in property <string> heading;`, bind `text: root.heading;`, and call `set_heading(i18n::tr("feature.heading").into())` from Rust while building the view. Power passes translated labels and descriptions through `SettingsState.content` into `app/ui/screens/recording_settings.slint`. Internal IDs, Steam setting keys, and bridge payloads retain their original values.
- The workspace guard counts Cyrillic string literals by file across the repository, skipping locale and test directories and dedicated test files. When migrating a file, lower its count in `locales/cyrillic-allowlist.json` or delete the entry at zero. The test reports counts and line numbers on increases. Comments and Cyrillic character literals are outside the guard's scope.

## Adding a language

1. Copy `locales/en.json` to `locales/<code>.json` and translate every value. English defines the complete key set and placeholder names. Preserve whitespace where it affects rendering. Match the message type (text or plural), and use the new language's plural categories with an `other` form.
2. In `crates/steam-assets/src/i18n.rs`, add the locale to `LOCALES` and update the array length. Set the short code, the Steam bundle name (the suffix in `steamui_<name>-json.js`), the `include_str!` path, and the integer plural rule and its expected categories. Reuse an existing plural rule if it matches. Add resolver and plural-rule cases to the unit tests.
3. Run the catalog completeness test, then the workspace suite on the PC.

   ```sh
   DECKSHELL_DRYRUN=1 cargo test -p steam-assets catalogs_match_english
   DECKSHELL_DRYRUN=1 cargo test --workspace
   ```

4. Launch DeckShell on the PC with the override and open Settings → Power. Check the rail label, dropdown values, descriptions, wrapping, and any messages you can exercise in dry-run. Repeat with `DECKSHELL_LANG=ru` to check the existing Russian copy.

   ```sh
   DECKSHELL_LANG=en DECKSHELL_DRYRUN=1 cargo run -p deckshell
   ```

   Replace `en` with the new locale code to preview that language.

## Copyable prompt for a coding agent

> Add **LANGUAGE** to DeckShell with locale code **CODE** and Steam bundle name **STEAM_NAME**. Read the repository rules and `docs/LANGUAGES.md`. Copy `locales/en.json` to `locales/CODE.json`, translate every value, and preserve each key and named placeholder. Register the catalog and Steam mapping in `crates/steam-assets/src/i18n.rs`. Implement or reuse the language's integer plural rule and add resolver and plural coverage. Prefer existing Steam tokens for stock labels and preserve the current Russian UI text and behavior. Run the completeness test and `cargo test --workspace` with `DECKSHELL_DRYRUN=1` on the PC. Use the Dispatcher wrapper if applicable. Verify Power with `DECKSHELL_LANG=CODE` and report what you verified. Use only synthetic test data. Leave the Deck and its session alone, and keep screenshots local.

## Кратко по-русски

DeckShell автоматически выбирает язык Steam. `russian` даёт русский, а неподдерживаемый язык, пустое значение или отсутствие `registry.vdf` дают английский. Переопределение через `DECKSHELL_LANG` имеет приоритет над `~/.config/deckshell/lang`. После смены языка перезапустите DeckShell.

Чтобы добавить язык, скопируйте `locales/en.json`, переведите значения, сохраните ключи и плейсхолдеры, зарегистрируйте код и имя языка Steam и прогоните тесты с `DECKSHELL_DRYRUN=1`. Пример переведённого экрана — «Настройки → Питание».
