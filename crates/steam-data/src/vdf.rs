//! Text (KeyValues) and binary VDF parsers. Hand written, lenient, read-only.

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Map(Vec<(String, Value)>),
}

impl Value {
    /// Case-insensitive lookup of a direct child (first match).
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Map(m) => m.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v),
            _ => None,
        }
    }
    /// Case-insensitive nested lookup.
    pub fn path(&self, keys: &[&str]) -> Option<&Value> {
        let mut cur = self;
        for k in keys {
            cur = cur.get(k)?;
        }
        Some(cur)
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_map(&self) -> Option<&[(String, Value)]> {
        match self {
            Value::Map(m) => Some(m),
            _ => None,
        }
    }
    pub fn str_at(&self, key: &str) -> Option<&str> {
        self.get(key)?.as_str()
    }
    pub fn u64_at(&self, key: &str) -> Option<u64> {
        self.str_at(key)?.trim().parse().ok()
    }
}

// ---------------------------------------------------------------- text

struct TextParser<'a> {
    s: &'a str,
    b: &'a [u8],
    i: usize,
}

enum Tok {
    Str(String),
    Open,
    Close,
}

impl<'a> TextParser<'a> {
    fn skip_ws(&mut self) {
        loop {
            while self.i < self.b.len() && (self.b[self.i].is_ascii_whitespace() || self.b[self.i] == 0) {
                self.i += 1;
            }
            if self.b[self.i.min(self.b.len())..].starts_with(b"//") {
                while self.i < self.b.len() && self.b[self.i] != b'\n' {
                    self.i += 1;
                }
            } else {
                break;
            }
        }
    }

    fn next(&mut self) -> Result<Option<Tok>, String> {
        self.skip_ws();
        let Some(&c) = self.b.get(self.i) else { return Ok(None) };
        match c {
            b'{' => {
                self.i += 1;
                Ok(Some(Tok::Open))
            }
            b'}' => {
                self.i += 1;
                Ok(Some(Tok::Close))
            }
            b'"' => {
                self.i += 1;
                let start = self.i;
                // fast path: no escapes
                while self.i < self.b.len() && self.b[self.i] != b'"' && self.b[self.i] != b'\\' {
                    self.i += 1;
                }
                if self.i >= self.b.len() {
                    return Err("unterminated string".into());
                }
                if self.b[self.i] == b'"' {
                    let r = self.s[start..self.i].to_owned();
                    self.i += 1;
                    return Ok(Some(Tok::Str(r)));
                }
                let mut out: Vec<u8> = self.b[start..self.i].to_vec();
                while self.i < self.b.len() {
                    let c = self.b[self.i];
                    if c == b'"' {
                        self.i += 1;
                        return Ok(Some(Tok::Str(String::from_utf8_lossy(&out).into_owned())));
                    }
                    if c == b'\\' && self.i + 1 < self.b.len() {
                        let n = self.b[self.i + 1];
                        self.i += 2;
                        match n {
                            b'n' => out.push(b'\n'),
                            b't' => out.push(b'\t'),
                            b'r' => out.push(b'\r'),
                            b'\\' => out.push(b'\\'),
                            b'"' => out.push(b'"'),
                            o => {
                                out.push(b'\\');
                                out.push(o);
                            }
                        }
                    } else {
                        out.push(c);
                        self.i += 1;
                    }
                }
                Err("unterminated string".into())
            }
            _ => {
                let start = self.i;
                while self.i < self.b.len() {
                    let c = self.b[self.i];
                    if c.is_ascii_whitespace() || c == b'"' || c == b'{' || c == b'}' {
                        break;
                    }
                    self.i += 1;
                }
                Ok(Some(Tok::Str(self.s[start..self.i].to_owned())))
            }
        }
    }

    /// `[$X360]` style conditional after a value: skipped.
    fn skip_conditional(&mut self) {
        self.skip_ws();
        if self.b.get(self.i) == Some(&b'[') {
            while self.i < self.b.len() && self.b[self.i] != b']' {
                self.i += 1;
            }
            self.i += 1;
        }
    }

    fn map(&mut self, depth: usize) -> Result<Vec<(String, Value)>, String> {
        if depth > 128 {
            return Err("nesting too deep".into());
        }
        let mut out = Vec::new();
        loop {
            match self.next()? {
                None | Some(Tok::Close) => return Ok(out),
                Some(Tok::Open) => {} // stray brace, ignore
                Some(Tok::Str(k)) => match self.next()? {
                    Some(Tok::Open) => {
                        let m = self.map(depth + 1)?;
                        out.push((k, Value::Map(m)));
                    }
                    Some(Tok::Str(v)) => {
                        self.skip_conditional();
                        out.push((k, Value::Str(v)));
                    }
                    Some(Tok::Close) | None => return Ok(out),
                },
            }
        }
    }
}

/// Parse text KeyValues. Result is a root `Map` holding the top-level pairs.
/// Truncated input (missing closing braces) is tolerated.
pub fn parse_text(s: &str) -> Result<Value, String> {
    let s = s.strip_prefix('\u{feff}').unwrap_or(s);
    let mut p = TextParser { s, b: s.as_bytes(), i: 0 };
    let mut root = Vec::new();
    loop {
        let m = p.map(0)?;
        root.extend(m);
        if p.i >= p.b.len() {
            break;
        }
    }
    Ok(Value::Map(root))
}

// -------------------------------------------------------------- binary

struct BinParser<'a> {
    d: &'a [u8],
    i: usize,
    table: Option<&'a [String]>,
}

