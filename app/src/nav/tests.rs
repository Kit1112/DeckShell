//! Unit tests derived from the original engine logic (sp.js line refs in each test).
use super::history::RestoreFocus;
use super::scroll::{Behavior, Edges, ScrollBox, ScrollTarget};
use super::tree::*;
use super::*;

fn leaf() -> Props {
    Props { focusable: true, ..Default::default() }
}
fn cont(layout: Layout) -> Props {
    Props { layout, ..Default::default() }
}
fn press(t: &mut Tree, b: Button) -> bool {
    t.handle_button(b, false)
}
fn focused_key(t: &Tree) -> String {
    t.focused().and_then(|f| t.nav_key(f).map(|s| s.to_string())).unwrap_or_default()
}
fn keyed(k: &str) -> Props {
    Props { focusable: true, nav_key: Some(k.into()), ..Default::default() }
}
fn focus(t: &mut Tree, n: NodeId) {
    t.transfer_focus(FocusSource::Application, Some(n), None);
    t.drain_events();
}
fn scrolls(ev: &[NavEvent]) -> Vec<ScrollReq> {
    ev.iter().filter_map(|e| if let NavEvent::Scroll(s) = e { Some(*s) } else { None }).collect()
}

/// grid: row0 = 4 cells (x 0,100,200,300), row1 = 2 wide cells (0-200, 200-400), row2 = 4 cells
fn grid() -> (Tree, NodeId, Vec<NodeId>) {
    let mut t = Tree::new((1280.0, 800.0));
    let r = t.root();
    let g = t.add_node(r, cont(Layout::Grid), Some(Rect::new(0.0, 0.0, 400.0, 300.0)));
    let mut cells = vec![];
    for i in 0..4 {
        cells.push(t.add_node(g, keyed(&format!("a{i}")), Some(Rect::xywh(i as f32 * 100.0, 0.0, 100.0, 100.0))));
    }
    for i in 0..2 {
        cells.push(t.add_node(g, keyed(&format!("b{i}")), Some(Rect::xywh(i as f32 * 200.0, 100.0, 200.0, 100.0))));
    }
    for i in 0..4 {
        cells.push(t.add_node(g, keyed(&format!("c{i}")), Some(Rect::xywh(i as f32 * 100.0, 200.0, 100.0, 100.0))));
    }
    t.flush_deferred_focus();
    (t, g, cells)
}

#[test]
fn column_has_no_wrap_and_fails_at_end() {
    // sp.js:81933 FindNextFocusableChildInDirection: linear scan, no wrap; unhandled DIR => FailedNav
    let mut t = Tree::new((1280.0, 800.0));
    let r = t.root();
    let c = t.add_node(r, cont(Layout::Column), Some(Rect::new(0.0, 0.0, 100.0, 300.0)));
    let a = t.add_node(c, leaf(), Some(Rect::xywh(0.0, 0.0, 100.0, 100.0)));
    let b = t.add_node(c, leaf(), Some(Rect::xywh(0.0, 100.0, 100.0, 100.0)));
    focus(&mut t, a);
    assert!(press(&mut t, Button::DirDown));
    assert_eq!(t.focused(), Some(b));
    assert!(t.drain_events().contains(&NavEvent::BasicNav));
    assert!(!press(&mut t, Button::DirDown));
    assert_eq!(t.focused(), Some(b));
    assert!(t.drain_events().contains(&NavEvent::FailedNav));
    // a column answers only UP/DOWN (sp.js:81860): LEFT bubbles to the root (also a column) => unhandled
    assert!(!press(&mut t, Button::DirLeft));
    assert!(press(&mut t, Button::DirUp));
    assert_eq!(t.focused(), Some(a));
}

#[test]
fn row_reverse_inverts_direction() {
    // ComputeRelativeDirection (sp.js:81860): ROW_REVERSE: LEFT => FORWARD (higher index)
    let mut t = Tree::new((1280.0, 800.0));
    let r = t.root();
    let c = t.add_node(r, cont(Layout::RowReverse), Some(Rect::new(0.0, 0.0, 300.0, 100.0)));
    let a = t.add_node(c, leaf(), Some(Rect::xywh(200.0, 0.0, 100.0, 100.0)));
    let b = t.add_node(c, leaf(), Some(Rect::xywh(100.0, 0.0, 100.0, 100.0)));
    focus(&mut t, a);
    assert!(press(&mut t, Button::DirLeft));
    assert_eq!(t.focused(), Some(b));
    assert!(!press(&mut t, Button::DirUp)); // row answers only L/R
}

