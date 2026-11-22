//! Footer hint clicks (touch/mouse): same action as the gamepad button the hint shows.
//! Uses App::hint_spec (app.rs) as the single hint list; index -1 = left STEAM hint.
use crate::app::App;
use crate::input::Action;
use crate::{AppWindow, Shell};
use slint::ComponentHandle;

fn actions(a: &App) -> Vec<Action> {
    a.hint_spec().into_iter().map(|h| h.4).collect() // same list update_hint renders
}

pub fn wire(ui: &AppWindow) {
    ui.global::<Shell>().on_footer_clicked(|i| {
        crate::with_app(|a| {
            let act = if i < 0 { Some(Action::Steam) } else { actions(a).get(i as usize).copied() };
            if let Some(act) = act {
                a.handle(act); // same path as the pad, without switching the glyph mode
            }
        });
    });
}
