//! Steam editor session data. Keep raw bindings for lossless Undo (including icons/settings).
use steam_assets::i18n::tr_ref;
use steam_bridge::{controller_edit::BindingTarget, controller_layout::{fields, parse_layout, Wire}, Result, Error};
#[derive(Clone, Debug)]
pub struct BindingRow { pub label: String, pub target: BindingTarget, pub binding: Vec<u8> }
pub(super) fn msg(raw: &[u8], field: u32) -> Result<Vec<&[u8]>> {
    Ok(fields(raw)?.into_iter().filter_map(|f| match f.wire { Wire::Len(v) if f.number == field => Some(v), _ => None }).collect())
}
pub(super) fn number(raw: &[u8], field: u32) -> Result<i64> {
    Ok(fields(raw)?.into_iter().find_map(|f| match f.wire { Wire::Varint(v) if f.number == field => Some(v as i64), _ => None }).unwrap_or(0))
}
pub fn binding_at(raw: &[u8], t: &BindingTarget) -> Result<Vec<u8>> {
    for m in msg(raw, 9)? {
        if number(m,1)? != t.source_mode || number(m,2)? != t.modeid { continue; }
        for i in msg(m,4)? {
            if number(i,1)? != t.input_key { continue; }
            let acts=msg(i,2)?;
            if let Some(act)=acts.get(t.activator_index as usize) {
                return msg(act,2)?.get(t.binding_index as usize).map(|b|b.to_vec()).ok_or_else(||Error::Protocol(tr_ref("controller.binding_missing_from_the_steam_reply").into()));
            }
        }
    }
    Err(Error::Protocol(tr_ref("controller.input_missing_from_the_steam_reply").into()))
}
/// Activation type (`EControllerActivationType`) of the addressed activator, if the activator exists.
pub fn activation_at(raw:&[u8],t:&BindingTarget)->Option<i64>{
    for m in msg(raw,9).ok()? {
        if number(m,1).ok()? != t.source_mode || number(m,2).ok()? != t.modeid { continue; }
        for i in msg(m,4).ok()? {
            if number(i,1).ok()? != t.input_key { continue; }
            return msg(i,2).ok()?.get(t.activator_index as usize).and_then(|a|number(a,1).ok());
        }
    }
    None
}
/// Number of activators of the addressed input (0 when the input is empty or absent).
pub fn activator_count(raw:&[u8],t:&BindingTarget)->usize{
    msg(raw,9).ok().into_iter().flatten().filter(|m|number(m,1).ok()==Some(t.source_mode)&&number(m,2).ok()==Some(t.modeid))
        .flat_map(|m|msg(m,4).ok().unwrap_or_default()).filter(|i|number(i,1).ok()==Some(t.input_key)).map(|i|msg(i,2).map(|a|a.len()).unwrap_or(0)).sum()
}
pub fn rows(raw: &[u8]) -> Result<Vec<BindingRow>> {
    let layout=parse_layout(raw)?;
    if !layout.error_msg.is_empty(){return Err(Error::Protocol(layout.error_msg));}
    let mut out=Vec::new();
    for set in &layout.action_sets {
        for mode in &layout.modes {
            for input in &mode.inputs {
                let label=match input.key {12=>"A",10=>"B",11=>"X",9=>"Y",51=>tr_ref("controller.select"),52=>tr_ref("controller.start"),53=>tr_ref("controller.left_bumper"),54=>tr_ref("controller.right_bumper"),55=>"L4",56=>"R4",57=>"L5",58=>"R5",
                    // Other sources (D-pad, triggers, pads, sticks): rows are named by controller_pages, not by this label.
                    1=>"N",3=>"E",5=>"S",7=>"W",13=>"Click",15=>"Edge",50=>"Touch",_=>continue};
                let t=BindingTarget {action_set_key:set.key.clone(),action_set_layer_key:None,source_binding_key:0,mode_shift:false,input_key:input.key,activator_index:0,binding_index:0,source_mode:mode.mode,modeid:mode.modeid as i64};
                let Ok(t)=t.resolve_source_binding(raw) else {continue};
                for (ai,act) in input.activators.iter().enumerate() { for (bi,_) in act.bindings.iter().enumerate() {
                    let mut t=t.clone();t.activator_index=ai as i64;t.binding_index=bi as i64;
                    let binding=binding_at(raw,&t)?;
                    out.push(BindingRow {label: if layout.action_sets.len()>1 || ai>0 || bi>0 {format!("{} · {} · {}.{}",set.display_name,label,ai+1,bi+1)}else{label.into()},target:t,binding});
                }}
            }
        }
    }
    // Stock Buttons puts face buttons first, A/B/X/Y.
    out.sort_by_key(|r|match r.target.input_key {12=>0,10=>1,11=>2,9=>3,n=>n});
    Ok(out)
}
/// Inputs shown on the «Кнопки» page (face and system buttons); the rest belong to the other editor pages.
pub fn is_button_input(key:i64)->bool{matches!(key,12|10|11|9|51..=58)
}
pub fn command(kind:i64, field:u32, value:i64) -> Vec<u8> {
    use steam_bridge::controller_edit::Writer;
    let mut inner=Writer::new();inner.int(1,value);
    let mut w=Writer::new();w.int(1,kind);w.message(field,&inner.finish());w.finish()
}
pub fn identity(raw:&[u8]) -> Option<(i64,i64)> {
    let kind=number(raw,1).ok()?;
    let field=match kind {1=>3,2=>4,3=>5,4=>6,7=>9,_=>return None};
    Some((kind,number(msg(raw,field).ok()?.first()?,1).ok()?))
}

