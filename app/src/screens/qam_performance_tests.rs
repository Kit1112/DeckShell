use crate::screens::expected_text;
use super::*;
fn ready() -> PerfState {
    PerfState { advanced: true, perf_avail: Some(true), overlay_observed: Some(0), vrs_avail: true, refresh_avail: true, vrr_supported: true, ..PerfState::default() }
}
#[test]
fn tdp_enable_switch_follows_tdp_readback() {
    let mut b = B::new();
    performance_rows(&mut b, &ready(), Some((14, 3, 15)), None);
    assert!(b.items.contains(&Item::TdpOn) && b.items.contains(&Item::Tdp));
    let mut b = B::new();
    performance_rows(&mut b, &ready(), None, None);
    assert!(!b.items.contains(&Item::TdpOn));
}
#[test]
fn vrr_toggle_only_when_steam_reports_support() {
    for (supported, expected) in [(true, true), (false, false)] {
        let mut b = B::new();
        let mut p = ready();
        p.vrr_supported = supported;
        performance_rows(&mut b, &p, None, None);
        assert_eq!(b.items.contains(&Item::Vrr), expected, "supported={supported}");
    }
}
#[test]
fn tdp_toggle_writes_hardware_target_from_the_stock_switch() {
    // on keeps the shown cap inside the hardware range; off restores the hardware maximum
    assert_eq!(tdp_toggle_target(true, 9, 3, 15), 9);
    assert_eq!(tdp_toggle_target(true, 2, 3, 15), 3);
    assert_eq!(tdp_toggle_target(true, 20, 3, 15), 15);
    assert_eq!(tdp_toggle_target(false, 9, 3, 15), 15);
}
#[test]
fn tdp_is_only_interactive_with_readback_and_no_pending_or_failed_write() {
    for (tdp, pending, failed, expected) in [
        (Some((14, 3, 15)), false, false, true),
        (None, false, false, false),
        (Some((14, 3, 15)), true, false, false),
        (Some((14, 3, 15)), false, true, false),
    ] {
        let mut b = B::new();
        let mut p = ready();
        p.tdp_pending = pending;
        p.tdp_failed = failed;
        performance_rows(&mut b, &p, tdp, None);
        assert_eq!(b.items.contains(&Item::Tdp), expected);
    }
}
#[test]
fn gpu_controls_remain_reachable_during_pending_and_failed_writes() {
    for (gpu, pending, failed, toggle, slider) in [
        (Some((false, None, 200, 1600)), false, false, true, false),
        (Some((true, Some(300), 200, 1600)), false, false, true, true),
        (None, false, false, false, false),
        (Some((true, None, 200, 1600)), false, true, true, false),
        (Some((true, Some(300), 200, 1600)), true, false, true, true),
        (Some((true, Some(300), 200, 1600)), false, true, true, true),
    ] {
        let mut b = B::new();
        let mut p = ready();
        p.gpu_pending = pending;
        p.gpu_failed = failed;
        performance_rows(&mut b, &p, None, gpu);
        assert_eq!(b.items.contains(&Item::GpuOn), toggle);
        assert_eq!(b.items.contains(&Item::Gpu), slider);
    }
}
#[test]
fn perf_rows_need_confirmed_perf_channel() {
    for avail in [None, Some(false)] {
        let mut b = B::new();
        let mut p = ready();
        p.perf_avail = avail;
        p.gs_fps = None;
        performance_rows(&mut b, &p, None, None);
        for item in [Item::FpsLimit, Item::Refresh, Item::Vrr, Item::Filter, Item::Scaler, Item::Sharp, Item::Profile, Item::ProfileReset] {
            assert!(!b.items.contains(&item), "{item:?} listed without the Perf channel");
        }
    }
}
#[test]
fn gamescope_controls_have_visible_labels_without_perf_channel() {
    let mut p = ready();
    p.perf_avail = Some(false);
    p.gs_refresh = Some(90);
    p.refresh_hz = 90.0;
    p.refresh_game_policy = true;
    p.gs_filter = Some(0);
    p.gs_scaler = Some(0);
    let mut b = B::new();
    performance_rows(&mut b, &p, None, None);
    for item in [Item::Refresh, Item::Filter, Item::Scaler] { assert!(b.items.contains(&item)); }
    let labels: Vec<String> = b.rows.iter().filter(|r| r.kind == 6).map(|r| r.label.to_string()).collect();
    assert!(labels.iter().any(|s| s.contains(expected_text("90 Гц", "90 Hz")) && s.contains(i18n::tr_ref("qam.sets_the_game").trim())));
    assert!(labels.iter().any(|s| s.contains(i18n::tr_ref("qam.linear"))));
    assert!(labels.iter().any(|s| s.contains(i18n::tr_ref("qam.automotive"))));
    assert!(!b.items.contains(&Item::Vrr));
}
#[test]
fn gamescope_incapable_output_hides_vrr_even_with_perf() {
    let mut p = ready();
    p.gs_vrr_capable = Some(false);
    let mut b = B::new();
    performance_rows(&mut b, &p, None, None);
    assert!(!b.items.contains(&Item::Vrr));
}
#[test]
fn fps_row_without_perf_channel_follows_gamescope_atom() {
    // Only the FPS atom, and only with a value from the list; the other perf rows stay hidden.
    let mut p = ready();
    p.perf_avail = Some(false);
    for (atom, shown) in [(Some(60), true), (Some(45), true), (Some(75), false), (None, false)] {
        p.gs_fps = atom;
        p.fps = p.fps_opts.iter().position(|&x| Some(x) == atom).unwrap_or(0);
        let mut b = B::new();
        performance_rows(&mut b, &p, None, None);
        assert_eq!(b.items.contains(&Item::FpsLimit), shown, "atom={atom:?}");
        assert!(!b.items.contains(&Item::Refresh) && !b.items.contains(&Item::Filter) && !b.items.contains(&Item::Vrr));
    }
}
#[test]
fn every_listed_perf_control_is_focusable_and_mapped_to_readback() {
    let mut b = B::new();
    performance_rows(&mut b, &ready(), Some((14, 3, 15)), Some((false, None, 200, 1600)));
    for item in [Item::Overlay, Item::OverlayLevel, Item::Advanced, Item::Tdp, Item::GpuOn, Item::FpsLimit, Item::Refresh, Item::Vrr, Item::Tearing, Item::Vrs, Item::Filter, Item::Scaler, Item::Sharp, Item::Profile, Item::ProfileReset] {
        assert!(b.items.contains(&item), "{item:?} missing");
    }
    assert_eq!(b.items.len(), b.rows.iter().filter(|r| r.fid >= 0).count());
}
