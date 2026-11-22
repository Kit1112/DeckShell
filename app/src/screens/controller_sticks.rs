//! Stock joystick_move (mode 6) settings: chunk~2dcc5aaf7.js mode table.
//! Addresses come from action-set source groups; values/ranges come from Steam.
use steam_assets::i18n::{tr_args, tr_ref};
use super::controller_live::{msg, number, string};
use steam_bridge::{controller_edit::BindingTarget, Error, Result};
#[derive(Clone, Debug)]
pub struct Stick { pub label: String, pub target: BindingTarget, pub mode: i64 }
#[derive(Clone, Debug)]
pub struct Setting { pub key: i64, pub label: &'static str, pub value: i64, pub min: i64, pub max: i64, pub choice_keys: &'static [&'static str], pub toggle: bool }
impl Setting {
    pub fn text(&self) -> String { self.choice_keys.get(self.value as usize).map(|key| tr_ref(key).to_owned()).unwrap_or_else(||match self.key {31|32=>format!("{}%",self.value),45=>format!("{}°",self.value),_=>self.value.to_string()}) }
    pub fn next(&self, dir: i64) -> i64 {
        if self.toggle {return 1-self.value;}
        if !self.choice_keys.is_empty() {return (self.value+dir).rem_euclid(self.choice_keys.len() as i64);}
        (self.value+dir*if [7,12,13,18,19].contains(&self.key){100}else{1}).clamp(self.min,self.max)
    }
}
fn signed(raw:&[u8],field:u32)->Result<i64>{let n=number(raw,field)?;Ok((n>>1)^-(n&1))}
pub fn sticks(raw:&[u8])->Result<Vec<Stick>> {
    let mut out=Vec::new();
    for set in msg(raw,8)? {
        let mut layers=vec![(set,None)];
        for layer in msg(set,4)? {layers.push((layer,Some(string(layer,1)?)));}
        for (layer,layer_key) in layers {for source in msg(layer,6)? {
            let key=number(source,1)?;if key!=3 && key!=12 {continue;}
            for group in msg(source,2)? {
                let mode=number(group,1)?;
                let target=BindingTarget {action_set_key:string(set,1)?,action_set_layer_key:layer_key.clone(),source_binding_key:key,mode_shift:false,input_key:0,activator_index:0,binding_index:0,source_mode:mode,modeid:number(group,5)?}.resolve_source_binding(raw)?;
                let mut label=if key==3 {tr_ref("controller.left_joystick_2")}else{tr_ref("controller.right_joystick_2")}.to_string();
                if msg(raw,8)?.len()>1 {label=format!("{} · {}",string(set,2)?,label);}
                if let Some(layer)=&layer_key {label=format!("{label} · {layer}");}
                out.push(Stick{label,target,mode});
            }
        }}
    }
    Ok(out)
}
pub(super) fn group<'a>(raw:&'a [u8],t:&BindingTarget)->Result<&'a [u8]> {
    for set in msg(raw,8)? {
        if string(set,1)?!=t.action_set_key {continue;}
        let layers=if let Some(key)=&t.action_set_layer_key {msg(set,4)?.into_iter().filter(|s|string(s,1).ok().as_ref()==Some(key)).collect()}else{vec![set]};
        for layer in layers {for source in msg(layer,6)? {
            if number(source,1)?!=t.source_binding_key {continue;}
            for group in msg(source, 2)? {
                let group = if t.mode_shift {
                    let Some(shift) = msg(group, 7)?.first().copied() else { continue };
                    shift
                } else { group };
                if number(group, 1)? == t.source_mode && number(group, 5)? == t.modeid { return Ok(group); }
            }
        }}
    }
    Err(Error::Protocol(tr_ref("controller.stick_source_missing_from_the_steam_reply").into()))
}
pub fn value(raw:&[u8],t:&BindingTarget,key:i64)->Result<i64>{
    for s in msg(group(raw,t)?,4)? {if number(s,1)?==key{return signed(s,2);}}
    Err(Error::Protocol(tr_args("controller.stick_setting_key_missing_from_the_steam_reply", &[("key", &key.to_string())])))
}
pub fn settings(raw:&[u8],t:&BindingTarget,section:usize)->Result<Vec<Setting>> {
    // Other modes have different contracts. Never invent joystick settings for them.
    if t.source_mode!=6 {return Ok(vec![]);}
    let defs:Vec<(i64,&str,&[&str],bool)>=match section {
        0=>vec![(32,tr_ref("controller.horizontal_scale"),&[],false),(31,tr_ref("controller.vertical_scale"),&[],false),(41,tr_ref("controller.invert_horizontal_axis"),&[],true),(42,tr_ref("controller.invert_vertical_axis"),&[],true),(9,tr_ref("controller.stick_response_curve"),&["controller.linear","controller.curve_1","controller.curve_2","controller.curve_3","controller.curve_4","controller.custom"],false),(10,tr_ref("controller.custom_curve_2"),&[],false)],
        1=>vec![(8,tr_ref("controller.output_joystick"),&["controller.left_joystick_2","controller.right_joystick_2","controller.relative_mouse"],false),(21,tr_ref("controller.output_axis"),&["controller.vertical","controller.horizontal","controller.both_axes"],false),(45,tr_ref("controller.rotate_output"),&[],false)],
        2=>vec![(124,tr_ref("controller.deadzone_source"),&["controller.no_deadzone","controller.calibration","controller.custom"],false),(12,tr_ref("controller.analog_range_end"),&[],false),(13,tr_ref("controller.analog_range_start"),&[],false),(14,tr_ref("controller.deadzone_shape"),&["controller.cross","controller.circle","controller.square"],false),(127,tr_ref("controller.advanced_settings_2"),&[],true),(18,tr_ref("controller.anti_deadzone"),&[],false),(19,tr_ref("controller.anti_deadzone_buffer"),&[],false)],
        3=>vec![(7,tr_ref("controller.command_radius"),&[],false),(20,tr_ref("controller.command_invert"),&[],true)],
        _=>vec![(3,tr_ref("controller.haptics_intensity"),&["controller.off","controller.low_2","controller.medium_2","controller.high_2"],false),(158,tr_ref("controller.gyro_activation_haptics"),&["controller.off","controller.on_enable","controller.on_disable","controller.on_enable_and_disable"],false)]
    };
    let g=group(raw,t)?;let mut out=Vec::new();
    for (key,label,choices,toggle) in defs {
        if key==10 && value(raw,t,9)?!=5 {continue;}
        let dz=value(raw,t,124)?;
        if section==2 && ((key==12 && dz==0) || ([13,14,127].contains(&key)&&dz!=2) || ([18,19].contains(&key)&&(dz!=2||value(raw,t,127)?==0))) {continue;}
        for s in msg(g,4)? {if number(s,1)?==key {
            let value=signed(s,2)?;
            let (min,max)=if toggle {(0,1)}else if !choices.is_empty(){(0,choices.len() as i64-1)}else{(signed(s,3)?,signed(s,4)?)};
            out.push(Setting{key,label,value,min,max,choice_keys:choices,toggle});
        }}
    }
    Ok(out)
}
#[derive(Clone,Debug)]
pub enum Undo { Binding(BindingTarget,Vec<u8>), Setting(BindingTarget,i64,i64), SourceSetting(BindingTarget,i64,i64,bool), SourceMode(usize,i64,i64),
    SwapSources { set: usize, left: i64, right: i64 },
    /// A binding that did not exist before: undo removes it (and its empty activator).
    Remove(BindingTarget),
    /// Previous activation type of an activator.
    Activation(BindingTarget,i64) }

