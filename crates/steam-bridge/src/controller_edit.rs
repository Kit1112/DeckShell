//! Editor of a game's controller layout (task 210): the protobuf writer and the editing calls.
//!
//! The stock editor sends each change as a base64 string of a serialized protobuf message and
//! gets back the whole edited layout (base64 `CMsgControllerConfiguration`), which it reads with
//! the same reader as [`crate::controller_layout::parse_layout`]. Stock code (`sp.js` and the
//! chunk `chunk~2dcc5aaf7.js`, `SetEditingConfigurationValue`):
//! - `SteamClient.Input.StartEditingControllerConfigurationForAppIDAndControllerIndex(app, idx)`
//!   loads the layout for editing; the reply is base64 of `CMsgControllerConfiguration`.
//! - `SteamClient.Input.SetEditingControllerConfigurationInputBinding(idx, b64)` and the other
//!   `SetEditingControllerConfiguration*` calls take base64 of the message named in the call.
//! - `SteamClient.Input.SaveEditingControllerConfiguration(idx, selectionType, bShared)` saves.
//!
//! This module holds the pieces that do not touch Steam: base64, the protobuf writer and the
//! message builders. The live calls are in [`Bridge`] methods at the end of the file.
use crate::controller_layout::{parse_layout, ControllerLayout};
use crate::{Bridge, Error, Result};

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding, as the stock editor uses (`getResultBase64String`).
pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Decode standard padded base64. Anything else is an error: a partial payload must not be used.
pub fn base64_decode(s: &str) -> Result<Vec<u8>> {
    let bytes = s.as_bytes();
    if bytes.len() % 4 != 0 {
        return Err(Error::Protocol("base64: length is not a multiple of 4".into()));
    }
    let val = |c: u8| -> Result<u32> {
        B64.iter()
            .position(|&x| x == c)
            .map(|p| p as u32)
            .ok_or_else(|| Error::Protocol("base64: invalid character".into()))
    };
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let groups = bytes.len() / 4;
    for (gi, group) in bytes.chunks(4).enumerate() {
        let last = gi + 1 == groups;
        let pad = group.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return Err(Error::Protocol("base64: misplaced padding".into()));
        }
        let mut n = 0u32;
        for (i, &c) in group.iter().enumerate() {
            let v = if i >= 4 - pad { 0 } else { val(c)? };
            n = (n << 6) | v;
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Ok(out)
}

/// Minimal protobuf writer for the wire types the editor messages use: varint, length-delimited.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    fn varint_raw(&mut self, mut v: u64) {
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.buf.push(byte);
                return;
            }
            self.buf.push(byte | 0x80);
        }
    }

    fn tag(&mut self, field: u32, wire: u8) {
        self.varint_raw((u64::from(field) << 3) | u64::from(wire));
    }

    /// Enum or int32/uint32 field. Negative int32 values are sign-extended to 64 bits, as protobuf
    /// does for int32.
    pub fn int(&mut self, field: u32, v: i64) {
        self.tag(field, 0);
        self.varint_raw(v as u64);
    }

    pub fn bool(&mut self, field: u32, v: bool) {
        self.int(field, i64::from(v));
    }

    pub fn string(&mut self, field: u32, v: &str) {
        self.tag(field, 2);
        self.varint_raw(v.len() as u64);
        self.buf.extend_from_slice(v.as_bytes());
    }

    pub fn message(&mut self, field: u32, body: &[u8]) {
        self.tag(field, 2);
        self.varint_raw(body.len() as u64);
        self.buf.extend_from_slice(body);
    }

    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

/// Fetch the layout the editor holds for an app and controller slot, without changing it.
/// The raw bytes are returned too, so a caller can keep them as a backup.
pub fn start_editing_expr(app: u32, controller_index: u32) -> String {
    native_b64_call(&format!(
        "SteamClient.Input.StartEditingControllerConfigurationForAppIDAndControllerIndex({app},{controller_index})"
    ))
}

/// Decode a `StartEditing…` reply (base64 of `CMsgControllerConfiguration`) into a layout.
pub fn parse_editing_reply(b64: &str) -> Result<(Vec<u8>, ControllerLayout)> {
    let raw = base64_decode(b64)?;
    let layout = parse_layout(&raw)?;
    Ok((raw, layout))
}

