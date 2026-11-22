use i_slint_compiler::{lexer, literals, parser::SyntaxKind};
use proc_macro2::{Delimiter, TokenStream, TokenTree};
use std::collections::BTreeMap;
use std::path::Path;
use unicode_script::{Script, UnicodeScript};

fn contains_cyrillic(text: &str) -> bool {
    text.chars().any(|ch| ch.script() == Script::Cyrillic)
}

fn rust_literals(tokens: TokenStream, lines: &mut Vec<usize>) {
    let mut attribute = false;
    let mut test_item = false;
    for token in tokens {
        match token {
            TokenTree::Punct(punct) if punct.as_char() == '#' => {
                attribute = true;
                continue;
            }
            TokenTree::Punct(punct) if attribute && punct.as_char() == '!' => continue,
            TokenTree::Group(group) if attribute && group.delimiter() == Delimiter::Bracket => {
                let stream = group.stream();
                let mut tokens = stream.clone().into_iter();
                match tokens.next() {
                    Some(TokenTree::Ident(id)) if id == "doc" => (),
                    Some(TokenTree::Ident(id)) if id == "test" => test_item = true,
                    Some(TokenTree::Ident(id)) if id == "cfg" => {
                        test_item |= matches!(tokens.next(), Some(TokenTree::Group(g))
                            if g.delimiter() == Delimiter::Parenthesis && g.stream().to_string() == "test");
                        rust_literals(stream, lines);
                    }
                    _ => rust_literals(stream, lines),
                }
            }
            TokenTree::Group(group) if test_item && group.delimiter() == Delimiter::Brace => {
                test_item = false;
            }
            TokenTree::Punct(punct) if punct.as_char() == ';' => test_item = false,
            _ if test_item => (),
            TokenTree::Group(group) => rust_literals(group.stream(), lines),
            TokenTree::Literal(literal) => {
                let cyrillic =
                    match syn::parse2::<syn::Lit>(TokenTree::Literal(literal.clone()).into()) {
                        Ok(syn::Lit::Str(text)) => contains_cyrillic(&text.value()),
                        Ok(syn::Lit::ByteStr(text)) => {
                            std::str::from_utf8(&text.value()).is_ok_and(contains_cyrillic)
                        }
                        Ok(syn::Lit::CStr(text)) => {
                            text.value().to_str().is_ok_and(contains_cyrillic)
                        }
                        _ => false,
                    };
                if cyrillic {
                    lines.push(literal.span().start().line);
                }
            }
            _ => (),
        }
        attribute = false;
    }
}

fn cyrillic_json(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(text) => contains_cyrillic(text),
        serde_json::Value::Array(items) => items.iter().any(cyrillic_json),
        serde_json::Value::Object(fields) => fields
            .iter()
            .any(|(key, value)| contains_cyrillic(key) || cyrillic_json(value)),
        _ => false,
    }
}

fn script_lines(extension: &str, source: &str) -> Vec<usize> {
    let mut block_comment = false;
    let mut lines = Vec::new();
    for (line, text) in source.lines().enumerate() {
        let mut code = text.trim_start();
        if extension == "sh" && code.starts_with('#') {
            continue;
        }
        if extension != "sh" {
            if code.starts_with("/*") {
                block_comment = true;
            }
            if block_comment {
                let Some((_, rest)) = code.split_once("*/") else {
                    continue;
                };
                code = rest.trim_start();
                block_comment = false;
            }
            if code.starts_with("//") {
                continue;
            }
        }
        if contains_cyrillic(code) {
            lines.push(line + 1);
        }
    }
    lines
}