#[test]
fn grid_left_right_stay_in_row() {
    // sp.js:81995: LEFT/RIGHT stop as soon as a candidate no longer overlaps vertically
    let (mut t, _g, cells) = grid();
    focus(&mut t, cells[3]); // a3 (last of row 0)
    assert!(!press(&mut t, Button::DirRight));
    assert_eq!(t.focused(), Some(cells[3]));
    focus(&mut t, cells[4]); // b0, first of row 1
    assert!(!press(&mut t, Button::DirLeft));
    assert_eq!(t.focused(), Some(cells[4]));
    assert!(press(&mut t, Button::DirRight));
    assert_eq!(t.focused(), Some(cells[5]));
}

#[test]
fn grid_row_change_preserves_column() {
    // sp.js:82053 tangent memory: go right to column 3, down through a 2-cell row and back to 4-cell row
    let (mut t, _g, cells) = grid();
    focus(&mut t, cells[0]);
    for _ in 0..3 {
        assert!(press(&mut t, Button::DirRight));
    }
    assert_eq!(focused_key(&t), "a3");
    assert!(press(&mut t, Button::DirDown));
    assert_eq!(focused_key(&t), "b1"); // wide cell 200..400 overlaps column 300..400 most
    assert!(press(&mut t, Button::DirDown));
    assert_eq!(focused_key(&t), "c3"); // remembered column wins, not b1's left edge (c2)
    assert!(press(&mut t, Button::DirUp));
    assert_eq!(focused_key(&t), "b1");
    assert!(press(&mut t, Button::DirUp));
    assert_eq!(focused_key(&t), "a3");
    assert!(!press(&mut t, Button::DirUp)); // top row: bubble => FailedNav
}

#[test]
fn grid_tie_breaks_by_left_edge() {
    // sp.js:82131: equal x-overlap => nearest left edge. Move from a1 (100..200) down to b0 (0..200): overlap 100 vs b1 0.
    let (mut t, _g, cells) = grid();
    focus(&mut t, cells[1]);
    assert!(press(&mut t, Button::DirDown));
    assert_eq!(focused_key(&t), "b0");
    // a2 (200..300) down => b1
    focus(&mut t, cells[2]);
    assert!(press(&mut t, Button::DirDown));
    assert_eq!(focused_key(&t), "b1");
}

#[test]
fn grid_skips_rows_without_focusable_cells() {
    let (mut t, _g, cells) = grid();
    for &c in &cells[4..6] {
        t.set_props(c, Props { focusable: false, ..keyed("dead") });
    }
    focus(&mut t, cells[1]);
    assert!(press(&mut t, Button::DirDown));
    assert_eq!(focused_key(&t), "c1"); // continues to the following row (sp.js:82014 loop)
}

#[test]
fn focus_memory_on_reentry_and_reset() {
    // sp.js:81768 FindFocusableDescendant: remembered active child; resetNavOnEntry forgets (only with a direction)
    let mut t = Tree::new((1280.0, 800.0));
    let r = t.root();
    let row = t.add_node(r, cont(Layout::Row), Some(Rect::new(0.0, 0.0, 600.0, 400.0)));
    let left = t.add_node(row, cont(Layout::Column), Some(Rect::new(0.0, 0.0, 300.0, 400.0)));
    let l: Vec<_> = (0..3).map(|i| t.add_node(left, leaf(), Some(Rect::xywh(0.0, i as f32 * 100.0, 300.0, 100.0)))).collect();
    let right = t.add_node(row, cont(Layout::Column), Some(Rect::new(300.0, 0.0, 600.0, 400.0)));
    let rr = t.add_node(right, leaf(), Some(Rect::xywh(300.0, 0.0, 300.0, 100.0)));
    focus(&mut t, l[0]);
    assert!(press(&mut t, Button::DirDown));
    assert!(press(&mut t, Button::DirDown));
    assert_eq!(t.focused(), Some(l[2]));
    assert!(press(&mut t, Button::DirRight));
    assert_eq!(t.focused(), Some(rr));
    assert!(press(&mut t, Button::DirLeft));
    assert_eq!(t.focused(), Some(l[2])); // remembered
    // resetNavOnEntry: entering with a direction starts from the first child again
    let mut p = t.props(left).clone();
    p.reset_nav_on_entry = true;
    t.set_props(left, p);
    assert!(press(&mut t, Button::DirRight));
    assert!(press(&mut t, Button::DirLeft));
    assert_eq!(t.focused(), Some(l[0]));
}