/// Where a binding sits: the stock `CMsgSetControllerInputBinding` fields (task 210, field numbers
/// from `docs/api/protobufs.json`, values from the stock calls in the bundle).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingTarget {
    /// Field 1. The action set key of the layout (`Default` for the base set).
    pub action_set_key: String,
    /// Field 2. Layer set key; `None` for the base layer.
    pub action_set_layer_key: Option<String>,
    /// Field 3. `sets[].source_bindings[].key`, NOT the `k_EInputSource_*` bitmask.
    /// The captured Deck ABXY source key is 4; the unrelated input-source bitmask is 16.
    pub source_binding_key: i64,
    /// Field 4. Mode-shift flag.
    pub mode_shift: bool,
    /// Field 5. The input inside the source mode (`CMsgControllerInput.key`).
    pub input_key: i64,
    /// Field 6. Activator index (0 = the first, e.g. press).
    pub activator_index: i64,
    /// Field 7. Binding index inside the activator.
    pub binding_index: i64,
    /// Field 9. Source mode (`CMsgControllerMode.mode`).
    pub source_mode: i64,
    /// Field 10. Mode id (`CMsgControllerMode.modeid`).
    pub modeid: i64,
}

impl BindingTarget {
    /// Resolve the source address from the selected action set/layer and mode in the raw
    /// StartEditing reply. Top-level `modes[]` alone cannot identify a source binding.
    /// Reject missing/ambiguous paths instead of sending an input-source bitmask to Steam.
    pub fn resolve_source_binding(&self, raw: &[u8]) -> Result<Self> {
        use crate::controller_layout::{fields, Wire};
        fn message<'a>(f: &crate::controller_layout::Field<'a>) -> Result<&'a [u8]> {
            match f.wire {
                Wire::Len(b) => Ok(b),
                _ => Err(Error::Protocol("editor: expected nested message".into())),
            }
        }
        fn number(raw: &[u8], key: u32) -> Result<i64> {
            Ok(fields(raw)?
                .iter()
                .find_map(|f| match f.wire {
                    Wire::Varint(n) if f.number == key => Some(n as i64),
                    _ => None,
                })
                .unwrap_or(0))
        }
        fn named_set<'a>(raw: &'a [u8], field: u32, name: &str) -> Result<&'a [u8]> {
            for f in fields(raw)?.iter().filter(|f| f.number == field) {
                let set = message(f)?;
                if fields(set)?.iter().any(|f| f.number == 1 && f.wire == Wire::Len(name.as_bytes())) {
                    return Ok(set);
                }
            }
            Err(Error::Protocol(format!("editor: action set/layer {name:?} missing")))
        }
        let mut set = named_set(raw, 8, &self.action_set_key)?;
        if let Some(layer) = &self.action_set_layer_key {
            set = named_set(set, 4, layer)?;
        }
        let mut keys = Vec::new();
        for source in fields(set)?.iter().filter(|f| f.number == 6) {
            let source = message(source)?;
            for group in fields(source)?.iter().filter(|f| f.number == 2) {
                let mut group = message(group)?;
                if self.mode_shift {
                    let nested = fields(group)?;
                    let Some(shift) = nested.iter().find(|f| f.number == 7) else {
                        continue;
                    };
                    group = message(shift)?;
                }
                // CMsgControllerSourceGroup: mode=1, modeid=5. These differ from
                // top-level CMsgControllerMode (modeid=2).
                if number(group, 1)? == self.source_mode && number(group, 5)? == self.modeid {
                    keys.push(number(source, 1)?);
                }
            }
        }
        if keys.len() != 1 {
            return Err(Error::Protocol(format!(
                "editor: expected one source for mode {}, found {}",
                self.modeid, keys.len()
            )));
        }
        let mut resolved = self.clone();
        resolved.source_binding_key = keys[0];
        Ok(resolved)
    }
}

/// `CMsgControllerInputBinding` with a gamepad output button: field 1 type = 3 (Gamepad),
/// field 5 `gamepad_button` = `{ button }` (field 1). The button values are the stock
/// `gamepad_button.button` numbers; A = 3, B = 4, Y = 5, X = 6 (stock defaults, case table in
/// `chunk~2dcc5aaf7.js`, the default check for the Buttons source).
pub const BINDING_TYPE_GAMEPAD: i64 = 3;

pub fn gamepad_binding(button: i64) -> Vec<u8> {
    let mut inner = Writer::new();
    inner.int(1, button);
    let mut w = Writer::new();
    w.int(1, BINDING_TYPE_GAMEPAD);
    w.message(5, &inner.finish());
    w.finish()
}

/// `CMsgSetControllerInputBinding` for one binding: the target plus the new binding bytes.
pub fn set_input_binding_msg(target: &BindingTarget, new_binding: &[u8]) -> Vec<u8> {
    input_binding_msg(target, Some(new_binding))
}

/// The same message without field 8: Steam removes the addressed binding (stock «Удалить эту команду»,
/// `SetControllerInputBinding` with `new_binding` undefined). `binding_index` -1 appends when a binding is set.
pub fn remove_input_binding_msg(target: &BindingTarget) -> Vec<u8> {
    input_binding_msg(target, None)
}

