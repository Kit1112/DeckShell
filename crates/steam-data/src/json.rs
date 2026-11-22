//! Minimal JSON parser (enough for cloud-storage collections).

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        if let Json::Str(s) = self { Some(s) } else { None }
    }
    pub fn as_arr(&self) -> Option<&[Json]> {
        if let Json::Arr(a) = self { Some(a) } else { None }
    }
    pub fn as_f64(&self) -> Option<f64> {
        if let Json::Num(n) = self { Some(*n) } else { None }
    }
}

struct P<'a> {
    b: &'a [u8],
    s: &'a str,
    i: usize,
}

impl<'a> P<'a> {
    fn ws(&mut self) {
        while self.i < self.b.len() && self.b[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn value(&mut self, depth: usize) -> Option<Json> {
        if depth > 64 {
            return None;
        }
        self.ws();
        match *self.b.get(self.i)? {
            b'{' => {
                self.i += 1;
                let mut m = Vec::new();
                loop {
                    self.ws();
                    match *self.b.get(self.i)? {
                        b'}' => {
                            self.i += 1;
                            return Some(Json::Obj(m));
                        }
                        b',' => self.i += 1,
                        _ => {
                            let k = self.string()?;
                            self.ws();
                            if *self.b.get(self.i)? != b':' {
                                return None;
                            }
                            self.i += 1;
                            m.push((k, self.value(depth + 1)?));
                        }
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut a = Vec::new();
                loop {
                    self.ws();
                    match *self.b.get(self.i)? {
                        b']' => {
                            self.i += 1;
                            return Some(Json::Arr(a));
                        }
                        b',' => self.i += 1,
                        _ => a.push(self.value(depth + 1)?),
                    }
                }
            }
            b'"' => self.string().map(Json::Str),
            b't' if self.b[self.i..].starts_with(b"true") => {
                self.i += 4;
                Some(Json::Bool(true))
            }
            b'f' if self.b[self.i..].starts_with(b"false") => {
                self.i += 5;
                Some(Json::Bool(false))
            }
            b'n' if self.b[self.i..].starts_with(b"null") => {
                self.i += 4;
                Some(Json::Null)
            }
            _ => {
                let st = self.i;
                while self.i < self.b.len() && matches!(self.b[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
                    self.i += 1;
                }
                self.s[st..self.i].parse().ok().map(Json::Num)
            }
        }
    }
    fn hex4(&mut self) -> Option<u32> {
        let h = self.s.get(self.i..self.i + 4)?;
        self.i += 4;
        u32::from_str_radix(h, 16).ok()
    }
    fn string(&mut self) -> Option<String> {
        if *self.b.get(self.i)? != b'"' {
            return None;
        }
        self.i += 1;
        let mut out = String::new();
        loop {
            let st = self.i;
            while self.i < self.b.len() && self.b[self.i] != b'"' && self.b[self.i] != b'\\' {
                self.i += 1;
            }
            out.push_str(&self.s[st..self.i]);
            match *self.b.get(self.i)? {
                b'"' => {
                    self.i += 1;
                    return Some(out);
                }
                _ => {
                    let n = *self.b.get(self.i + 1)?;
                    self.i += 2;
                    match n {
                        b'n' => out.push('\n'),
                        b't' => out.push('\t'),
                        b'r' => out.push('\r'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'u' => {
                            let mut c = self.hex4()?;
                            if (0xD800..0xDC00).contains(&c) && self.b[self.i..].starts_with(b"\\u") {
                                self.i += 2;
                                let lo = self.hex4()?;
                                c = 0x10000 + ((c - 0xD800) << 10) + (lo.wrapping_sub(0xDC00) & 0x3FF);
                            }
                            out.push(char::from_u32(c).unwrap_or('\u{fffd}'));
                        }
                        o => out.push(o as char),
                    }
                }
            }
        }
    }
}

pub fn parse(s: &str) -> Option<Json> {
    let mut p = P { b: s.as_bytes(), s, i: 0 };
    p.value(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn basic() {
        let j = parse(r#"[["a",{"v":"{\"name\":\"\\u0414x\",\"added\":[1,3009679566]}","n":null,"b":true}]]"#).unwrap();
        let inner = j.as_arr().unwrap()[0].as_arr().unwrap()[1].get("v").unwrap().as_str().unwrap();
        let k = parse(inner).unwrap();
        assert_eq!(k.get("name").unwrap().as_str(), Some("Дx"));
        assert_eq!(k.get("added").unwrap().as_arr().unwrap()[1].as_f64(), Some(3009679566.0));
    }
}
