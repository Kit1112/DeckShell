//! Минимальный protobuf (wire format) для сообщений CAuthentication_*: только нужные типы полей.

#[derive(Default, Clone, Debug)]
pub struct Writer(pub Vec<u8>);

fn varint(mut n: u64, out: &mut Vec<u8>) {
    loop {
        let b = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 { out.push(b); return; }
        out.push(b | 0x80);
    }
}

impl Writer {
    pub fn uint(mut self, field: u32, v: u64) -> Self { varint((field as u64) << 3, &mut self.0); varint(v, &mut self.0); self }
    /// int32 (в т.ч. отрицательный: знаковое расширение до 64 бит, как у protobuf).
    pub fn int32(self, field: u32, v: i32) -> Self { self.uint(field, v as i64 as u64) }
    pub fn bytes(mut self, field: u32, v: &[u8]) -> Self {
        varint((field as u64) << 3 | 2, &mut self.0);
        varint(v.len() as u64, &mut self.0);
        self.0.extend_from_slice(v);
        self
    }
    pub fn string(self, field: u32, v: &str) -> Self { self.bytes(field, v.as_bytes()) }
    pub fn message(self, field: u32, m: Writer) -> Self { self.bytes(field, &m.0) }
    pub fn fixed64(mut self, field: u32, v: u64) -> Self { varint((field as u64) << 3 | 1, &mut self.0); self.0.extend_from_slice(&v.to_le_bytes()); self }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value<'a> { Varint(u64), Fixed64(u64), Bytes(&'a [u8]), Fixed32(u32) }

/// Разбор сообщения в список (номер поля, значение). Ошибка формата — `None`.
pub fn parse(mut b: &[u8]) -> Option<Vec<(u32, Value<'_>)>> {
    fn read_varint(b: &mut &[u8]) -> Option<u64> {
        let mut n = 0u64;
        for shift in (0..64).step_by(7) {
            let (&x, rest) = b.split_first()?;
            *b = rest;
            n |= ((x & 0x7f) as u64) << shift;
            if x & 0x80 == 0 { return Some(n); }
        }
        None
    }
    let mut out = Vec::new();
    while !b.is_empty() {
        let key = read_varint(&mut b)?;
        let field = (key >> 3) as u32;
        let v = match key & 7 {
            0 => Value::Varint(read_varint(&mut b)?),
            1 => { let (h, t) = b.split_at_checked(8)?; b = t; Value::Fixed64(u64::from_le_bytes(h.try_into().ok()?)) }
            2 => { let n = read_varint(&mut b)? as usize; let (h, t) = b.split_at_checked(n)?; b = t; Value::Bytes(h) }
            5 => { let (h, t) = b.split_at_checked(4)?; b = t; Value::Fixed32(u32::from_le_bytes(h.try_into().ok()?)) }
            _ => return None,
        };
        out.push((field, v));
    }
    Some(out)
}

pub struct Msg<'a>(pub Vec<(u32, Value<'a>)>);
impl<'a> Msg<'a> {
    pub fn new(b: &'a [u8]) -> Option<Self> { parse(b).map(Msg) }
    fn last(&self, f: u32) -> Option<&Value<'a>> { self.0.iter().rev().find(|(n, _)| *n == f).map(|(_, v)| v) }
    pub fn uint(&self, f: u32) -> Option<u64> { match self.last(f)? { Value::Varint(v) | Value::Fixed64(v) => Some(*v), _ => None } }
    pub fn bytes(&self, f: u32) -> Option<&'a [u8]> { match self.last(f)? { Value::Bytes(v) => Some(v), _ => None } }
    pub fn string(&self, f: u32) -> Option<String> { self.bytes(f).and_then(|b| std::str::from_utf8(b).ok()).map(str::to_owned) }
    pub fn float(&self, f: u32) -> Option<f32> { match self.last(f)? { Value::Fixed32(v) => Some(f32::from_bits(*v)), _ => None } }
    pub fn bool(&self, f: u32) -> bool { self.uint(f).is_some_and(|v| v != 0) }
    /// Повторяющееся вложенное сообщение.
    pub fn all(&self, f: u32) -> Vec<Msg<'a>> {
        self.0.iter().filter(|(n, _)| *n == f).filter_map(|(_, v)| match v { Value::Bytes(b) => Msg::new(b), _ => None }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip_of_all_field_kinds() {
        let w = Writer::default().string(1, "Steam Deck").uint(2, 1).int32(3, -600).message(4, Writer::default().uint(1, 3)).fixed64(5, 0x0102030405060708);
        let m = Msg::new(&w.0).unwrap();
        assert_eq!(m.string(1).unwrap(), "Steam Deck");
        assert_eq!(m.uint(2), Some(1));
        assert_eq!(m.uint(3).map(|v| v as i64 as i32), Some(-600));
        assert_eq!(m.all(4)[0].uint(1), Some(3));
        assert_eq!(m.uint(5), Some(0x0102030405060708));
    }
    #[test]
    fn real_begin_qr_reply_decodes() {
        // Ответ api.steampowered.com (18.11.2026) на BeginAuthSessionViaQR; client_id/request_id одноразовые и давно истекли.
        let body: &[u8] = b"\x08\xc5\xf5\x87\xe8\x90\xc4\xd0\xa3,\x12&https://s.team/q/1/3190591570734873285\x1a\x10\x8c\xe5\xc0;i\x9b\x80\x89\x15\xae`\xc5\xcdL\x16\xde%\x00\x00\xa0@*\x02\x08\x04*\x02\x08\x030\x01";
        let m = Msg::new(body).unwrap();
        assert_eq!(m.string(2).unwrap(), "https://s.team/q/1/3190591570734873285");
        assert_eq!(m.bytes(3).unwrap().len(), 16);
        assert_eq!(m.float(4), Some(5.0));
        assert_eq!(m.all(5).iter().map(|c| c.uint(1).unwrap()).collect::<Vec<_>>(), vec![4, 3]);
        assert!(m.uint(1).unwrap() > 0);
    }
    #[test]
    fn truncated_input_is_rejected() {
        assert!(parse(b"\x12\x05ab").is_none());
        assert!(parse(b"\x08").is_none());
    }
}