fn input_binding_msg(target: &BindingTarget, new_binding: Option<&[u8]>) -> Vec<u8> {
    let mut w = Writer::new();
    w.string(1, &target.action_set_key);
    if let Some(layer) = &target.action_set_layer_key {
        w.string(2, layer);
    }
    w.int(3, target.source_binding_key);
    w.bool(4, target.mode_shift);
    w.int(5, target.input_key);
    w.int(6, target.activator_index);
    w.int(7, target.binding_index);
    if let Some(binding) = new_binding { w.message(8, binding); }
    w.int(9, target.source_mode);
    w.int(10, target.modeid);
    w.finish()
}

/// JS that sends base64 of a message to a native editing call and returns base64 of the reply.
/// Binary replies are converted inside the page (a binary value does not survive the JSON result).
fn native_b64_call(call: &str) -> String {
    format!(
        "(async()=>{{let r;try{{r=await {call}}}catch(e){{throw new Error(\"Steam Input \"+(e?.result??\"\")+\": \"+(e?.message??String(e)))}}if(typeof r==='string')return r;\
const u=r instanceof Uint8Array?r:new Uint8Array(r);let s='';\
for(let i=0;i<u.length;i+=8192)s+=String.fromCharCode.apply(null,u.subarray(i,i+8192));return btoa(s);}})()"
    )
}

/// The native write: `SetEditingControllerConfigurationInputBinding(idx, base64)`. Its reply is
/// base64 of the whole edited layout. Stock order: write, then the save call (see
/// [`save_editing_expr`]).
pub fn set_input_binding_expr(controller_index: u32, msg_b64: &str) -> String {
    native_b64_call(&format!(
        "SteamClient.Input.SetEditingControllerConfigurationInputBinding({controller_index},'{msg_b64}')"
    ))
}

/// Stock CMsgSetControllerSourceMode: one setting, without changing the mode.
/// CMsgControllerSetting.int_value is sint32 (zigzag), unlike enum addresses.
pub fn set_source_setting_msg(target: &BindingTarget, key: i64, value: i64) -> Vec<u8> {
    source_setting_msg(target, key, value, 2)
}

fn source_setting_msg(target: &BindingTarget, key: i64, value: i64, field: u32) -> Vec<u8> {
    let mut setting = Writer::new();
    setting.int(1, key);
    setting.int(field, (value << 1) ^ (value >> 63));
    let mut w = Writer::new();
    w.string(1, &target.action_set_key);
    if let Some(layer) = &target.action_set_layer_key { w.string(2, layer); }
    w.int(3, target.source_binding_key);
    w.bool(4, target.mode_shift);
    w.message(7, &setting.finish());
    w.int(9, target.modeid);
    w.finish()
}

/// Stock source-mode request. The quick-settings mode selector sets change_mode.
/// Mode-shift creation is the configurator's onCreateModeShift path with virtual menu support.
pub fn set_source_mode_msg(target: &BindingTarget, new_mode: i64) -> Vec<u8> {
    let mut w = Writer::new();
    w.string(1, &target.action_set_key);
    if let Some(layer) = &target.action_set_layer_key { w.string(2, layer); }
    w.int(3, target.source_binding_key);
    w.bool(4, target.mode_shift);
    w.int(5, new_mode);
    if target.mode_shift {
        w.bool(12, true);
    } else {
        w.int(9, 0);
        w.bool(10, true);
    }
    w.finish()
}

/// Steam's CMsgSwapControllerSourceModes, for two sources in one base action set.
fn swap_sources_msg(action_set: &str, left: i64, right: i64) -> Vec<u8> {
    let mut w = Writer::new();
    w.string(1, action_set);
    w.int(3, left);
    w.string(4, action_set);
    w.int(6, right);
    w.finish()
}

/// Source key the stock «Виртуальное меню» page addresses (`d.gS`, sp.js ~376024): the left stick.
pub const VIRTUAL_MENU_SOURCE: i64 = 3;
/// Menu styles the stock «Создать» menu offers: touch menu 11, radial menu 14, hotbar 19.
pub const VIRTUAL_MENU_MODES: [i64; 3] = [14, 11, 19];

/// Stock «Добавить виртуальное меню» (sp.js `$`): `SetControllerSourceMode` with `new_mode`, `new_name`,
/// `new_virtual_menu` (11) and `enable_virtual_menu_support` (12) on the left-stick source key; no mode shift.
pub fn create_virtual_menu_msg(action_set_key: &str, layer_key: Option<&str>, mode: i64, name: &str) -> Vec<u8> {
    let mut w = Writer::new();
    w.string(1, action_set_key);
    if let Some(layer) = layer_key { w.string(2, layer); }
    w.int(3, VIRTUAL_MENU_SOURCE);
    w.int(5, mode);
    w.string(8, name);
    w.bool(11, true);
    w.bool(12, true);
    w.finish()
}