#[cfg(test)] mod tests {
    use super::*;
    use steam_bridge::controller_edit::{Writer, gamepad_binding};
    fn nested(field:u32, raw:&[u8])->Vec<u8>{let mut w=Writer::new();w.message(field,raw);w.finish()}
    fn layout()->Vec<u8>{
        let mut group=Writer::new();group.int(1,3);group.int(5,0);
        let mut source=Writer::new();source.int(1,4);source.message(2,&group.finish());
        let mut set=Writer::new();set.string(1,"Default");set.message(6,&source.finish());
        let mut act=Writer::new();act.int(1,1);act.message(2,&gamepad_binding(3));
        let mut input=Writer::new();input.int(1,12);input.message(2,&act.finish());
        let mut mode=Writer::new();mode.int(1,3);mode.int(2,0);mode.message(4,&input.finish());
        [nested(8,&set.finish()),nested(9,&mode.finish())].concat()
    }
    #[test] fn real_source_address_and_lossless_binding_readback(){let raw=layout();let r=rows(&raw).unwrap();assert_eq!(r.len(),1);assert_eq!(r[0].target.source_binding_key,4);assert_eq!(r[0].target.input_key,12);assert_eq!(r[0].binding,gamepad_binding(3));assert_eq!(identity(&r[0].binding),Some((3,3)));assert!(rows(&raw[..raw.len()-1]).is_err());}
    #[test] fn missing_target_is_an_error(){let raw=layout();let mut target=rows(&raw).unwrap()[0].target.clone();target.input_key=999;assert!(binding_at(&raw,&target).is_err());}
}

