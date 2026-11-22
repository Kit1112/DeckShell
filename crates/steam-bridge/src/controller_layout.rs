//! Read side of a game's controller layout (`CMsgControllerConfiguration`), decoded from the
//! protobuf wire format.
//!
//! Field numbers come from the stock UI's message descriptors (`docs/api/protobufs.json`, the
//! bundle `chunk~2dcc5aaf7.js`), not from a `.proto` file. Only the fields the DeckShell screens
//! need are typed; every other field is skipped. The bytes come from Steam's editing call, which
//! is not invoked by this crate yet, so the tests build payloads with the same field numbers.
//! Nothing here writes to Steam.
use crate::{Error, Result};

/// One decoded wire value. Groups (wire types 3 and 4) are not used by these messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire<'a> {
    Varint(u64),
    Fixed64(u64),
    Len(&'a [u8]),
    Fixed32(u32),
}

/// One field of a message, in wire order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Field<'a> {
    pub number: u32,
    pub wire: Wire<'a>,
}

fn varint(buf: &[u8]) -> Result<(u64, usize)> {
    let mut value = 0u64;
    for (i, byte) in buf.iter().enumerate().take(10) {
        value |= u64::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            return Ok((value, i + 1));
        }
    }
    Err(Error::Protocol("protobuf: truncated or overlong varint".into()))
}

/// Read an enum list accepting either protobuf representation.
pub fn repeated_enums(buf: &[u8], number: u32) -> Result<Vec<i64>> {
    let mut values = Vec::new();
    for field in fields(buf)?.into_iter().filter(|field| field.number == number) {
        match field.wire {
            Wire::Varint(value) => values.push(i64::from(value as i32)),
            Wire::Len(mut packed) => {
                while !packed.is_empty() {
                    let (value, width) = varint(packed)?;
                    values.push(i64::from(value as i32));
                    packed = &packed[width..];
                }
            }
            _ => return Err(Error::Protocol("protobuf: invalid enum list".into())),
        }
    }
    Ok(values)
}

/// Split a message into its fields. A truncated field or an unsupported wire type is an error:
/// a partial layout must not be shown as if it were complete.
pub fn fields(buf: &[u8]) -> Result<Vec<Field<'_>>> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < buf.len() {
        let (tag, n) = varint(&buf[at..])?;
        at += n;
        let number = u32::try_from(tag >> 3).map_err(|_| Error::Protocol("protobuf: field number too large".into()))?;
        if number == 0 {
            return Err(Error::Protocol("protobuf: field number 0".into()));
        }
        let wire = match tag & 7 {
            0 => {
                let (v, n) = varint(&buf[at..])?;
                at += n;
                Wire::Varint(v)
            }
            1 => {
                let b = take(buf, &mut at, 8)?;
                Wire::Fixed64(u64::from_le_bytes(b.try_into().expect("8 bytes")))
            }
            2 => {
                let (len, n) = varint(&buf[at..])?;
                at += n;
                let len = usize::try_from(len).map_err(|_| Error::Protocol("protobuf: length too large".into()))?;
                Wire::Len(take(buf, &mut at, len)?)
            }
            5 => {
                let b = take(buf, &mut at, 4)?;
                Wire::Fixed32(u32::from_le_bytes(b.try_into().expect("4 bytes")))
            }
            w => return Err(Error::Protocol(format!("protobuf: unsupported wire type {w}"))),
        };
        out.push(Field { number, wire });
    }
    Ok(out)
}

fn take<'a>(buf: &'a [u8], at: &mut usize, len: usize) -> Result<&'a [u8]> {
    let end = at.checked_add(len).filter(|&e| e <= buf.len());
    let end = end.ok_or_else(|| Error::Protocol("protobuf: field runs past the end of the message".into()))?;
    let s = &buf[*at..end];
    *at = end;
    Ok(s)
}