/// `CMsgSetControllerInputActivator` (sp.js `d.Ob`): 1 set, 2 layer, 3 source key, 4 mode shift, 5 input,
/// 6 activator index, 7 `new_activation` (`EControllerActivationType`), 9 modeid. Without field 7 Steam removes the
/// activator (stock «Нет — удалить активатор»); the settings field 8 is not written here.
pub fn set_input_activator_msg(target: &BindingTarget, activation: Option<i64>) -> Vec<u8> {
    let mut w = Writer::new();
    w.string(1, &target.action_set_key);
    if let Some(layer) = &target.action_set_layer_key { w.string(2, layer); }
    w.int(3, target.source_binding_key);
    w.bool(4, target.mode_shift);
    w.int(5, target.input_key);
    w.int(6, target.activator_index);
    if let Some(kind) = activation { w.int(7, kind); }
    w.int(9, target.modeid);
    w.finish()
}

pub fn set_input_activator_expr(index: u32, msg_b64: &str) -> String {
    native_b64_call(&format!("SteamClient.Input.SetEditingControllerConfigurationInputActivator({index},'{msg_b64}')"))
}

pub fn set_source_setting_expr(index: u32, msg_b64: &str) -> String {
    native_b64_call(&format!("SteamClient.Input.SetEditingControllerConfigurationSourceMode({index},'{msg_b64}')"))
}

pub fn action_set_msg(key: &str, name: &str, copy_key: Option<&str>) -> Vec<u8> {
    let mut writer = Writer::new();
    writer.string(1, key);
    if copy_key.is_none() { writer.string(2, ""); }
    writer.string(3, name);
    if let Some(source) = copy_key { writer.string(4, source); }
    writer.finish()
}

/// `SaveEditingControllerConfiguration(idx, selectionType, bShared)`; the stock passes the
/// selection type of the edited config and `false` for a personal save.
pub fn save_editing_expr(controller_index: u32, selection_type: u32, shared: bool) -> String {
    format!(
        "(async()=>{{const result=await SteamClient.Input.SaveEditingControllerConfiguration({controller_index},{selection_type},{shared});if(result!=null&&result!==1&&result!==true)throw new Error('Steam Input save: '+String(result));return true}})()"
    )
}

impl Bridge {
    pub fn editor_set_action_set(&self, index: u32, key: &str, name: &str, copy_key: Option<&str>) -> Result<(Vec<u8>, ControllerLayout)> {
        if name.trim().is_empty() || name.chars().count() > 80 || key.is_empty() != copy_key.is_some() {
            return Err(Error::Protocol("Invalid action set operation".into()));
        }
        let payload = base64_encode(&action_set_msg(key, name, copy_key));
        let reply = self.eval_json_once(&native_b64_call(&format!("SteamClient.Input.SetEditingControllerConfigurationActionSet({index},'{payload}')")))?;
        parse_editing_reply(reply.as_str().ok_or_else(|| Error::Protocol("ActionSet: expected a layout reply".into()))?)
    }

    /// Load the editable layout of a game (read of the editor state; the stock UI does the same
    /// when a controller page opens). Returns the raw protobuf bytes and the decoded layout.
    pub fn editor_start(&self, app: u32, controller_index: u32) -> Result<(Vec<u8>, ControllerLayout)> {
        if app == 0 {
            return Err(Error::Protocol("editor: app id 0 refused".into()));
        }
        let v = self.eval_json_once(&start_editing_expr(app, controller_index))?;
        let b64 = v
            .as_str()
            .ok_or_else(|| Error::Protocol(format!("StartEditing: reply is not a string: {v}")))?;
        parse_editing_reply(b64)
    }

    /// Write one binding into the editing copy and return the edited layout (not saved yet).
    /// Resolve the source key from the current StartEditing reply, in the selected set/layer.
    /// The caller must supply bytes from this editing session, before any structural changes.
    /// One call, no retry: a failed call may have changed Steam; read the layout back.
    pub fn editor_set_input_binding(
        &self,
        controller_index: u32,
        editing_layout: &[u8],
        target: &BindingTarget,
        new_binding: &[u8],
    ) -> Result<(Vec<u8>, ControllerLayout)> {
        self.editor_input_binding_request(controller_index, editing_layout, target, Some(new_binding))
    }

    /// Remove one binding from the editing copy (see [`remove_input_binding_msg`]); the caller reads the reply back.
    pub fn editor_remove_input_binding(&self, controller_index: u32, editing_layout: &[u8], target: &BindingTarget) -> Result<(Vec<u8>, ControllerLayout)> {
        self.editor_input_binding_request(controller_index, editing_layout, target, None)
    }