#[test]
fn entry_prefer_last_and_preferred_child() {
    let mut t = Tree::new((1280.0, 800.0));
    let r = t.root();
    let c = t.add_node(r, Props { nav_entry_prefer_position: Some(EntryPrefer::Last), ..cont(Layout::Column) }, Some(Rect::new(0.0, 0.0, 100.0, 300.0)));
    let k: Vec<_> = (0..3).map(|i| t.add_node(c, leaf(), Some(Rect::xywh(0.0, i as f32 * 100.0, 100.0, 100.0)))).collect();
    assert!(t.child_take_focus(c, FocusSource::Application, None));
    assert_eq!(t.focused(), Some(k[2]));
    let d = t.add_node(r, Props { nav_entry_prefer_position: Some(EntryPrefer::PreferredChild), ..cont(Layout::Column) }, Some(Rect::new(100.0, 0.0, 200.0, 300.0)));
    let _x = t.add_node(d, leaf(), Some(Rect::xywh(100.0, 0.0, 100.0, 100.0)));
    let y = t.add_node(d, Props { preferred_focus: true, ..leaf() }, Some(Rect::xywh(100.0, 100.0, 100.0, 100.0)));
    assert!(t.child_take_focus(d, FocusSource::Application, None));
    assert_eq!(t.focused(), Some(y));
}

#[test]
fn maintain_y_entry_picks_closest_row() {
    // sp.js:81800 MAINTAIN_Y: entering a column from the side keeps the vertical position
    let mut t = Tree::new((1280.0, 800.0));
    let r = t.root();
    let row = t.add_node(r, cont(Layout::Row), Some(Rect::new(0.0, 0.0, 400.0, 300.0)));
    let a = t.add_node(row, cont(Layout::Column), Some(Rect::new(0.0, 0.0, 100.0, 300.0)));
    let ak: Vec<_> = (0..3).map(|i| t.add_node(a, leaf(), Some(Rect::xywh(0.0, i as f32 * 100.0, 100.0, 100.0)))).collect();
    let b = t.add_node(row, Props { nav_entry_prefer_position: Some(EntryPrefer::MaintainY), ..cont(Layout::Column) }, Some(Rect::new(200.0, 0.0, 300.0, 300.0)));
    let bk: Vec<_> = (0..3).map(|i| t.add_node(b, leaf(), Some(Rect::xywh(200.0, i as f32 * 100.0, 100.0, 100.0)))).collect();
    focus(&mut t, ak[2]);
    assert!(press(&mut t, Button::DirRight));
    assert_eq!(t.focused(), Some(bk[2]));
}

#[test]
fn geometric_picks_overlap_then_distance() {
    // sp.js:82082 FindClosetChildInDirection
    let mut t = Tree::new((1280.0, 800.0));
    let r = t.root();
    let g = t.add_node(r, cont(Layout::Geometric), Some(Rect::new(0.0, 0.0, 500.0, 500.0)));
    let me = t.add_node(g, keyed("me"), Some(Rect::new(0.0, 0.0, 100.0, 100.0)));
    let _far_overlap = t.add_node(g, keyed("far"), Some(Rect::new(0.0, 300.0, 100.0, 400.0)));
    let _near_no_overlap = t.add_node(g, keyed("near"), Some(Rect::new(150.0, 120.0, 250.0, 220.0)));
    let _near_overlap = t.add_node(g, keyed("nearov"), Some(Rect::new(50.0, 150.0, 150.0, 250.0)));
    focus(&mut t, me);
    assert!(press(&mut t, Button::DirDown));
    assert_eq!(focused_key(&t), "nearov"); // overlapping beats non-overlapping; nearer beats farther
    assert!(press(&mut t, Button::DirRight));
}

#[test]
fn tab_wrap() {
    // TabBar sp.js:347290: wrapAround defaults true
    assert_eq!(tab_step(0, -1, 4, None), Some(3));
    assert_eq!(tab_step(3, 1, 4, None), Some(0));
    assert_eq!(tab_step(3, 1, 4, Some(false)), None);
    assert_eq!(tab_step(1, 1, 4, Some(false)), Some(2));
}