fn as_str(f: &Field<'_>) -> Result<String> {
    match f.wire {
        Wire::Len(b) => String::from_utf8(b.to_vec())
            .map_err(|_| Error::Protocol(format!("protobuf: field {} is not UTF-8", f.number))),
        _ => Err(Error::Protocol(format!("protobuf: field {} is not a string", f.number))),
    }
}

fn as_msg<'a>(f: &Field<'a>) -> Result<&'a [u8]> {
    match f.wire {
        Wire::Len(b) => Ok(b),
        _ => Err(Error::Protocol(format!("protobuf: field {} is not a message", f.number))),
    }
}

fn as_enum(f: &Field<'_>) -> Result<i64> {
    match f.wire {
        // int32/enum fields: negative values arrive as 10-byte two's complement varints.
        Wire::Varint(v) => Ok(i64::from(v as i32)),
        _ => Err(Error::Protocol(format!("protobuf: field {} is not a varint", f.number))),
    }
}

/// One action set (`CMsgGameActionSet`): its key, its display name and how many layers it has.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ActionSet {
    pub key: String,
    pub display_name: String,
    pub layer_count: usize,
}

/// One binding of an activator (`CMsgControllerInputBinding`): its type and the label Steam shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Binding {
    pub kind: i64,
    /// `key_binding_data.friendly_name_utf8`; empty when the binding has no key data.
    pub label: String,
    /// `gamepad_button.button` (field 5 → field 1) for a gamepad binding; the stock value of the
    /// output button (A = 3, B = 4, Y = 5, X = 6). `None` for other binding types.
    pub gamepad_value: Option<i64>,
}

/// One activator of an input (`CMsgControllerInputActivator`): press, hold, double press and so on.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Activator {
    pub activation: i64,
    pub bindings: Vec<Binding>,
}

/// One input of a source mode (`CMsgControllerInput`): a button, a stick direction and so on.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Input {
    pub key: i64,
    pub activators: Vec<Activator>,
}

/// One source mode (`CMsgControllerMode`): what a source (stick, trackpad, buttons) does.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SourceMode {
    pub mode: i64,
    pub modeid: u64,
    pub friendly_name: String,
    pub inputs: Vec<Input>,
}

/// The whole layout (`CMsgControllerConfiguration`), with the fields the screens use.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ControllerLayout {
    pub display_name: String,
    pub description: String,
    pub creator: String,
    pub controller_type: i64,
    pub controller_style: i64,
    pub url: String,
    pub error_msg: String,
    pub action_sets: Vec<ActionSet>,
    pub modes: Vec<SourceMode>,
}

/// Decode a `CMsgControllerConfiguration` payload.
pub fn parse_layout(buf: &[u8]) -> Result<ControllerLayout> {
    let mut layout = ControllerLayout::default();
    for f in fields(buf)? {
        match f.number {
            2 => layout.display_name = as_str(&f)?,
            3 => layout.description = as_str(&f)?,
            4 => layout.creator = as_str(&f)?,
            5 => layout.controller_type = as_enum(&f)?,
            7 => layout.controller_style = as_enum(&f)?,
            8 => layout.action_sets.push(parse_action_set(as_msg(&f)?)?),
            9 => layout.modes.push(parse_mode(as_msg(&f)?)?),
            10 => layout.error_msg = as_str(&f)?,
            13 => layout.url = as_str(&f)?,
            _ => {}
        }
    }
    Ok(layout)
}

fn parse_action_set(buf: &[u8]) -> Result<ActionSet> {
    let mut set = ActionSet::default();
    for f in fields(buf)? {
        match f.number {
            1 => set.key = as_str(&f)?,
            2 => set.display_name = as_str(&f)?,
            4 => set.layer_count += 1,
            _ => {}
        }
    }
    Ok(set)
}