    fn editor_input_binding_request(&self, controller_index: u32, editing_layout: &[u8], target: &BindingTarget, new_binding: Option<&[u8]>) -> Result<(Vec<u8>, ControllerLayout)> {
        let target = target.resolve_source_binding(editing_layout)?;
        let msg = input_binding_msg(&target, new_binding);
        let v = self.eval_json_once(&set_input_binding_expr(controller_index, &base64_encode(&msg)))?;
        let b64 = v
            .as_str()
            .ok_or_else(|| Error::Protocol(format!("SetEditingInputBinding: reply is not a string: {v}")))?;
        parse_editing_reply(b64)
    }

    /// Change the activation type of one activator (`Some`) or remove it (`None`) in the editing copy.
    /// No Save, no retry; the caller reads the returned copy back.
    pub fn editor_set_input_activator(&self, index: u32, raw: &[u8], target: &BindingTarget, activation: Option<i64>) -> Result<(Vec<u8>, ControllerLayout)> {
        let target = target.resolve_source_binding(raw)?;
        let msg = base64_encode(&set_input_activator_msg(&target, activation));
        let v = self.eval_json_once(&set_input_activator_expr(index, &msg))?;
        let reply = v.as_str().ok_or_else(|| Error::Protocol(format!("SetEditingInputActivator: reply is not a string: {v}")))?;
        parse_editing_reply(reply)
    }

    /// Write one source setting; no Save, no automatic retry. Read back the returned copy.
    pub fn editor_set_source_setting(&self, index: u32, raw: &[u8], target: &BindingTarget, key: i64, value: i64) -> Result<(Vec<u8>, ControllerLayout)> {
        let target = target.resolve_source_binding(raw)?;
        self.editor_source_request(index, set_source_setting_msg(&target, key, value))
    }

    /// The gyro button picker uses a signed 64-bit mask (setting field 7).
    pub fn editor_set_source_mask(&self, index: u32, raw: &[u8], target: &BindingTarget, key: i64, value: i64) -> Result<(Vec<u8>, ControllerLayout)> {
        let target = target.resolve_source_binding(raw)?;
        self.editor_source_request(index, source_setting_msg(&target, key, value, 7))
    }

    fn editor_source_request(&self, index: u32, request: Vec<u8>) -> Result<(Vec<u8>, ControllerLayout)> {
        let msg = base64_encode(&request);
        let v = self.eval_json_once(&set_source_setting_expr(index, &msg))?;
        let reply = v.as_str().ok_or_else(|| Error::Protocol(format!("SetEditingSourceMode: reply is not a string: {v}")))?;
        parse_editing_reply(reply)
    }

    /// Change the mode of one source in the editing copy; no Save, no retry. Read back the returned copy.
    /// The caller supplies the source address: a source without a group cannot be resolved from the layout.
    pub fn editor_set_source_mode(&self, index: u32, target: &BindingTarget, new_mode: i64) -> Result<(Vec<u8>, ControllerLayout)> {
        self.editor_source_request(index, set_source_mode_msg(target, new_mode))
    }

    /// Swap the left and right sources in the native editing copy. No Save and no automatic retry.
    pub fn editor_swap_sources(&self, index: u32, action_set: &str, left: i64, right: i64) -> Result<Vec<u8>> {
        let payload = base64_encode(&swap_sources_msg(action_set, left, right));
        let expr = native_b64_call(&format!("SteamClient.Input.SwapControllerConfigurationSourceModes({index},'{payload}')"));
        let reply = self.eval_json_once(&expr)?;
        let encoded = reply.as_str().ok_or_else(|| Error::Protocol("SwapControllerSourceModes: expected a layout reply".into()))?;
        parse_editing_reply(encoded).map(|(raw, _)| raw)
    }

    /// Create a virtual menu in the editing copy (no Save). The caller compares the returned layout.
    pub fn editor_create_virtual_menu(&self, index: u32, action_set_key: &str, layer_key: Option<&str>, mode: i64, name: &str) -> Result<(Vec<u8>, ControllerLayout)> {
        if name.trim().is_empty() || name.chars().count() > 80 || !VIRTUAL_MENU_MODES.contains(&mode) {
            return Err(Error::Protocol("Invalid virtual menu request".into()));
        }
        self.editor_source_request(index, create_virtual_menu_msg(action_set_key, layer_key, mode, name))
    }