#[cfg(test)] mod tests {
    use super::*;
    use steam_bridge::controller_edit::{Writer,set_source_setting_msg};
    fn fixture(layer:bool,dz:i64,curve:i64)->Vec<u8>{
        let mut group=Writer::new();group.int(1,6);group.int(5,8);
        for (key,value,min,max) in [(32,73,0,100),(31,100,0,100),(9,curve,0,5),(10,200,25,375),(124,dz,0,2),(12,0,0,32000),(13,28000,0,32000),(14,1,0,2),(127,0,0,0),(45,-20,-180,180),(7,25000,1,32767),(20,0,0,0)] {
            let mut s=Writer::new();s.int(1,key);for (f,v) in [(2,value),(3,min),(4,max)]{s.int(f,(v<<1)^(v>>63));}group.message(4,&s.finish());
        }
        let mut source=Writer::new();source.int(1,12);source.message(2,&group.finish());
        let mut set=Writer::new();set.string(1,"base");
        if layer {let mut l=Writer::new();l.string(1,"layer");l.message(6,&source.finish());set.message(4,&l.finish());}else{set.message(6,&source.finish());}
        let mut raw=Writer::new();raw.message(8,&set.finish());raw.finish()
    }
    #[test] fn controller_sticks_source_layer_ranges_and_signed_setting_match_stock_contract(){
        for layer in [false,true] {
            let raw=fixture(layer,0,0);let ss=sticks(&raw).unwrap();assert_eq!(ss.len(),1);let t=&ss[0].target;
            assert_eq!(t.source_binding_key,12);assert_eq!(t.modeid,8);assert_eq!(t.action_set_layer_key,layer.then(||"layer".into()));
            let p=settings(&raw,t,0).unwrap();assert_eq!(p[0].value,73);assert_eq!(p[0].next(1),74);assert_eq!(p[1].next(1),100);
            let rotation=settings(&raw,t,1).unwrap().pop().unwrap();assert_eq!(rotation.value,-20);assert_eq!(rotation.min,-180);
            let wire=set_source_setting_msg(t,45,-21);assert_eq!(number(msg(&wire,7).unwrap()[0],2).unwrap(),41);
            assert_eq!(number(&wire,3).unwrap(),12);assert_eq!(number(&wire,9).unwrap(),8);
            assert!(value(&raw,t,999).is_err());assert!(sticks(&raw[..raw.len()-1]).is_err());
        }
    }
    #[test] fn controller_sticks_dependent_rows_follow_native_values(){
        let raw=fixture(false,0,0);let t=sticks(&raw).unwrap().remove(0).target;
        assert_eq!(settings(&raw,&t,2).unwrap().len(),1);
        assert!(!settings(&raw,&t,0).unwrap().iter().any(|p|p.key==10));
        let custom=fixture(false,2,5);assert!(settings(&custom,&t,0).unwrap().iter().any(|p|p.key==10));
        assert_eq!(settings(&custom,&t,2).unwrap().iter().map(|p|p.key).collect::<Vec<_>>(),vec![124,12,13,14,127]);
        let toggle=settings(&custom,&t,3).unwrap().pop().unwrap();assert_eq!(toggle.next(-1),1);
    }
}