#[test]
fn history_push_pop_restores_active_children_by_navkey() {
    // module 33787: Restore by navKey, unkeyed by position; policy decides re-focus
    let (mut t, g, cells) = grid();
    focus(&mut t, cells[5]); // b1
    let root = t.root();
    t.push_state(root);
    focus(&mut t, cells[0]);
    assert_eq!(focused_key(&t), "a0");
    t.pop_state(root, RestoreFocus::IfHadFocus);
    assert_eq!(focused_key(&t), "b1");
    // Never: active child restored but focus untouched
    focus(&mut t, cells[0]);
    t.save_state(root, "x");
    focus(&mut t, cells[8]);
    assert!(t.restore_state(root, "x", RestoreFocus::Never));
    assert_eq!(focused_key(&t), "c2");
    assert_eq!(t.active_child(g), Some(cells[0]));
    assert!(!t.restore_state(root, "missing", RestoreFocus::Always));
    // Always re-focuses the restored leaf
    t.restore_state(root, "x", RestoreFocus::Always);
    assert_eq!(focused_key(&t), "a0");
}

#[test]
fn history_survives_rebuilt_children_with_new_ids() {
    // Same navKeys, new node ids (page re-created): restore re-applies active child by key (sp.js:81175)
    let mut t = Tree::new((1280.0, 800.0));
    let r = t.root();
    let c = t.add_node(r, cont(Layout::Column), Some(Rect::new(0.0, 0.0, 100.0, 300.0)));
    let ks: Vec<_> = (0..3).map(|i| t.add_node(c, keyed(&format!("k{i}")), Some(Rect::xywh(0.0, i as f32 * 100.0, 100.0, 100.0)))).collect();
    focus(&mut t, ks[2]);
    t.save_state(r, "page");
    for k in ks {
        t.remove_node(k);
    }
    assert!(t.focused().is_none());
    let ks2: Vec<_> = (0..3).map(|i| t.add_node(c, keyed(&format!("k{i}")), Some(Rect::xywh(0.0, i as f32 * 100.0, 100.0, 100.0)))).collect();
    t.restore_state(r, "page", RestoreFocus::Always);
    assert_eq!(t.focused(), Some(ks2[2]));
}

/// A vertical scroll list: viewport 0..400 high, 20 rows of 100 px, container max scroll 1600.
fn list(n: usize, padding: f32) -> (Tree, NodeId, Vec<NodeId>) {
    let mut t = Tree::new((1280.0, 800.0));
    let r = t.root();
    let c = t.add_node(r, Props { scroll_into_view_when_child_focused: ScrollWhenChild::Yes, ..cont(Layout::Column) }, Some(Rect::new(0.0, 0.0, 300.0, 400.0)));
    let mut sb = ScrollBox::vertical(n as f32 * 100.0 - 400.0);
    sb.padding = Edges::top(padding);
    t.set_scroll_box(c, sb);
    let rows = (0..n).map(|i| t.add_node(c, leaf(), Some(Rect::xywh(0.0, i as f32 * 100.0, 300.0, 100.0)))).collect();
    (t, c, rows)
}
/// Re-lay the rows for the container's scroll position (the UI layer's job after applying a ScrollReq).
fn apply_scroll(t: &mut Tree, c: NodeId, rows: &[NodeId], top: f32) {
    t.set_scroll(Some(c), top, 0.0);
    for (i, &r) in rows.iter().enumerate() {
        t.set_rect(r, Rect::xywh(0.0, i as f32 * 100.0 - top, 300.0, 100.0));
    }
}

#[test]
fn scroll_nearest_is_smooth_and_minimal() {
    // sp.js:82343 P + sp.js:82513 O: visible => nothing; just below => smooth by overshoot
    let (mut t, c, rows) = list(20, 0.0);
    focus(&mut t, rows[0]);
    assert!(press(&mut t, Button::DirDown)); // row1 fully visible
    assert!(scrolls(&t.drain_events()).is_empty());
    for _ in 0..3 {
        press(&mut t, Button::DirDown);
    }
    // row 4 spans 400..500 in a 0..400 box: needs +100
    let s = scrolls(&t.drain_events());
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].target, ScrollTarget::Node(c));
    assert_eq!(s[0].top, Some(100.0));
    assert_eq!(s[0].behavior, Behavior::Smooth);
}