fn parse_mode(buf: &[u8]) -> Result<SourceMode> {
    let mut mode = SourceMode::default();
    for f in fields(buf)? {
        match f.number {
            1 => mode.mode = as_enum(&f)?,
            2 => mode.modeid = as_u64(&f)?,
            4 => mode.inputs.push(parse_input(as_msg(&f)?)?),
            6 => mode.friendly_name = as_str(&f)?,
            _ => {}
        }
    }
    Ok(mode)
}

fn parse_input(buf: &[u8]) -> Result<Input> {
    let mut input = Input::default();
    for f in fields(buf)? {
        match f.number {
            1 => input.key = as_enum(&f)?,
            2 => input.activators.push(parse_activator(as_msg(&f)?)?),
            _ => {}
        }
    }
    Ok(input)
}

fn parse_activator(buf: &[u8]) -> Result<Activator> {
    let mut act = Activator::default();
    for f in fields(buf)? {
        match f.number {
            1 => act.activation = as_enum(&f)?,
            2 => act.bindings.push(parse_binding(as_msg(&f)?)?),
            _ => {}
        }
    }
    Ok(act)
}

fn gamepad_button_value(buf: &[u8]) -> Result<i64> {
    let mut value = 0;
    for f in fields(buf)? {
        if f.number == 1 {
            value = as_enum(&f)?;
        }
    }
    Ok(value)
}

fn parse_binding(buf: &[u8]) -> Result<Binding> {
    let mut b = Binding::default();
    for f in fields(buf)? {
        match f.number {
            1 => b.kind = as_enum(&f)?,
            2 => b.label = key_binding_label(as_msg(&f)?)?,
            5 => b.gamepad_value = Some(gamepad_button_value(as_msg(&f)?)?),
            _ => {}
        }
    }
    Ok(b)
}

/// `CMsgControllerInputBinding_KeyBindingData`: field 2 is the friendly name shown on the button.
fn key_binding_label(buf: &[u8]) -> Result<String> {
    let mut label = String::new();
    for f in fields(buf)? {
        if f.number == 2 {
            label = as_str(&f)?;
        }
    }
    Ok(label)
}

