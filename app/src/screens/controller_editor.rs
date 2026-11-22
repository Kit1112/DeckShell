//! Provisional focus state for the controller editor fixture.
//!
//! The rail and row/gear layout are visible in Gaming Mode capture 12. Exact
//! directional behaviour still requires the stock focus walk; keep this model
//! isolated until that evidence and the Steam Input data contract are ready.
use steam_assets::i18n::{tr_args, tr_ref};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Column { Rail, Value, Gear }
/// Controls present on an editor row. Action-set headings have only a gear;
/// regular binding rows can have a value and a gear, or just a value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RowControls { Value, ValueAndGear, GearOnly }
impl RowControls {
    fn has_value(self) -> bool { self != Self::GearOnly }
    fn has_gear(self) -> bool { self != Self::Value }
    fn entry(self) -> Column { if self.has_value() { Column::Value } else { Column::Gear } }
    fn normalize(self, column: Column) -> Column {
        match column {
            Column::Value if !self.has_value() => Column::Gear,
            Column::Gear if !self.has_gear() => Column::Value,
            other => other,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction { Up, Down, Left, Right }
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorFocus {
    pub section: usize,
    pub row: usize,
    pub column: Column,
    pub picker_open: bool,
    // Restore a separate row when returning to a section.
    rows_by_section: Vec<usize>,
}
impl EditorFocus {
    pub fn new(sections: usize) -> Self {
        Self { section: 0, row: 0, column: Column::Rail, picker_open: false, rows_by_section: vec![0; sections] }
    }
    /// Captures 14a–14g: B from the command picker returned to the editor
    /// section rail. Returns false when no picker was open.
    pub fn back_from_picker(&mut self) -> bool {
        if !self.picker_open { return false; }
        self.picker_open = false;
        self.column = Column::Rail;
        true
    }
    pub fn move_in(&mut self, direction: Direction, row_gears: &[Vec<RowControls>]) {
        if row_gears.is_empty() { self.section = 0; self.row = 0; self.column = Column::Rail; return; }
        self.section = self.section.min(row_gears.len() - 1);
        self.rows_by_section.resize(row_gears.len(), 0);
        let count = row_gears[self.section].len();
        self.row = self.row.min(count.saturating_sub(1));
        if count == 0 { self.column = Column::Rail; }
        else { self.column = row_gears[self.section][self.row].normalize(self.column); }
        match (self.column, direction) {
            (Column::Rail, Direction::Up | Direction::Down) => {
                self.section = if direction == Direction::Up { self.section.saturating_sub(1) } else { (self.section + 1).min(row_gears.len() - 1) };
                self.row = self.rows_by_section[self.section].min(row_gears[self.section].len().saturating_sub(1));
            }
            (Column::Rail, Direction::Right) if count > 0 => self.column = row_gears[self.section][self.row].entry(),
            (Column::Value, Direction::Left) => self.column = Column::Rail,
            (Column::Value, Direction::Right) if row_gears[self.section][self.row].has_gear() => self.column = Column::Gear,
            (Column::Gear, Direction::Left) => self.column = if row_gears[self.section][self.row].has_value() { Column::Value } else { Column::Rail },
            (Column::Value | Column::Gear, Direction::Up | Direction::Down) => {
                self.row = if direction == Direction::Up { self.row.saturating_sub(1) } else { (self.row + 1).min(count - 1) };
                self.column = row_gears[self.section][self.row].normalize(self.column);
                self.rows_by_section[self.section] = self.row;
            }
            _ => {}
        }
    }
}

/// Focus in the stick settings: rail selects a section, content retains a row
/// per section. Row counts follow the settings present in the native editing copy.
#[derive(Clone, Debug, Default)]
pub struct StickFocus {
    pub section: usize,
    pub row: usize,
    pub content: bool,
    rows: [usize; 5],
}
impl StickFocus {
    #[cfg(test)]
    pub const COUNTS: [usize; 5] = [5, 3, 1, 2, 2];
    #[cfg(test)]
    pub fn step(&mut self, d: Direction) {
        self.step_with_counts(d,Self::COUNTS);
    }
    pub fn step_with_counts(&mut self,d:Direction,counts:[usize;5]) {
        match (self.content, d) {
            (false, Direction::Up) => self.section = self.section.saturating_sub(1),
            (false, Direction::Down) => self.section = (self.section + 1).min(4),
            (false, Direction::Right) => self.content = true,
            (true, Direction::Left) => self.content = false,
            (true, Direction::Up) => self.row = self.row.saturating_sub(1),
            (true, Direction::Down) => self.row = (self.row + 1).min(counts[self.section].saturating_sub(1)),
            _ => (),
        }
        if self.content { self.rows[self.section] = self.row; } else { self.row = self.rows[self.section].min(counts[self.section].saturating_sub(1)); }
    }
    pub fn back(&mut self) -> bool {
        if self.content { self.content = false; true } else { false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn moves_across_rail_values_and_available_gears() {
        let rows = vec![vec![RowControls::ValueAndGear, RowControls::Value], vec![], vec![RowControls::ValueAndGear]];
        let mut f = EditorFocus::new(3);
        f.move_in(Direction::Right, &rows);
        f.move_in(Direction::Right, &rows);
        assert_eq!(f.column, Column::Gear);
        f.move_in(Direction::Down, &rows);
        assert_eq!((f.row, f.column), (1, Column::Value));
        f.move_in(Direction::Left, &rows);
        f.move_in(Direction::Down, &rows);
        f.move_in(Direction::Right, &rows);
        assert_eq!(f.column, Column::Rail); // empty section cannot strand focus
        f.move_in(Direction::Down, &rows);
        f.move_in(Direction::Up, &rows);
        f.move_in(Direction::Up, &rows);
        assert_eq!(f.row, 1); // section's previous row restored
    }
    #[test]
    fn gear_only_action_set_is_reachable_and_returns_to_rail() {
        let rows = vec![vec![RowControls::GearOnly, RowControls::Value, RowControls::ValueAndGear]];
        let mut f = EditorFocus::new(1);
        f.move_in(Direction::Right, &rows);
        assert_eq!(f.column, Column::Gear);
        f.move_in(Direction::Left, &rows);
        assert_eq!(f.column, Column::Rail);
        f.move_in(Direction::Right, &rows);
        f.move_in(Direction::Down, &rows);
        assert_eq!((f.row, f.column), (1, Column::Value));
        f.move_in(Direction::Up, &rows);
        assert_eq!((f.row, f.column), (0, Column::Gear));
    }
    #[test]
    fn picker_back_restores_editor_rail() {
        let mut f = EditorFocus::new(1);
        f.column = Column::Value;
        f.picker_open = true;
        assert!(f.back_from_picker());
        assert_eq!(f.column, Column::Rail);
        assert!(!f.back_from_picker());
    }
    #[test]
    fn stick_settings_focus_returns_and_remembers_each_section() {
        let mut f = StickFocus::default();
        f.step(Direction::Right); f.step(Direction::Down); f.step(Direction::Down);
        assert!(f.back()); f.step(Direction::Down); f.step(Direction::Right);
        assert_eq!((f.section, f.row), (1, 0));
        assert!(f.back()); f.step(Direction::Up); f.step(Direction::Right);
        assert_eq!((f.section, f.row), (0, 2));
        assert!(f.back()); assert!(!f.back());
    }
    #[test]
    fn changing_or_empty_data_is_bounded() {
        let mut f = EditorFocus::new(8);
        f.section = 7; f.row = usize::MAX; f.column = Column::Gear;
        f.move_in(Direction::Down, &[vec![]]);
        assert_eq!((f.section, f.row, f.column), (0, 0, Column::Rail));
        f.move_in(Direction::Right, &[]);
        assert_eq!((f.section, f.row, f.column), (0, 0, Column::Rail));
    }
}


/// Picker geometry is in physical pixels relative to the content below the header.
#[derive(Clone, Debug)]
pub struct Key { pub label: String, pub x: f32, pub y: f32, pub w: f32, pub h: f32, pub center: bool, pub bottom: bool }
pub fn key(label: &str, x: f32, y: f32, w: f32, h: f32) -> Key { Key { label: label.into(), x, y, w, h, center: true, bottom: false } }
/// Keyboard tab from Gaming Mode capture 14d (`editor-buttons/14d-picker-keyboard.dom.json`).
/// Each row is (label, css x, css width); the page is 854 CSS px wide at DPR 1.5.
/// Rows start at CSS y 139 with a 53 px pitch, keys are 50 px tall, and the picker
/// sits 60 px below the header, so the picker-relative y of row 0 is 148.5.
/// Label alignment follows the stock keys: 1-unit keys are centred, wide keys and
/// the top row are anchored to the bottom, and the narrow «~» key is left-aligned.
pub fn keyboard_keys() -> Vec<Key> {
    const DPR: f32 = 1.5;
    const ROWS: [&[(&str, f32, f32)]; 6] = [
        &[("Esc",33.,53.),("F1",89.,58.),("F2",150.,58.),("F3",211.,58.),("F4",272.,58.),("F5",333.,58.),("F6",394.,58.),("F7",455.,58.),("F8",516.,58.),("F9",577.,58.),("F10",638.,58.),("F11",699.,58.),("F12",760.,58.)],
        &[("~\n`",33.,29.),("!\n1",65.,52.),("@\n2",120.,52.),("#\n3",175.,52.),("$\n4",230.,52.),("%\n5",285.,52.),("^\n6",340.,52.),("&\n7",394.,52.),("*\n8",449.,52.),("(\n9",504.,52.),(")\n0",559.,52.),("_\n-",614.,52.),("+\n=",669.,52.),("Backspace",724.,94.)],
        &[("Tab",33.,59.),("Q",95.,53.),("W",151.,53.),("E",207.,53.),("R",263.,53.),("T",319.,53.),("Y",374.,53.),("U",430.,53.),("I",486.,53.),("O",542.,53.),("P",598.,53.),("{\n[",654.,53.),("}\n]",709.,53.),("|\n\\",765.,53.)],
        &[("Caps Lock",33.,77.),("A",113.,52.),("S",169.,52.),("D",224.,52.),("F",279.,52.),("G",335.,52.),("H",390.,52.),("J",446.,52.),("K",501.,52.),("L",556.,52.),(":\n;",612.,52.),("\"\n'",667.,52.),("Enter",723.,95.)],
        &[("Shift",33.,114.),("Z",150.,52.),("X",205.,52.),("C",261.,52.),("V",316.,52.),("B",372.,52.),("N",427.,52.),("M",482.,52.),("<\n,",538.,52.),(">\n.",593.,52.),("?\n/",649.,52.),("Shift",704.,114.)],
        &[("Ctrl",33.,72.),("Win",108.,72.),("Alt",183.,72.),("Space",258.,410.),("Alt",671.,72.),("Ctrl",746.,72.)],
    ];
    let mut out = Vec::with_capacity(72);
    for (r, row) in ROWS.iter().enumerate() {
        for &(label, x, w) in row.iter() {
            let space = label == "Space";
            let label = if space { tr_ref("controller.space") } else { label };
            out.push(Key {
                label: label.into(),
                x: x * DPR,
                y: 148.5 + r as f32 * 79.5,
                w: w * DPR,
                h: 50. * DPR,
                center: space || (r > 0 && (51.0..56.0).contains(&w)),
                bottom: r == 0 || w >= 56.0,
            });
        }
    }
    out
}
/// Labels and geometry reconstructed from captures 13a and 14c–14f. These
/// define presentation only; picker_command supplies separately verified stock identifiers.
pub fn picker_keys(tab: usize) -> Vec<Key> {
    match tab {
        0 => [
            (tr_ref("controller.left_trigger"),51.,132.,210.,42.),(tr_ref("controller.left_bumper"),51.,178.5,210.,42.),
            (tr_ref("controller.select"),475.5,154.5,123.,42.),(tr_ref("controller.start"),682.5,154.5,123.,42.),
            (tr_ref("controller.right_trigger"),1020.,132.,210.,42.),(tr_ref("controller.right_bumper"),1020.,178.5,210.,42.),
            ("↑",154.5,237.,49.5,49.5),("↓",154.5,352.5,49.5,49.5),("←",96.,294.,49.5,49.5),("→",211.5,294.,49.5,49.5),("L",142.5,282.,75.,75.),
            ("↑",162.,438.,63.,63.),("↓",162.,543.,63.,63.),("←",109.5,490.5,63.,63.),("→",214.5,490.5,63.,63.),
            ("Y",1093.5,258.,63.,63.),("A",1093.5,363.,63.,63.),("X",1041.,310.5,63.,63.),("B",1146.,310.5,63.,63.),
            ("↑",1039.5,462.,49.5,49.5),("↓",1039.5,577.5,49.5,49.5),("←",982.5,520.5,49.5,49.5),("→",1098.,520.5,49.5,49.5),("R",1027.5,507.,75.,75.)
        ].into_iter().map(|(l,x,y,w,h)| key(l,x,y,w,h)).collect(),
        1 => [(tr_ref("controller.left_mouse_click"),211.,185.),(tr_ref("controller.middle_mouse_click"),211.,260.),(tr_ref("controller.right_mouse_click"),211.,335.),(tr_ref("controller.mouse_4_click"),211.,434.),(tr_ref("controller.mouse_5_click"),211.,509.),(tr_ref("controller.scroll_wheel_up"),813.,185.),(tr_ref("controller.scroll_wheel_down"),813.,260.),(tr_ref("controller.move_to_position"),813.,434.),(tr_ref("controller.move_by_amount"),813.,509.)].into_iter().map(|(l,x,y)|key(l,x,y,252.,57.)).collect(),
        2 => keyboard_keys(),
        3 => {
            let labels=["Insert","Home","Page Up","Delete","End","Page Down","↑","←","↓","→","▶","■","⏭","Num Lock","/","*","7","8","9","4","5","6","1","2","3","0",".","⏮","-","+","Enter"];
            labels.into_iter().enumerate().map(|(i,l)| {
                let (x,y,w,h)=if i<6 {(313.+(i%3) as f32*90.,149.+(i/3) as f32*79.,85.,75.)}else if i<10 {let (c,r)=[(1,4),(0,5),(1,5),(2,5)][i-6];(313.+c as f32*90.,149.+r as f32*79.,85.,75.)}else if i<27 {let j=i-10;let (c,r)=if j<15 {(j%3,j/3)}else{(if j==15 {0}else{2},5)};(658.+c as f32*90.,149.+r as f32*79.,if i==25 {175.}else{85.},75.)}else{(928.,149.+[0.,79.,158.,316.][i-27],85.,if i>28 {154.}else{75.})};key(l,x,y,w,h)
            }).collect()
        },
        4 => [(tr_ref("controller.change_action_set"),511.5,204.),(tr_ref("controller.add_action_set_layer"),255.,289.5),(tr_ref("controller.remove_action_set_layer"),511.5,289.5),(tr_ref("controller.hold_action_set_layer"),768.,289.5),(tr_ref("controller.cleared_from_parent"),511.5,375.)].into_iter().map(|(l,x,y)|key(l,x,y,252.,57.)).collect(),
        5 => [tr_ref("controller.take_screenshot"), tr_ref("controller.game_recording"), tr_ref("controller.recording_marker"), tr_ref("controller.save_clip"), tr_ref("controller.show_keyboard"), tr_ref("controller.magnifier"), tr_ref("controller.exit_game"), tr_ref("controller.player_number"), tr_ref("controller.touch_normal"), tr_ref("controller.touch_left_click"), tr_ref("controller.touch_right_click"), tr_ref("controller.touch_middle_click"), tr_ref("controller.touch_hover"), tr_ref("controller.touch_disabled"), tr_ref("controller.volume"), tr_ref("controller.volume_2")].into_iter().enumerate().map(|(i,l)|key(l,70.+(i%4) as f32*300.,150.+(i/4) as f32*100.,280.,85.)).collect(),
        _ => [tr_ref("controller.reset_camera_horizon"), tr_ref("controller.calibration_turn"), tr_ref("controller.turn_to_direction")].into_iter().enumerate().map(|(i,l)|key(l,110.+i as f32*360.,200.,330.,120.)).collect(),
    }
}
/// Spatial navigation uses nearest eligible key; preserves each tab's cursor.
pub fn picker_move(keys: &[Key], current: usize, d: Direction) -> usize {
    let Some(from)=keys.get(current) else { return 0 };
    let (x,y)=(from.x+from.w/2.,from.y+from.h/2.);
    keys.iter().enumerate().filter_map(|(i,k)| {
        let (dx,dy)=(k.x+k.w/2.-x,k.y+k.h/2.-y);
        let (primary,secondary)=match d { Direction::Up if dy < -1. => (-dy,dx.abs()), Direction::Down if dy > 1. => (dy,dx.abs()), Direction::Left if dx < -1. => (-dx,dy.abs()), Direction::Right if dx > 1. => (dx,dy.abs()), _ => return None };
        Some((i, primary+secondary*2.))
    }).min_by(|a,b|a.1.total_cmp(&b.1)).map(|x|x.0).unwrap_or(current)
}
#[cfg(test)] mod picker_tests {
    use super::*;
    #[test] fn all_tabs_have_bounded_directional_focus() {
        for tab in 0..7 { let keys=picker_keys(tab); assert!(!keys.is_empty()); for i in 0..keys.len() { for dir in [Direction::Up,Direction::Down,Direction::Left,Direction::Right] { assert!(picker_move(&keys,i,dir)<keys.len()); } } }
    }
}

/// IDs from stock tables; display labels are never used as write identifiers.
pub fn picker_command(tab: usize, index: usize) -> Option<(i64,u32,i64)> {
    use super::controller_commands as c;
    let (kind,field,value)=match tab {
        0=>(3,5,*c::GAMEPAD.get(index)?),
        1=>{ let values=[0,2,1,4,3,5,6]; if index<7 {(if index<5 {2}else{4},if index<5 {4}else{6},values[index])}else{(7,9,*[3,36].get(index-7)?)} },
        2=>(1,3,*c::KEYBOARD.get(index)?),
        3=>(1,3,*c::NUMPAD.get(index)?),
        4=>{let actions=["#ControllerActionKey_Change_Preset","#ControllerActionKey_Add_ActionSet_Layer","#ControllerActionKey_Remove_ActionSet_Layer","#ControllerActionKey_Hold_ActionSet_Layer","#ControllerActionKey_Layer_Empty_Binding"];let tok=actions.get(index)?;let row=c::COMMANDS.iter().find(|r|r.2==*tok)?;(7,9,row.1)},
        5=>{let toks=["Screenshot","GR_Toggle","GR_Marker","GR_Clip","Show_Keyboard","Toggle_Magnifier","Quit_Application","Change_Player_Number","Change_TouchscreenMode_Native","Change_TouchscreenMode_LeftClick","Change_TouchscreenMode_RightClick","Change_TouchscreenMode_MiddleClick","Change_TouchscreenMode_Hover","Change_TouchscreenMode_None"];if index>=14 {(1,3,*[108,109].get(index-14)?)}else{let token=format!("#ControllerActionKey_{}",toks[index]);let row=c::COMMANDS.iter().find(|r|r.2==token)?;(7,9,row.1)}},
        6=>(7,9,*[37,38,39].get(index)?),
        _=>return None,
    };Some((kind,field,value))
}
pub fn command_label(binding:&[u8])->String {
    if let Ok(meta)=super::controller_live::msg(binding,2) {if let Some(meta)=meta.first() {if let Ok(name)=super::controller_live::string(meta,2) {if !name.is_empty(){return if name.starts_with('#'){crate::assets::t(&name)}else{name};}}}}
    let Some((kind,value))=super::controller_live::identity(binding) else {return tr_ref("controller.steam_command").into()};
    super::controller_commands::COMMANDS.iter().find(|r|r.3==kind&&r.1==value).map(|r|crate::assets::t(r.2)).unwrap_or_else(||tr_args("controller.command_kind_value", &[("kind", &kind.to_string()), ("value", &value.to_string())]))
}
pub fn command_glyph(binding:&[u8], focused:bool)->slint::Image {
    let Some((kind,value))=super::controller_live::identity(binding) else{return Default::default()};
    let name=match (kind,value) {(3,3)=>"shared_button_a",(3,4)=>"shared_button_b",(3,5)=>"shared_button_y",(3,6)=>"shared_button_x",(3,1)=>"sd_l2",(3,2)=>"sd_r2",(3,7)=>"sd_l1",(3,8)=>"sd_r1",(3,11)=>"sd_button_menu",(3,12)=>"sd_button_view",(3,9)=>"shared_lstick_click",(3,10)=>"shared_rstick_click",(3,18)=>"shared_lstick_up",(3,19)=>"shared_lstick_down",(3,20)=>"shared_lstick_left",(3,21)=>"shared_lstick_right",(3,22)=>"shared_rstick_up",(3,23)=>"shared_rstick_down",(3,24)=>"shared_rstick_left",(3,25)=>"shared_rstick_right",(3,14)=>"shared_dpad_up",(3,15)=>"shared_dpad_down",(3,16)=>"shared_dpad_left",(3,17)=>"shared_dpad_right",(2,0)=>"shared_mouse_l_click",(2,1)=>"shared_mouse_r_click",(2,2)=>"shared_mouse_mid_click",(2,3)=>"shared_mouse_5",(2,4)=>"shared_mouse_4",(4,5)=>"shared_mouse_scroll_up",(4,6)=>"shared_mouse_scroll_down",_=>return Default::default()};
    crate::assets::controller_glyph(name, if focused { "light.md" } else { "dark.md" }).unwrap_or_default()
}
#[cfg(test)] mod command_tests {
    use super::*;
    #[test] fn every_stock_picker_key_has_a_typed_command(){for tab in 0..7 {let keys=picker_keys(tab);for (i,_) in keys.iter().enumerate(){assert!(picker_command(tab,i).is_some(),"tab {tab}, key {i}");}assert!(picker_command(tab,keys.len()).is_none());}assert_eq!(picker_keys(0).len(),24);assert_eq!(picker_keys(1).len(),9);assert_eq!(picker_keys(2).len(),72);assert_eq!(picker_keys(3).len(),31);assert_eq!(picker_keys(4).len(),5);assert_eq!(picker_keys(5).len(),16);assert_eq!(picker_keys(6).len(),3);}
    #[test] fn identical_letters_in_distinct_tabs_write_distinct_commands(){assert_eq!(picker_command(0,16),Some((3,5,3)));assert_eq!(picker_command(2,42),Some((1,3,4)));assert_eq!(picker_command(2,54),Some((1,3,101)));assert_eq!(picker_command(2,65),Some((1,3,105)));}
}

#[cfg(test)]
mod keyboard_geometry_tests {
    use super::*;

    /// Anchors from Gaming Mode capture 14d (CSS px × 1.5 = device px).
    #[test]
    fn keyboard_tab_matches_stock_capture_14d() {
        let keys = keyboard_keys();
        assert_eq!(keys.len(), 72);
        let find = |label: &str| keys.iter().find(|k| k.label == label).unwrap_or_else(|| panic!("missing key {label:?}"));
        let esc = find("Esc");
        assert!((esc.x - 49.5).abs() < 0.01 && (esc.w - 79.5).abs() < 0.01, "Esc {:?}", (esc.x, esc.w));
        let f1 = find("F1");
        assert!((f1.x - 133.5).abs() < 0.01 && (f1.w - 87.0).abs() < 0.01, "F1 {:?}", (f1.x, f1.w));
        let backspace = find("Backspace");
        assert!((backspace.x + backspace.w - 1227.0).abs() < 0.01);
        let space = find(tr_ref("controller.space"));
        assert!((space.w - 615.0).abs() < 0.01 && space.center && space.bottom);
        // Row 0 sits at picker y 148.5 (picker origin 60 px) = 208.5 device px, the capture's first key row.
        assert!((keys[0].y + 60.0 - 208.5).abs() < 0.01);
        assert!(keys.iter().all(|k| (k.h - 75.0).abs() < 0.01));
        for pair in keys.windows(2).filter(|p| p[0].y == p[1].y) {
            assert!(pair[0].x + pair[0].w <= pair[1].x + 0.01, "keys overlap at y {}", pair[0].y);
        }
        // Stock alignment: the top row and wide keys are bottom-anchored, letters centred, «~» left.
        assert!(esc.bottom && !esc.center);
        assert!(find("Tab").bottom && find("Enter").bottom);
        assert!(find("A").center && !find("A").bottom);
        let tilde = find("~\n`");
        assert!(!tilde.center && !tilde.bottom);
    }
}