#[test]
fn scroll_padding_and_margin_are_honoured() {
    let (mut t, c, rows) = list(20, 20.0);
    focus(&mut t, rows[0]);
    t.drain_events();
    apply_scroll(&mut t, c, &rows, 500.0);
    // focus row 5 (rect 0..100 after scrolling 500): above padded edge (20) => scroll up by 20
    t.set_rect(rows[5], Rect::xywh(0.0, 0.0, 300.0, 100.0));
    t.transfer_focus(FocusSource::Gamepad, Some(rows[5]), Some(Button::DirUp));
    let s = scrolls(&t.drain_events());
    assert_eq!(s[0].top, Some(480.0));
}

#[test]
fn scroll_far_target_jumps_instantly_then_500ms_window() {
    // behavior_choice (sp.js:82372): > 0.4*vh off-screen => 'auto', and for the next 500 ms everything is instant
    let (mut t, c, rows) = list(30, 0.0);
    focus(&mut t, rows[0]);
    t.now_ms = 1000.0;
    // jump straight to row 20 (2000 px below a 800 high window; rect.top 2000 > 800+320)
    t.transfer_focus(FocusSource::Gamepad, Some(rows[20]), Some(Button::DirDown));
    let s = scrolls(&t.drain_events());
    assert_eq!(s[0].behavior, Behavior::Auto);
    assert_eq!(s[0].top, Some(1700.0));
    apply_scroll(&mut t, c, &rows, 1700.0);
    // 300 ms later a near move is still instant
    t.now_ms = 1300.0;
    t.transfer_focus(FocusSource::Gamepad, Some(rows[21]), Some(Button::DirDown));
    let s = scrolls(&t.drain_events());
    assert!(s.iter().all(|x| x.behavior == Behavior::Auto));
    apply_scroll(&mut t, c, &rows, 1800.0);
    // after the window a near move animates again
    t.now_ms = 2000.0;
    t.transfer_focus(FocusSource::Gamepad, Some(rows[22]), Some(Button::DirDown));
    let s = scrolls(&t.drain_events());
    assert!(s.iter().all(|x| x.behavior == Behavior::Smooth), "{s:?}");
}

#[test]
fn scroll_instant_while_restoring_history() {
    let (mut t, _c, rows) = list(20, 0.0);
    focus(&mut t, rows[0]);
    t.restoring_history = true;
    t.transfer_focus(FocusSource::Gamepad, Some(rows[4]), Some(Button::DirDown));
    let s = scrolls(&t.drain_events());
    assert_eq!(s[0].behavior, Behavior::Auto);
}

#[test]
fn scroll_first_focus_without_gamepad_jumps() {
    // "no previous element and source != GAMEPAD" => 'auto'
    let (mut t, _c, rows) = list(20, 0.0);
    t.transfer_focus(FocusSource::Application, Some(rows[6]), None);
    let s = scrolls(&t.drain_events());
    assert_eq!(s[0].behavior, Behavior::Auto);
}

#[test]
fn scroll_region_to_edge_when_no_more_targets() {
    // sp.js:82462 D()/z(): at the last item, DOWN scrolls the region to its end
    let (mut t, c, rows) = list(3, 0.0);
    let mut p = t.props(c).clone();
    p.scroll_region_to_start_or_end = true;
    t.set_props(c, p);
    // make it scrollable: 3 rows of 200 in a 400 box
    t.set_scroll_box(c, ScrollBox::vertical(200.0));
    for (i, &r) in rows.iter().enumerate() {
        t.set_rect(r, Rect::xywh(0.0, i as f32 * 100.0, 300.0, 100.0));
    }
    focus(&mut t, rows[1]);
    press(&mut t, Button::DirDown);
    let s = scrolls(&t.drain_events());
    assert!(s.iter().any(|x| x.top == Some(200.0) && x.behavior == Behavior::Smooth), "{s:?}");
}