fn as_u64(f: &Field<'_>) -> Result<u64> {
    match f.wire {
        Wire::Varint(v) => Ok(v),
        _ => Err(Error::Protocol(format!("protobuf: field {} is not a varint", f.number))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test-only encoder with the same wire rules as the decoder above.
    fn put_varint(out: &mut Vec<u8>, mut v: u64) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return;
            }
            out.push(b | 0x80);
        }
    }
    fn put_int(out: &mut Vec<u8>, field: u32, v: u64) {
        put_varint(out, u64::from(field) << 3);
        put_varint(out, v);
    }
    fn put_bytes(out: &mut Vec<u8>, field: u32, b: &[u8]) {
        put_varint(out, (u64::from(field) << 3) | 2);
        put_varint(out, b.len() as u64);
        out.extend_from_slice(b);
    }
    fn put_str(out: &mut Vec<u8>, field: u32, s: &str) {
        put_bytes(out, field, s.as_bytes());
    }

    /// A small Balatro-like layout: one action set with one layer, one mode with one input
    /// (button A) that has a press activator bound to a labelled key.
    fn fixture() -> Vec<u8> {
        let mut binding = Vec::new();
        put_int(&mut binding, 1, 1);
        let mut kb = Vec::new();
        put_str(&mut kb, 1, "A");
        put_str(&mut kb, 2, "Кнопка A");
        put_bytes(&mut binding, 2, &kb);

        let mut activator = Vec::new();
        put_int(&mut activator, 1, 1);
        put_bytes(&mut activator, 2, &binding);

        let mut input = Vec::new();
        put_int(&mut input, 1, 1);
        put_bytes(&mut input, 2, &activator);

        let mut mode = Vec::new();
        put_int(&mut mode, 1, 2);
        put_int(&mut mode, 2, 7);
        put_bytes(&mut mode, 4, &input);
        put_str(&mut mode, 6, "Лицевые кнопки");

        let mut set = Vec::new();
        put_str(&mut set, 1, "Gameplay");
        put_str(&mut set, 2, "Игра");
        put_bytes(&mut set, 4, &[]);
        put_bytes(&mut set, 4, &[]);

        let mut msg = Vec::new();
        put_str(&mut msg, 2, "Официальная раскладка Balatro - Геймпад");
        put_str(&mut msg, 4, "Official Creator");
        put_int(&mut msg, 5, 4);
        put_int(&mut msg, 7, 2);
        put_str(&mut msg, 13, "workshop/official-balatro");
        put_bytes(&mut msg, 8, &set);
        put_bytes(&mut msg, 9, &mode);
        // A field this decoder does not type must be skipped, not rejected.
        put_int(&mut msg, 30, 99);
        msg
    }

    #[test]
    fn decodes_layout_fields_by_descriptor_numbers() {
        let l = parse_layout(&fixture()).unwrap();
        assert_eq!(l.display_name, "Официальная раскладка Balatro - Геймпад");
        assert_eq!(l.creator, "Official Creator");
        assert_eq!(l.controller_type, 4);
        assert_eq!(l.controller_style, 2);
        assert_eq!(l.url, "workshop/official-balatro");
        assert_eq!(
            l.action_sets,
            vec![ActionSet { key: "Gameplay".into(), display_name: "Игра".into(), layer_count: 2 }]
        );
        assert_eq!(l.modes.len(), 1);
        let m = &l.modes[0];
        assert_eq!((m.mode, m.modeid, m.friendly_name.as_str()), (2, 7, "Лицевые кнопки"));
        assert_eq!(m.inputs[0].key, 1);
        assert_eq!(m.inputs[0].activators[0].activation, 1);
        assert_eq!(
            m.inputs[0].activators[0].bindings,
            vec![Binding { kind: 1, label: "Кнопка A".into(), gamepad_value: None }]
        );
    }

    #[test]
    fn empty_message_is_an_empty_layout() {
        assert_eq!(parse_layout(&[]).unwrap(), ControllerLayout::default());
    }

    #[test]
    fn truncated_field_is_an_error_not_a_partial_layout() {
        // Cut the middle of a length-delimited field: the declared length runs past the end.
        let mut bytes = Vec::new();
        put_str(&mut bytes, 13, "workshop/official-balatro");
        bytes.truncate(bytes.len() - 3);
        assert!(matches!(parse_layout(&bytes), Err(Error::Protocol(_))));
        // A complete message with the same trailing field decodes.
        let mut whole = Vec::new();
        put_str(&mut whole, 13, "workshop/official-balatro");
        assert_eq!(parse_layout(&whole).unwrap().url, "workshop/official-balatro");
    }

    #[test]
    fn unsupported_wire_type_and_field_zero_are_errors() {
        // field 1, wire type 3 (start group): not used by these messages.
        assert!(matches!(fields(&[0x0b]), Err(Error::Protocol(_))));
        // field 0, varint.
        assert!(matches!(fields(&[0x00, 0x01]), Err(Error::Protocol(_))));
    }

    #[test]
    fn string_field_with_wrong_wire_type_is_an_error() {
        let mut msg = Vec::new();
        put_int(&mut msg, 2, 5);
        assert!(matches!(parse_layout(&msg), Err(Error::Protocol(_))));
    }

    #[test]
    fn negative_int32_enum_decodes_as_signed() {
        let mut msg = Vec::new();
        // int32 -1 is sent as a 10-byte varint.
        put_int(&mut msg, 5, u64::MAX);
        assert_eq!(parse_layout(&msg).unwrap().controller_type, -1);
    }

    #[test]
    fn varint_longer_than_ten_bytes_is_an_error() {
        assert!(matches!(varint(&[0x80; 11]), Err(Error::Protocol(_))));
    }
}