    /// Update the button chord of an already-created mode shift.
    pub fn editor_set_mode_shift_buttons(&self, index: u32, target: &BindingTarget, buttons: &[i64]) -> Result<(Vec<u8>, ControllerLayout)> {
        if !target.mode_shift { return Err(Error::Protocol("Expected a mode shift target".into())); }
        let mut writer = Writer::new();
        writer.string(1, &target.action_set_key);
        if let Some(layer) = &target.action_set_layer_key { writer.string(2, layer); }
        writer.int(3, target.modeid);
        writer.int(4, target.source_binding_key);
        for &button in buttons { writer.int(5, button); }
        let payload = base64_encode(&writer.finish());
        let expression = native_b64_call(&format!("SteamClient.Input.SetControllerConfigurationModeShiftBinding({index},'{payload}')"));
        let reply = self.eval_json_once(&expression)?;
        let reply = reply.as_str().ok_or_else(|| Error::Protocol("SetControllerModeShiftBinding: reply is not a string".into()))?;
        parse_editing_reply(reply)
    }

    /// Save the editing copy (stock: `SaveEditingControllerConfiguration`). One call, no retry.
    pub fn editor_save(&self, controller_index: u32, selection_type: u32, shared: bool) -> Result<()> {
        self.eval_json_once(&save_editing_expr(controller_index, selection_type, shared))?;
        Ok(())
    }