#[test]
fn sparse_content_far_target_scrolls_instead_of_focusing() {
    // sp.js:81905 BTryInternalNavigation: NoTransformSparseContent + target farther than vw/3.33 => scroll only
    let mut t = Tree::new((1000.0, 800.0));
    let r = t.root();
    let row = t.add_node(r, Props { scroll_into_view_type: Some(ScrollType::NoTransformSparseContent), ..cont(Layout::Row) }, Some(Rect::new(0.0, 0.0, 1000.0, 200.0)));
    t.set_scroll_box(row, ScrollBox::horizontal(5000.0));
    let a = t.add_node(row, leaf(), Some(Rect::xywh(0.0, 0.0, 100.0, 100.0)));
    let b = t.add_node(row, leaf(), Some(Rect::xywh(3000.0, 0.0, 100.0, 100.0)));
    focus(&mut t, a);
    assert!(press(&mut t, Button::DirRight));
    assert_eq!(t.focused(), Some(a)); // focus did not move
    let s = scrolls(&t.drain_events());
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].left, Some(1000.0 / 3.33));
    // once the target is near, focus moves
    t.set_rect(b, Rect::xywh(500.0, 0.0, 100.0, 100.0));
    assert!(press(&mut t, Button::DirRight));
    assert_eq!(t.focused(), Some(b));
}

#[test]
fn on_move_handler_catches_unhandled_direction() {
    // sp.js:81839: internal navigation first, then onMoveX; returning true stops the bubble
    use std::cell::Cell;
    use std::rc::Rc;
    let hit = Rc::new(Cell::new(0));
    let h2 = hit.clone();
    let mut t = Tree::new((1280.0, 800.0));
    let r = t.root();
    let c = t.add_node(
        r,
        Props { on_move: [None, Some(Rc::new(move |_| { h2.set(h2.get() + 1); true })), None, None], ..cont(Layout::Column) },
        Some(Rect::new(0.0, 0.0, 100.0, 100.0)),
    );
    let a = t.add_node(c, leaf(), Some(Rect::xywh(0.0, 0.0, 100.0, 100.0)));
    focus(&mut t, a);
    assert!(press(&mut t, Button::DirRight));
    assert_eq!(hit.get(), 1);
    assert!(!t.drain_events().contains(&NavEvent::FailedNav));
}

#[test]
fn button_handlers_bubble_and_unhandled_reaches_root() {
    use std::rc::Rc;
    let mut t = Tree::new((1280.0, 800.0));
    let r = t.root();
    let c = t.add_node(r, Props { on_button: vec![(Button::Cancel, Rc::new(|_| true))], ..cont(Layout::Column) }, Some(Rect::new(0.0, 0.0, 100.0, 100.0)));
    let a = t.add_node(c, leaf(), Some(Rect::xywh(0.0, 0.0, 100.0, 100.0)));
    focus(&mut t, a);
    assert!(press(&mut t, Button::Cancel));
    assert!(!press(&mut t, Button::Options));
    assert!(t.drain_events().contains(&NavEvent::Unhandled(Button::Options)));
}

#[test]
fn autofocus_and_unmount_transfer() {
    let mut t = Tree::new((1280.0, 800.0));
    let r = t.root();
    let c = t.add_node(r, cont(Layout::Column), Some(Rect::new(0.0, 0.0, 100.0, 300.0)));
    let a = t.add_node(c, Props { auto_focus: true, ..leaf() }, Some(Rect::xywh(0.0, 0.0, 100.0, 100.0)));
    let b = t.add_node(c, leaf(), Some(Rect::xywh(0.0, 100.0, 100.0, 100.0)));
    t.flush_deferred_focus();
    assert_eq!(t.focused(), Some(a));
    // focused node unmounted without focusableIfEmpty ancestor => blur
    t.remove_node(a);
    assert_eq!(t.focused(), None);
    assert!(press(&mut t, Button::DirDown) || t.focused() == Some(b)); // tree re-takes focus on next press
    assert_eq!(t.focused(), Some(b));
}

#[test]
fn visible_child_take_focus_prefers_on_screen() {
    // module 48294
    let (mut t, c, rows) = list(20, 0.0);
    apply_scroll(&mut t, c, &rows, 1000.0); // rows 10..13 visible (rect.top 0..400)
    assert!(t.visible_child_take_focus(c, FocusSource::Application));
    let f = t.focused().unwrap();
    assert!(rows[10..14].contains(&f), "{f}");
}

#[test]
fn intercept_declines_with_empty_tree() {
    // the global tree has nothing registered => legacy handlers keep running
    assert!(!intercept(crate::input::Action::Down, false));
}
