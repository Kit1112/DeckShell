//! Live check of the controller layout calls on a Steam Deck (STEAM_CDP_PORT points at it).
//! Read-only by default. `--write` selects one community layout on Balatro, reads it back, clears
//! it and reads back again. Writes are refused for any app other than Balatro (2379780).
//! Usage: STEAM_CDP_PORT=18080 cargo run -p steam-bridge --example controller_layouts_live -- <controller_index> [--write]
use std::time::Instant;

use steam_bridge::controller_list::*;
use steam_bridge::Bridge;

const BALATRO: u32 = 2379780;
const TEST_LAYOUT: &str = "workshop://3177015367";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let idx: u32 = args.first().and_then(|s| s.parse().ok()).expect("controller index");
    let write = args.iter().any(|a| a == "--write");
    let b = Bridge::connect().expect("connect");

    let t = Instant::now();
    let before = b.layout_active(BALATRO, idx).expect("active before");
    println!("active.before {:?} ms={}", before, t.elapsed().as_millis());

    let t = Instant::now();
    let compat = b.layout_list(BALATRO, idx, false).expect("list compatible");
    println!("list compatible-only n={}", compat.len());
    let list = b.layout_list(BALATRO, idx, true).expect("list all");
    let sec = |s| list.iter().filter(|e| e.source == s).count();
    println!(
        "list n={} official={} template={} community={} ms={}",
        list.len(),
        sec(LayoutSource::Official),
        sec(LayoutSource::Template),
        sec(LayoutSource::Community),
        t.elapsed().as_millis()
    );
    let hits = select_layouts(&list, &LayoutQuery { text: "sort".into(), ..Default::default() });
    println!("search 'sort' -> {:?}", hits.iter().map(|e| &e.url).collect::<Vec<_>>());
    let active_marked: Vec<&str> = list.iter().filter(|e| is_active(e, before.as_ref())).map(|e| e.url.as_str()).collect();
    println!("active marked in list -> {active_marked:?}");

    let ids: Vec<u64> = list.iter().filter_map(|e| e.published_file_id).filter(|&id| id > 1000).take(3).collect();
    let t = Instant::now();
    match b.layout_details(&ids) {
        Ok(d) => println!("details n={} ms={} first={:?}", d.len(), t.elapsed().as_millis(), d.first().map(|x| (x.published_file_id, &x.title, x.votes))),
        Err(e) => println!("details ERROR {e}"),
    }
    match b.layout_user_votes(&ids) {
        Ok(v) => println!("votes {:?}", v.iter().map(|x| (x.published_file_id, x.vote)).collect::<Vec<_>>()),
        Err(e) => println!("votes ERROR {e}"),
    }

    if !write {
        return;
    }
    let before = before.expect("refusing to write: no active layout to restore");
    let sel = before.selection_type;
    let t = Instant::now();
    b.layout_select(BALATRO, idx, TEST_LAYOUT, sel).expect("select");
    let after_select = b.layout_active(BALATRO, idx).expect("active after select");
    println!("select {TEST_LAYOUT} ms={} -> active {:?}", t.elapsed().as_millis(), after_select.as_ref().map(|a| &a.url));
    let list2 = b.layout_list(BALATRO, idx, true).expect("list after select");
    let flagged: Vec<&str> = list2.iter().filter(|e| is_active(e, after_select.as_ref())).map(|e| e.url.as_str()).collect();
    println!("list after select: active marked -> {flagged:?}");

    let t = Instant::now();
    let sel_now = after_select.as_ref().map(|a| a.selection_type).unwrap_or(sel);
    b.layout_clear(BALATRO, idx, sel_now).expect("clear");
    let after_clear = b.layout_active(BALATRO, idx).expect("active after clear");
    println!("clear ms={} -> active {:?}", t.elapsed().as_millis(), after_clear.as_ref().map(|a| (&a.url, &a.title)));
    let restored = after_clear.as_ref().map(|a| a.url == before.url && a.selection_type == before.selection_type).unwrap_or(false);
    println!("restored to the starting layout: {restored}");
}