fn cyrillic_lines(extension: &str, source: &str) -> Vec<usize> {
    match extension {
        "rs" => {
            let mut lines = Vec::new();
            rust_literals(source.parse().unwrap(), &mut lines);
            lines
        }
        "slint" => lexer::lex(source)
            .into_iter()
            .filter(|token| token.kind == SyntaxKind::StringLiteral)
            .filter(|token| {
                let text = token.text.as_str();
                literals::unescape_string(text)
                    .as_deref()
                    .map_or_else(|| contains_cyrillic(text), contains_cyrillic)
            })
            .map(|token| {
                source[..token.offset]
                    .bytes()
                    .filter(|&b| b == b'\n')
                    .count()
                    + 1
            })
            .collect(),
        "json" => {
            let value = serde_json::from_str(source).expect("UI data must be valid JSON");
            let mut lines = script_lines(extension, source);
            if lines.is_empty() && cyrillic_json(&value) {
                lines.push(1);
            }
            lines
        }
        "js" | "sh" => script_lines(extension, source),
        _ => unreachable!(),
    }
}

fn scan(root: &Path, dir: &Path, hits: &mut BTreeMap<String, Vec<usize>>) {
    for entry in dir.read_dir().unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            if !matches!(
                entry.file_name().to_str(),
                Some("tests" | "target" | ".git" | ".dispatcher" | "node_modules")
            ) {
                scan(root, &path, hits);
            }
        } else if kind.is_file() {
            let relative = path.strip_prefix(root).unwrap();
            if relative == Path::new("locales/ru.json") {
                continue;
            }
            let extension = path.extension().and_then(|name| name.to_str());
            let product_data = matches!(
                relative.iter().next().and_then(|part| part.to_str()),
                Some("app" | "crates" | "session" | "locales")
            );
            let extension = match extension {
                Some(ext @ ("rs" | "slint")) => ext,
                Some(ext @ ("js" | "json" | "sh")) if product_data => ext,
                _ => continue,
            };
            if path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| {
                    stem == "tests" || stem.ends_with("_tests")
                })
            {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            let lines = cyrillic_lines(extension, &source);
            if !lines.is_empty() {
                hits.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .to_owned(),
                    lines,
                );
            }
        }
    }
}

#[test]
fn detects_cyrillic_cooked_raw_escaped_and_interpolated_strings() {
    let rust = r##"/// "Комментарий"
        let _ = "Подпись";
        let _ = r#"Много
        строк"#;
        let _ = "\u{0410}";
        let _ = c"Ґ";
        let _ = b"\xd0\x90";
        let _ = 'я';
        let _ = "English";
        let _ = [doc, "Подпись"];
        /* "Тоже комментарий" */"##;
    assert_eq!(cyrillic_lines("rs", rust), [2, 3, 5, 6, 7, 10]);

    let slint = r#"// "Комментарий"
        text: "Счёт \{number} шт.";
        title: "\u{0410}";
        text: "\{number} шт.";
        /* "Тоже комментарий" */"#;
    assert_eq!(cyrillic_lines("slint", slint), [2, 2, 3, 4]);

    let rust = r#"#[cfg(test)] mod unit { fn sample() { let _ = "Пример"; } }
        #[test] fn test() { let _ = "Проверка"; }
        fn runtime() { let _ = "Ошибка"; }"#;
    assert_eq!(cyrillic_lines("rs", rust), [3]);
    assert_eq!(cyrillic_lines("json", r#"{"label":"\u042f"}"#), [1]);
    assert_eq!(
        cyrillic_lines("js", "// Описание\nconst title = 'Заголовок';"),
        [2]
    );
    assert_eq!(cyrillic_lines("sh", "# Описание\nprintf 'Ошибка'"), [2]);
}

#[test]
fn no_cyrillic_literals_outside_russian_catalog_and_tests() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut hits = BTreeMap::new();
    scan(&root, &root, &mut hits);
    let locations: Vec<_> = hits
        .iter()
        .map(|(file, lines)| format!("{file}: {lines:?}"))
        .collect();
    assert!(
        hits.is_empty(),
        "Cyrillic literals:\n{}",
        locations.join("\n")
    );
}