pub fn string(raw:&[u8],field:u32)->Result<String>{Ok(msg(raw,field)?.first().map(|b|String::from_utf8_lossy(b).into_owned()).unwrap_or_default())}
#[derive(Clone,Debug)]
pub struct Parameter {pub label:String,pub value:i64,pub min:i64,pub max:i64,pub choices:Vec<String>}
impl Parameter {
    fn number(label:&str,value:i64,min:i64,max:i64)->Self{Self{label:label.into(),value,min,max,choices:vec![]}}
    fn toggle(label:&str,value:bool)->Self{Self{label:label.into(),value:value as i64,min:0,max:1,choices:vec![tr_ref("controller.none_2").into(),tr_ref("controller.yes").into()]}}
    pub fn text(&self)->String{self.choices.get(self.value as usize).cloned().unwrap_or_else(||self.value.to_string())}
    pub fn step(&mut self,dir:i64){self.value=(self.value+dir).clamp(self.min,self.max);}
}
#[derive(Clone,Debug)]
pub struct CommandDialog {pub target:BindingTarget,pub action:i64,pub parameters:Vec<Parameter>,pub cursor:usize,sets:Vec<(String,Option<i64>)>}
impl CommandDialog {
    pub fn new(raw:&[u8],target:BindingTarget,action:i64)->Result<Option<Self>> {
        let mut sets=Vec::new();let mut parameters=Vec::new();
        match action {
            0|25|26|27=>{
                let mut names=Vec::new();
                for set in msg(raw,8)? {
                    let key=string(set,1)?;
                    if action==0 && key!=target.action_set_key {names.push(string(set,2)?);sets.push((key.clone(),None));}
                    if action!=0 && key==target.action_set_key {for layer in msg(set,4)? {names.push(string(layer,2)?);sets.push((string(layer,1)?,None));}}
                }
                if action==0 {for (label,n) in [(tr_ref("controller.none_2"),32767),(tr_ref("controller.next_set"),32766),(tr_ref("controller.previous_set"),32765)]{names.push(label.into());sets.push((String::new(),Some(n)));}}
                if names.is_empty(){return Err(Error::Protocol(tr_ref("controller.the_layout_has_no_action_set_layers").into()))}
                parameters.push(Parameter{label:if action==0{tr_ref("controller.action_set_2")}else{tr_ref("controller.layer")}.into(),value:0,min:0,max:names.len() as i64-1,choices:names});
                parameters.push(Parameter::toggle(tr_ref("controller.show_name"),true));parameters.push(Parameter::toggle(tr_ref("controller.play_sound"),true));
            }
            3|36=>{parameters.push(Parameter::number("X",0,-32768,32767));parameters.push(Parameter::number("Y",0,-32768,32767));if action==3 {parameters.push(Parameter::toggle(tr_ref("controller.restore_cursor"),true));}}
            31=>parameters.push(Parameter::number(tr_ref("controller.player_number"),1,1,4)),
            37=>{parameters.push(Parameter::number(tr_ref("controller.delay_ms"),33,0,100));parameters.push(Parameter::number(tr_ref("controller.reset_angle"),90,45,135));}
            38=>{parameters.push(Parameter::number(tr_ref("controller.duration_ms"),250,0,1000));parameters.push(Parameter::number(tr_ref("controller.turn_angle"),360,-360,360));}
            39=>{parameters.push(Parameter{label:tr_ref("controller.direction_from").into(),value:0,min:0,max:5,choices:[tr_ref("controller.left_stick"),tr_ref("controller.right_stick"),tr_ref("controller.left_trackpad_2"),tr_ref("controller.right_trackpad_2"),tr_ref("controller.dpad"),tr_ref("controller.face_buttons_2")].into_iter().map(String::from).collect()});parameters.push(Parameter::number(tr_ref("controller.duration_ms"),75,0,1000));parameters.push(Parameter::toggle(tr_ref("controller.last_direction_in_deadzone"),false));}
            _=>return Ok(None),
        }
        Ok(Some(Self{target,action,parameters,cursor:0,sets}))
    }
    pub fn binding(&self)->Vec<u8>{
        use steam_bridge::controller_edit::Writer;
        let p=|i:usize|self.parameters[i].value;
        let mut data=Writer::new();let subfield=match self.action {
            0|25|26|27=>{let (key,preset)=&self.sets[p(0) as usize];if let Some(v)=preset{data.int(1,*v)}else{data.string(2,key)}data.bool(3,p(1)!=0);data.bool(4,p(2)!=0);6}
            3|36=>{for (field,v) in [(1,p(0)),(2,p(1))]{data.int(field,(v<<1)^(v>>63));}data.bool(3,self.action==3&&p(2)!=0);2}
            31=>{data.int(1,p(0));8}
            37=>{data.int(1,180);data.int(2,p(0));data.int(3,p(1));3}
            38=>{data.int(1,p(1));data.int(2,p(0));4}
            39=>{data.int(1,[3,12,1,2,13,4][p(0) as usize]);data.int(2,p(1));data.bool(3,p(2)!=0);5}
            _=>unreachable!(),
        };
        let mut action=Writer::new();action.int(1,self.action);action.message(subfield,&data.finish());
        let mut b=Writer::new();b.int(1,7);b.message(9,&action.finish());b.finish()
    }
}
/// Compare requested command data, ignoring Steam's generated display metadata.
pub fn same_command(a:&[u8],b:&[u8])->Result<bool>{
    fn canonical(raw:&[u8],message_fields:&[u32])->Result<Vec<(u32,Vec<u8>)>>{
        let mut out=Vec::new();for f in fields(raw)? {let bytes=match f.wire {
            Wire::Varint(0)=>continue,
            Wire::Varint(n)=>n.to_le_bytes().to_vec(),
            Wire::Len(v) if message_fields.contains(&f.number)=>format!("{:?}",canonical(v,&[])?).into_bytes(),
            Wire::Len(v) if v.is_empty()=>continue,
            Wire::Len(v)=>v.to_vec(),_=>return Err(Error::Protocol(tr_ref("controller.unknown_command_fields").into()))};out.push((f.number,bytes));}out.sort();Ok(out)
    }
    // Work on the command submessage only; key_binding_data may contain generated labels/icons.
    let kind=number(b,1)?;if number(a,1)?!=kind{return Ok(false)}
    let field=match kind {1=>3,2=>4,3=>5,4=>6,6=>8,7=>9,_=>return Ok(a==b)};
    let aa=msg(a,field)?;let bb=msg(b,field)?;
    match (aa.first(),bb.first()) {(Some(a),Some(b))=>Ok(canonical(a,if kind==7 {&[2,3,4,5,6,7,8]}else{&[]})?==canonical(b,if kind==7 {&[2,3,4,5,6,7,8]}else{&[]})?),_=>Ok(false)}
}