impl<'a> BinParser<'a> {
    fn cstr(&mut self) -> Option<String> {
        let rest = self.d.get(self.i..)?;
        let n = rest.iter().position(|&c| c == 0)?;
        let s = String::from_utf8_lossy(&rest[..n]).into_owned();
        self.i += n + 1;
        Some(s)
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let r = self.d.get(self.i..self.i.checked_add(n)?)?;
        self.i += n;
        Some(r)
    }
    fn key(&mut self) -> Option<String> {
        match self.table {
            Some(t) => {
                let b = self.take(4)?;
                let idx = u32::from_le_bytes(b.try_into().ok()?) as usize;
                t.get(idx).cloned()
            }
            None => self.cstr(),
        }
    }
    fn map(&mut self, depth: usize) -> Vec<(String, Value)> {
        let mut out = Vec::new();
        if depth > 64 {
            return out;
        }
        loop {
            let Some(&ty) = self.d.get(self.i) else { return out };
            self.i += 1;
            if ty == 0x08 || ty == 0x0B {
                return out;
            }
            let Some(k) = self.key() else { return out };
            let v = match ty {
                0x00 => Value::Map(self.map(depth + 1)),
                0x01 => match self.cstr() {
                    Some(s) => Value::Str(s),
                    None => return out,
                },
                0x02 | 0x04 | 0x06 => match self.take(4) {
                    Some(b) => Value::Str(u32::from_le_bytes(b.try_into().unwrap()).to_string()),
                    None => return out,
                },
                0x03 => match self.take(4) {
                    Some(b) => Value::Str(f32::from_le_bytes(b.try_into().unwrap()).to_string()),
                    None => return out,
                },
                0x07 => match self.take(8) {
                    Some(b) => Value::Str(u64::from_le_bytes(b.try_into().unwrap()).to_string()),
                    None => return out,
                },
                0x0A => match self.take(8) {
                    Some(b) => Value::Str(i64::from_le_bytes(b.try_into().unwrap()).to_string()),
                    None => return out,
                },
                0x05 => {
                    // wide string, UTF-16LE, 0-terminated
                    let mut units = Vec::new();
                    loop {
                        match self.take(2) {
                            Some(b) => {
                                let u = u16::from_le_bytes([b[0], b[1]]);
                                if u == 0 {
                                    break;
                                }
                                units.push(u);
                            }
                            None => return out,
                        }
                    }
                    Value::Str(String::from_utf16_lossy(&units))
                }
                _ => return out,
            };
            out.push((k, v));
        }
    }
}

/// Parse binary KeyValues (shortcuts.vdf, appinfo payloads). Lenient: a truncated
/// buffer yields whatever was parsed. Integers are rendered as decimal strings
/// (int32 as unsigned). `table` supplies the string table for appinfo v41 keys.
pub fn parse_binary(data: &[u8], table: Option<&[String]>) -> Value {
    let mut p = BinParser { d: data, i: 0, table };
    Value::Map(p.map(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_basic() {
        let v = parse_text(
            "\"AppState\"\n{\n\t\"appid\"\t\"123\"\n\t// comment\n\t\"Name\" \"A \\\"quoted\\\" game\\\\x\"\n\t\"Sub\"\n\t{\n\t\t\"k\" \"v\"\n\t\t\"n\" { bare value }\n\t}\n}\n",
        )
        .unwrap();
        let a = v.get("appstate").unwrap();
        assert_eq!(a.u64_at("APPID"), Some(123));
        assert_eq!(a.str_at("name"), Some("A \"quoted\" game\\x"));
        assert_eq!(a.path(&["sub", "K"]).and_then(|v| v.as_str()), Some("v"));
        assert_eq!(a.path(&["sub", "n", "bare"]).and_then(|v| v.as_str()), Some("value"));
    }

    #[test]
    fn text_truncated_and_conditional() {
        let v = parse_text("\u{feff}\"a\" { \"b\" \"1\" [$WIN32] \"c\" { \"d\" \"2\"").unwrap();
        assert_eq!(v.path(&["a", "b"]).unwrap().as_str(), Some("1"));
        assert_eq!(v.path(&["a", "c", "d"]).unwrap().as_str(), Some("2"));
        assert!(parse_text("\"abc").is_err());
    }

    #[test]
    fn text_utf8() {
        let v = parse_text("\"n\" \"Мега Вадимус\"").unwrap();
        assert_eq!(v.str_at("n"), Some("Мега Вадимус"));
    }

    #[test]
    fn binary_shortcuts() {
        let mut d: Vec<u8> = vec![0];
        d.extend(b"shortcuts\0");
        d.push(0);
        d.extend(b"0\0");
        d.push(2);
        d.extend(b"appid\0");
        d.extend(&0xC000_0001u32.to_le_bytes());
        d.push(1);
        d.extend(b"AppName\0Cool Game\0");
        d.push(2);
        d.extend(b"LastPlayTime\0");
        d.extend(&1700000000u32.to_le_bytes());
        d.extend([8, 8, 8, 8]);
        let v = parse_binary(&d, None);
        let e = v.path(&["shortcuts", "0"]).unwrap();
        assert_eq!(e.u64_at("appid"), Some(0xC000_0001));
        assert_eq!(e.str_at("appname"), Some("Cool Game"));
        assert_eq!(e.u64_at("LastPlayTime"), Some(1700000000));
        // truncated input does not panic
        let _ = parse_binary(&d[..20], None);
    }

    #[test]
    fn binary_string_table() {
        let table = vec!["common".to_string(), "name".to_string()];
        let mut d = vec![0u8];
        d.extend(0u32.to_le_bytes());
        d.push(1);
        d.extend(1u32.to_le_bytes());
        d.extend(b"Hi\0");
        d.extend([8, 8]);
        let v = parse_binary(&d, Some(&table));
        assert_eq!(v.path(&["common", "name"]).unwrap().as_str(), Some("Hi"));
    }
}
