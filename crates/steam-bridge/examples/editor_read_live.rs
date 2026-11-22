//! Live read of a game's editable controller layout (task 210). Read only: the call loads the
//! editor state the stock UI loads when a controller page opens; nothing is saved.
//! Usage: STEAM_CDP_PORT=18080 cargo run -p steam-bridge --example editor_read_live -- <app> <controller_index> <raw_out_path>
//! The raw bytes go to <raw_out_path>; keep that path outside Git (diagnostics).
use steam_bridge::controller_layout::ControllerLayout;
use steam_bridge::Bridge;

fn summarize(l: &ControllerLayout) {
    println!("layout display={:?} url={:?} type={} style={} error={:?}", l.display_name, l.url, l.controller_type, l.controller_style, l.error_msg);
    for s in &l.action_sets {
        println!("set key={:?} name={:?} layers={}", s.key, s.display_name, s.layer_count);
    }
    for m in &l.modes {
        println!("mode mode={} modeid={} name={:?} inputs={}", m.mode, m.modeid, m.friendly_name, m.inputs.len());
        for i in &m.inputs {
            let acts: Vec<String> = i
                .activators
                .iter()
                .map(|a| format!("act{}:[{}]", a.activation, a.bindings.iter().map(|b| format!("t{}:{:?}", b.kind, b.label)).collect::<Vec<_>>().join(",")))
                .collect();
            println!("   input key={} {}", i.key, acts.join(" "));
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let app: u32 = args.first().and_then(|s| s.parse().ok()).expect("app id");
    let idx: u32 = args.get(1).and_then(|s| s.parse().ok()).expect("controller index");
    let out = args.get(2).expect("raw out path");
    let b = Bridge::connect().expect("connect");
    let (raw, layout) = b.editor_start(app, idx).expect("StartEditing");
    std::fs::write(out, &raw).expect("write raw");
    println!("raw bytes {} -> {out}", raw.len());
    summarize(&layout);
}