/// Game-actions tab is present only for SIAPI actions supplied by this layout.
pub fn game_actions(raw:&[u8],target:&BindingTarget)->Result<Vec<(String,Vec<u8>)>> {
    use steam_bridge::controller_edit::Writer;
    let mut out=Vec::new();
    for set in msg(raw,8)? {
        if string(set,1)?!=target.action_set_key{continue}
        let mut sets=vec![set];
        if let Some(layer)=&target.action_set_layer_key {sets.extend(msg(set,4)?.into_iter().filter(|s|string(s,1).ok().as_ref()==Some(layer)));}
        for set in sets {for t in msg(set,5)? {for action in msg(t,2)? {
            let mut data=Writer::new();data.string(1,&string(set,1)?);data.string(2,&string(action,1)?);
            let mut b=Writer::new();b.int(1,6);b.message(8,&data.finish());out.push((string(action,2)?,b.finish()));
        }}}
    }Ok(out)
}
#[cfg(test)] mod command_tests {
    use super::*;
    use steam_bridge::controller_edit::Writer;
    #[test] fn native_metadata_is_ignored_but_command_parameters_are_verified(){
        let requested=command(3,5,3);let mut meta=Writer::new();meta.string(2,"A personnalisé");let mut got=Writer::new();got.int(1,3);got.message(2,&meta.finish());let mut gp=Writer::new();gp.int(1,3);got.message(5,&gp.finish());assert!(same_command(&requested,&got.finish()).unwrap());assert!(!same_command(&requested,&command(3,5,4)).unwrap());
        let t=tests_target();let mut d=CommandDialog::new(&[],t,36).unwrap().unwrap();d.parameters[0].value=-42;let b=d.binding();assert!(same_command(&b,&b).unwrap());d.parameters[0].value=-41;assert!(!same_command(&b,&d.binding()).unwrap());
    }
    fn tests_target()->BindingTarget{BindingTarget{action_set_key:"Default".into(),action_set_layer_key:None,source_binding_key:4,mode_shift:false,input_key:12,activator_index:0,binding_index:0,source_mode:3,modeid:0}}
    #[test] fn stock_set_presets_and_camera_parameters_are_serialized(){let d=CommandDialog::new(&[],tests_target(),0).unwrap().unwrap();let b=d.binding();assert!(same_command(&b,&b).unwrap());let action=msg(&b,9).unwrap()[0];let set=msg(action,6).unwrap()[0];assert_eq!(number(set,1).unwrap(),32767);for action in [3,31,36,37,38,39]{let d=CommandDialog::new(&[],tests_target(),action).unwrap().unwrap();let b=d.binding();assert!(same_command(&b,&b).unwrap());}}
}

/// Real editor writes are opt-in during a dry-run device test.
pub fn writes_allowed(dryrun:bool,test_override:bool)->bool{!dryrun || test_override}
#[cfg(test)] mod dryrun_tests {#[test] fn writes_require_explicit_device_test_override(){assert!(!super::writes_allowed(true,false));assert!(super::writes_allowed(false,false));assert!(super::writes_allowed(true,true));}}
