//! Embedded DeckShell strings and runtime locale selection.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::OnceLock;

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Message {
    Text(String),
    Plural(BTreeMap<Category, String>),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "lowercase")]
enum Category {
    One,
    Few,
    Many,
    Other,
}

fn english_plural(count: i64) -> Category {
    if count.unsigned_abs() == 1 {
        Category::One
    } else {
        Category::Other
    }
}

fn russian_plural(count: i64) -> Category {
    let n = count.unsigned_abs();
    match (n % 10, n % 100) {
        (_, 11..=14) => Category::Many,
        (1, _) => Category::One,
        (2..=4, _) => Category::Few,
        _ => Category::Many,
    }
}

pub struct Locale {
    code: &'static str,
    steam: &'static str,
    source: &'static str,
    plural: fn(i64) -> Category,
    #[cfg(test)]
    forms: &'static [Category],
    catalog: OnceLock<BTreeMap<String, Message>>,
}

static LOCALES: [Locale; 2] = [
    Locale {
        code: "en",
        steam: "english",
        source: include_str!("../../../locales/en.json"),
        plural: english_plural,
        #[cfg(test)]
        forms: &[Category::One, Category::Other],
        catalog: OnceLock::new(),
    },
    Locale {
        code: "ru",
        steam: "russian",
        source: include_str!("../../../locales/ru.json"),
        plural: russian_plural,
        #[cfg(test)]
        forms: &[
            Category::One,
            Category::Few,
            Category::Many,
            Category::Other,
        ],
        catalog: OnceLock::new(),
    },
];

fn english() -> &'static Locale {
    &LOCALES[0]
}

/// Resolve a registered locale by its short code or Steam bundle name.
pub fn by_code(value: &str) -> Option<&'static Locale> {
    let value = value.trim();
    LOCALES.iter().find(|locale| {
        locale.code.eq_ignore_ascii_case(value) || locale.steam.eq_ignore_ascii_case(value)
    })
}

fn select(
    env: Option<&str>,
    config: impl FnOnce() -> Option<String>,
    steam: impl FnOnce() -> Option<String>,
) -> &'static Locale {
    env.and_then(by_code)
        .or_else(|| config().as_deref().and_then(by_code))
        .or_else(|| {
            let name = steam()?;
            LOCALES
                .iter()
                .find(|locale| locale.steam.eq_ignore_ascii_case(&name))
        })
        .unwrap_or_else(english)
}

/// Resolved once per process. Restart DeckShell after changing the language.
pub fn current() -> &'static Locale {
    static LOCALE: OnceLock<&Locale> = OnceLock::new();
    LOCALE.get_or_init(|| {
        select(
            std::env::var("DECKSHELL_LANG").ok().as_deref(),
            || std::fs::read_to_string(super::home().join(".config/deckshell/lang")).ok(),
            || {
                super::registry_language(&[
                    super::home().join(".steam/registry.vdf"),
                    super::steam_root().join("registry.vdf"),
                ])
            },
        )
    })
}

/// A plain string with no arguments.
pub fn tr(key: &str) -> String {
    current().tr(key)
}

/// Borrow a plain catalog string for APIs that take a string slice.
pub fn tr_ref(key: &str) -> &str {
    current().template_or_key(key, None)
}

/// Resolve a catalog reference emitted by the UI capture generators.
pub fn captured_text(text: &str) -> &str {
    match text.strip_prefix("i18n:") {
        Some(key) => tr_ref(key),
        None => text,
    }
}

/// Replace named placeholders with the supplied values.
pub fn tr_args(key: &str, args: &[(&str, &str)]) -> String {
    current().tr_args(key, args)
}

/// Select the plural form and supply the reserved `{count}` argument.
pub fn tr_plural(key: &str, count: i64, args: &[(&str, &str)]) -> String {
    current().tr_plural(key, count, args)
}

impl Locale {
    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn steam_name(&self) -> &'static str {
        self.steam
    }

    fn catalog(&self) -> &BTreeMap<String, Message> {
        self.catalog.get_or_init(|| {
            serde_json::from_str(self.source)
                .unwrap_or_else(|error| panic!("invalid embedded {} locale: {error}", self.code))
        })
    }

    fn template(&self, key: &str, count: Option<i64>) -> Option<&str> {
        match self.catalog().get(key)? {
            Message::Text(text) => Some(text),
            Message::Plural(forms) => {
                let category = count.map(self.plural).unwrap_or(Category::Other);
                forms
                    .get(&category)
                    .or_else(|| forms.get(&Category::Other))
                    .map(String::as_str)
            }
        }
    }

    fn template_or_key<'a>(&'a self, key: &'a str, count: Option<i64>) -> &'a str {
        let Some(template) = self
            .template(key, count)
            .or_else(|| english().template(key, count))
        else {
            #[cfg(debug_assertions)]
            eprintln!(
                "[deckshell] DEBUG i18n: missing key {key:?} ({})",
                self.code
            );
            return key;
        };
        template
    }

    fn format(&self, key: &str, args: &[(&str, &str)], count: Option<i64>) -> String {
        let template = self.template_or_key(key, count);
        let count = count.map(|number| number.to_string());
        render(template, |name| {
            if name == "count" && count.is_some() {
                return count.as_deref();
            }
            args.iter()
                .find_map(|(arg, value)| (*arg == name).then_some(*value))
        })
    }

    pub fn tr(&self, key: &str) -> String {
        self.tr_args(key, &[])
    }

    pub fn tr_args(&self, key: &str, args: &[(&str, &str)]) -> String {
        self.format(key, args, None)
    }

    pub fn tr_plural(&self, key: &str, count: i64, args: &[(&str, &str)]) -> String {
        self.format(key, args, Some(count))
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn render<'a>(mut template: &str, value: impl Fn(&str) -> Option<&'a str>) -> String {
    let mut output = String::with_capacity(template.len());
    while let Some(open) = template.find('{') {
        output.push_str(&template[..open]);
        let tail = &template[open + 1..];
        let Some((name, rest)) = tail.split_once('}').filter(|(name, _)| valid_name(name)) else {
            output.push('{');
            template = tail;
            continue;
        };
        match value(name) {
            Some(value) => output.push_str(value),
            None => output.push_str(&template[open..template.len() - rest.len()]),
        }
        template = rest;
    }
    output.push_str(template);
    output
}

#[cfg(test)]
#[path = "../tests/i18n_unit/mod.rs"]
mod tests;
