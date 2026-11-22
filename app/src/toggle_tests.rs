//! The one switch (app/ui/widgets/toggle.slint): PNG render of ON/OFF × focused/unfocused checked against the colours and sizes measured on the
//! owner's stock captures (toggle_{ON,OFF}_unfocused_and_focused.jpg), plus a source lint that keeps hand-made switches out of the UI.
//! PNGs go to $DECKSHELL_GALLERY_DIR/toggle-states.png when set.
use crate::*;
use slint::ComponentHandle;

type Px = (u8, u8, u8);

fn render() -> slint::SharedPixelBuffer<slint::Rgba8Pixel> {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions { renderer_name: Some("software".into()), mock_time: true, ..Default::default() },
    ))).unwrap();
    let ui = ToggleGallery::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(460, 120));
    let _ = ui.window().take_snapshot().unwrap();
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(2));
    slint::platform::update_timers_and_animations();
    let img = ui.window().take_snapshot().unwrap();
    if let Some(dir) = std::env::var_os("DECKSHELL_GALLERY_DIR").map(std::path::PathBuf::from) {
        std::fs::create_dir_all(&dir).unwrap();
        image::save_buffer(dir.join("toggle-states.png"), img.as_bytes(), img.width(), img.height(), image::ColorType::Rgba8).unwrap();
    }
    img
}

fn px(img: &slint::SharedPixelBuffer<slint::Rgba8Pixel>, x: i32, y: i32) -> Px {
    let i = (y as usize * img.width() as usize + x as usize) * 4;
    let b = img.as_bytes();
    (b[i], b[i + 1], b[i + 2])
}

fn near(a: Px, b: Px) -> bool { (a.0 as i32 - b.0 as i32).abs() <= 3 && (a.1 as i32 - b.1 as i32).abs() <= 3 && (a.2 as i32 - b.2 as i32).abs() <= 3 }

const CARD: Px = (0x23, 0x26, 0x2d);
const BLUE: Px = (0x1a, 0x9f, 0xff);
const WHITE: Px = (255, 255, 255);
const RING: Px = (0x8b, 0x92, 0x9a);
const RIM: Px = (0x67, 0x70, 0x7b);

/// Horizontal extent of the pill (pixels that are not the card) along the centre line, ignoring the focus ring/rim.
fn extent(img: &slint::SharedPixelBuffer<slint::Rgba8Pixel>, x0: i32, y: i32, skip: Px) -> (i32, i32) {
    let xs: Vec<i32> = (x0 - 20..x0 + 80).filter(|&x| { let p = px(img, x, y); !near(p, CARD) && !near(p, skip) }).collect();
    (*xs.first().unwrap(), *xs.last().unwrap())
}

#[test]
fn switch_states_match_the_stock_captures() {
    let img = render();
    let (y, yc) = (40, 40 + 16);
    // size: 58 x 33 (stock: pill 1164..1221 x 329..362)
    let (l, r) = extent(&img, 40, yc, RING);
    assert_eq!((r - l + 1, ), (58,), "unfocused ON pill width");
    let col: Vec<i32> = (0..120).filter(|&yy| !near(px(&img, 40 + 29, yy), CARD)).collect();
    assert_eq!(col.len(), 33, "pill height");
    // ON unfocused: blue rail, white knob on the right, no ring
    assert!(near(px(&img, 40 + 8, yc), BLUE));
    assert!(near(px(&img, 40 + 58 - 16, yc), WHITE));
    assert!(near(px(&img, 40 - 3, yc), CARD), "no ring when unfocused");
    // ON focused: 6px ring around the pill, 3px knob rim, same blue
    assert!(near(px(&img, 140 - 5, yc), RING) && near(px(&img, 140 - 1, yc), RING));
    assert!(near(px(&img, 140 + 29, y - 4), RING), "ring above the pill");
    assert!(near(px(&img, 140 + 8, yc), BLUE));
    assert!(near(px(&img, 140 + 58 - 16, yc), WHITE));
    assert!(near(px(&img, 140 + 58 + 1, yc), RIM) || near(px(&img, 140 + 58 + 2, yc), RIM), "knob rim at the pill end");
    // OFF unfocused: #43464d rail, white knob on the left
    assert!(near(px(&img, 240 + 45, yc), (0x43, 0x46, 0x4d)));
    assert!(near(px(&img, 240 + 16, yc), WHITE));
    assert!(near(px(&img, 240 - 3, yc), CARD));
    // OFF focused: darker #1d212a rail, ring, rim left of the knob
    assert!(near(px(&img, 340 + 45, yc), (0x1d, 0x21, 0x2a)));
    assert!(near(px(&img, 340 + 16, yc), WHITE));
    assert!(near(px(&img, 340 - 5, yc), RING));
    assert!(near(px(&img, 340 - 1, yc), RIM));
    assert!(near(px(&img, 340 + 29, y + 33 + 4), RING), "ring below the pill");
}

fn slint_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() { slint_files(&p, out); } else if p.extension().is_some_and(|x| x == "slint") { out.push(p); }
    }
}

/// Layout lint: the switch has one look and one size. Callers only place it; nobody re-draws it.
#[test]
fn no_hand_made_switches() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
    let mut files = vec![];
    slint_files(&root, &mut files);
    for f in files {
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        if name == "toggle.slint" { continue; }
        let src = std::fs::read_to_string(&f).unwrap();
        for (n, line) in src.lines().enumerate() {
            let at = format!("{name}:{}", n + 1);
            assert!(!line.contains("outer-ring") && !line.contains("qam-toggle-off"), "{at}: retired switch variant");
            // the knob slide easing exists only in the shared component
            assert!(!line.contains("cubic-bezier(0.1, 0.12, 0.53, 1.72)"), "{at}: hand-made switch knob");
            if let Some(i) = line.find("Toggle {") {
                let call = &line[i..];
                if !call.contains("GQam") && !line.contains("component") {
                    assert!(!call.contains(" width:") && !call.contains(" height:"), "{at}: Toggle has a fixed size, do not set width/height");
                }
            }
        }
    }
}