    pub fn editor_save_personal(&self, app: u32, index: u32, url: &str) -> Result<()> {
        crate::controller_list::check_personal_url(url)?;
        let active = self.layout_active(app, index)?.filter(|active| active.url == url)
            .ok_or_else(|| Error::Protocol("The personal layout is no longer active".into()))?;
        self.editor_save(index, active.selection_type, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swap_sources_encodes_both_addresses_in_the_same_action_set() {
        use crate::controller_layout::Wire;
        let message = swap_sources_msg("Driving", 3, 12);
        let fields = crate::controller_layout::fields(&message).unwrap();
        assert_eq!(fields.iter().map(|f| f.number).collect::<Vec<_>>(), [1, 3, 4, 6]);
        assert!(matches!(fields[0].wire, Wire::Len(b"Driving")));
        assert!(matches!(fields[1].wire, Wire::Varint(3)));
        assert!(matches!(fields[2].wire, Wire::Len(b"Driving")));
        assert!(matches!(fields[3].wire, Wire::Varint(12)));
    }

    #[test]
    fn action_set_operations_use_the_stock_fields() {
        assert_eq!(action_set_msg("", "New", Some("Base")), b"\x0a\x00\x1a\x03New\x22\x04Base");
        assert_eq!(action_set_msg("Base", "New", None), b"\x0a\x04Base\x12\x00\x1a\x03New");
    }

    #[test]
    fn virtual_menu_creation_uses_the_stock_fields() {
        // sp.js: { action_set_key, action_set_layer_key, source_binding_key: 3, new_mode, new_virtual_menu, new_name, enable_virtual_menu_support }
        assert_eq!(create_virtual_menu_msg("Default", None, 14, "Menu"),
            b"\x0a\x07Default\x18\x03\x28\x0e\x42\x04Menu\x58\x01\x60\x01");
        assert_eq!(create_virtual_menu_msg("Default", Some("L"), 11, "M"),
            b"\x0a\x07Default\x12\x01L\x18\x03\x28\x0b\x42\x01M\x58\x01\x60\x01");
        assert_eq!(VIRTUAL_MENU_MODES, [14, 11, 19]);
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        for data in [&b""[..], b"f", b"fo", b"foo", b"\x00\xff\x10\x80abc"] {
            assert_eq!(base64_decode(&base64_encode(data)).unwrap(), data);
        }
    }

    #[test]
    fn base64_rejects_bad_input() {
        assert!(base64_decode("Zg=").is_err());
        assert!(base64_decode("Z!==").is_err());
        assert!(base64_decode("Zg==Zg==").is_err());
        assert!(base64_decode("=Zg=").is_err());
    }

    #[test]
    fn varint_and_fields_match_protobuf_encoding() {
        // field 1 varint 150 -> 08 96 01 (protobuf spec example)
        let mut w = Writer::new();
        w.int(1, 150);
        assert_eq!(w.finish(), vec![0x08, 0x96, 0x01]);
        // field 2 string "testing" -> 12 07 + bytes
        let mut w = Writer::new();
        w.string(2, "testing");
        assert_eq!(w.finish(), [&[0x12, 0x07][..], b"testing"].concat());
        // bool true on field 4 -> 20 01
        let mut w = Writer::new();
        w.bool(4, true);
        assert_eq!(w.finish(), vec![0x20, 0x01]);
    }

    #[test]
    fn negative_int32_is_ten_byte_varint() {
        let mut w = Writer::new();
        w.int(6, -1);
        let b = w.finish();
        assert_eq!(b[0], 0x30);
        assert_eq!(b.len(), 11);
    }

    #[test]
    fn nested_message_has_its_length_prefix() {
        let mut inner = Writer::new();
        inner.int(1, 1);
        let inner = inner.finish();
        let mut w = Writer::new();
        w.message(8, &inner);
        assert_eq!(w.finish(), vec![0x42, 0x02, 0x08, 0x01]);
    }

    #[test]
    fn start_expression_names_app_and_index_only() {
        let e = start_editing_expr(2379780, 15);
        assert!(e.contains("StartEditingControllerConfigurationForAppIDAndControllerIndex(2379780,15)"));
    }

    #[test]
    fn gamepad_binding_bytes_match_the_layout_on_the_deck() {
        // Balatro A binding as read from the live layout: type 3, gamepad_button { button 3 }.
        assert_eq!(gamepad_binding(3), vec![0x08, 0x03, 0x2a, 0x02, 0x08, 0x03]);
        assert_eq!(gamepad_binding(4), vec![0x08, 0x03, 0x2a, 0x02, 0x08, 0x04]);
    }

    #[test]
    fn set_binding_message_has_the_stock_field_numbers() {
        let target = BindingTarget {
            action_set_key: "Default".into(),
            action_set_layer_key: None,
            source_binding_key: 16,
            mode_shift: false,
            input_key: 12,
            activator_index: 0,
            binding_index: 0,
            source_mode: 3,
            modeid: 0,
        };
        let msg = set_input_binding_msg(&target, &gamepad_binding(4));
        // Field 1 is the action set key "Default" (0a 07 ...).
        assert_eq!(&msg[..9], &[0x0a, 0x07, b'D', b'e', b'f', b'a', b'u', b'l', b't']);
        let fields = crate::controller_layout::fields(&msg).unwrap();
        let numbers: Vec<u32> = fields.iter().map(|f| f.number).collect();
        assert_eq!(numbers, vec![1, 3, 4, 5, 6, 7, 8, 9, 10]);
        let n = |num: u32| fields.iter().find(|f| f.number == num).unwrap().wire;
        assert_eq!(n(3), crate::controller_layout::Wire::Varint(16));
        assert_eq!(n(5), crate::controller_layout::Wire::Varint(12));
        assert_eq!(n(9), crate::controller_layout::Wire::Varint(3));
    }

    #[test]
    fn no_op_a_binding_matches_the_deck_probe_bytes() {
        // Fixture: the no-op write A -> A (source 16, mode 3, modeid 0, input 12, binding 3) that
        // the Deck probes sent on 2026-11-13 (task 218, `20-probe-focus-restart.js`). Steam
        // answered 12100 to it; this pins the bytes the shell sends, so a change is visible.
        let target = BindingTarget {
            action_set_key: "Default".into(),
            action_set_layer_key: None,
            source_binding_key: 16,
            mode_shift: false,
            input_key: 12,
            activator_index: 0,
            binding_index: 0,
            source_mode: 3,
            modeid: 0,
        };
        let msg = set_input_binding_msg(&target, &gamepad_binding(3));
        assert_eq!(base64_encode(&msg), "CgdEZWZhdWx0GBAgACgMMAA4AEIGCAMqAggDSANQAA==");
    }

    #[test]
    fn source_address_comes_from_selected_set_not_input_bitmask() {
        // Minimal, non-personal projection of task-210's Deck reply:
        // sets[Default].source_bindings[key=4].active_group {mode=3,modeid=0}.
        let raw = b"\x42\x13\x0a\x07Default\x32\x08\x08\x04\x12\x04\x08\x03\x28\x00";
        let target = BindingTarget {
            action_set_key: "Default".into(),
            action_set_layer_key: None,
            source_binding_key: 16,
            mode_shift: false,
            input_key: 12,
            activator_index: 0,
            binding_index: 0,
            source_mode: 3,
            modeid: 0,
        };
        let resolved = target.resolve_source_binding(raw).unwrap();
        assert_eq!(resolved.source_binding_key, 4);
        let msg = set_input_binding_msg(&resolved, &gamepad_binding(3));
        assert_eq!(base64_encode(&msg), "CgdEZWZhdWx0GAQgACgMMAA4AEIGCAMqAggDSANQAA==");
        let mut wrong_mode = target.clone();
        wrong_mode.modeid = 9;
        assert!(wrong_mode.resolve_source_binding(raw).is_err());
        let mut wrong_layer = target.clone();
        wrong_layer.action_set_layer_key = Some("missing".into());
        assert!(wrong_layer.resolve_source_binding(raw).is_err());
        assert!(target.resolve_source_binding(&raw[..raw.len() - 1]).is_err());
    }

    #[test]
    fn source_address_respects_layer_and_mode_shift_and_rejects_ambiguity() {
        let mut shift = Writer::new();
        shift.int(1, 3);
        shift.int(5, 42);
        let mut group = Writer::new();
        group.int(1, 3);
        group.int(5, 0);
        group.message(7, &shift.finish());
        let mut source = Writer::new();
        source.int(1, 4);
        source.message(2, &group.finish());
        let source = source.finish();
        let layout = |duplicate: bool| {
            let mut layer = Writer::new();
            layer.string(1, "layer");
            layer.message(6, &source);
            if duplicate {
                layer.message(6, &source);
            }
            let mut set = Writer::new();
            set.string(1, "Default");
            set.message(4, &layer.finish());
            let mut raw = Writer::new();
            raw.message(8, &set.finish());
            raw.finish()
        };
        let target = BindingTarget {
            action_set_key: "Default".into(),
            action_set_layer_key: Some("layer".into()),
            source_binding_key: 16,
            mode_shift: true,
            input_key: 12,
            activator_index: 0,
            binding_index: 0,
            source_mode: 3,
            modeid: 42,
        };
        assert_eq!(target.resolve_source_binding(&layout(false)).unwrap().source_binding_key, 4);
        assert!(target.resolve_source_binding(&layout(true)).is_err());
        let mut base = target.clone();
        base.action_set_layer_key = None;
        assert!(base.resolve_source_binding(&layout(false)).is_err());
    }

    #[test]
    fn set_binding_with_layer_writes_field_two() {
        let target = BindingTarget {
            action_set_key: "Default".into(),
            action_set_layer_key: Some("Layer1".into()),
            source_binding_key: 16,
            mode_shift: true,
            input_key: 12,
            activator_index: 1,
            binding_index: 2,
            source_mode: 3,
            modeid: 5,
        };
        let msg = set_input_binding_msg(&target, &gamepad_binding(6));
        let fields = crate::controller_layout::fields(&msg).unwrap();
        assert!(fields.iter().any(|f| f.number == 2));
        assert!(fields.iter().any(|f| f.number == 4 && f.wire == crate::controller_layout::Wire::Varint(1)));
    }

    #[test]
    fn append_and_remove_binding_messages_follow_the_stock_shape() {
        use crate::controller_layout::{fields, Wire};
        let target = BindingTarget { action_set_key: "Default".into(), action_set_layer_key: None, source_binding_key: 7, mode_shift: false,
            input_key: 13, activator_index: 0, binding_index: -1, source_mode: 10, modeid: 4 };
        fn get(m: &[u8], n: u32) -> Option<Wire<'_>> { fields(m).unwrap().into_iter().find(|f| f.number == n).map(|f| f.wire) }
        // int32 -1 is a sign-extended 10-byte varint, exactly what the stock writer emits for "append".
        assert_eq!(get(&set_input_binding_msg(&target, &gamepad_binding(3)), 7), Some(Wire::Varint(u64::MAX)));
        assert!(get(&set_input_binding_msg(&target, &gamepad_binding(3)), 8).is_some());
        let mut target = target;
        target.binding_index = 0;
        assert_eq!(get(&remove_input_binding_msg(&target), 8), None);
        assert_eq!(get(&remove_input_binding_msg(&target), 9), Some(Wire::Varint(10)));
    }

    #[test]
    fn set_activator_message_has_the_stock_field_numbers() {
        use crate::controller_layout::{fields, Wire};
        let target = BindingTarget { action_set_key: "Default".into(), action_set_layer_key: None, source_binding_key: 7, mode_shift: false,
            input_key: 13, activator_index: 1, binding_index: 0, source_mode: 10, modeid: 4 };
        fn get(m: &[u8], n: u32) -> Option<Wire<'_>> { fields(m).unwrap().into_iter().find(|f| f.number == n).map(|f| f.wire) }
        let with = set_input_activator_msg(&target, Some(6));
        assert_eq!(get(&with, 3), Some(Wire::Varint(7)));
        assert_eq!(get(&with, 5), Some(Wire::Varint(13)));
        assert_eq!(get(&with, 6), Some(Wire::Varint(1)));
        assert_eq!(get(&with, 7), Some(Wire::Varint(6)));
        assert_eq!(get(&with, 9), Some(Wire::Varint(4)));
        // Removal: no new_activation field at all.
        assert_eq!(get(&set_input_activator_msg(&target, None), 7), None);
        assert!(set_input_activator_expr(15, "CAM=").contains("SetEditingControllerConfigurationInputActivator(15,'CAM=')"));
    }

    #[test]
    fn set_expression_sends_only_base64_and_index() {
        let e = set_input_binding_expr(15, "CAM=");
        assert!(e.contains("SetEditingControllerConfigurationInputBinding(15,'CAM=')"));
        assert!(e.contains("btoa"));
        let s = save_editing_expr(15, 0, false);
        assert!(s.contains("SaveEditingControllerConfiguration(15,0,false)"));
    }
}
