//! The colours, after FL's default skin: slate grey panels, darker wells
//! for readouts and meters, and orange for what is selected.

use egui::{Color32, Context, CornerRadius, Stroke, Theme};

/// FL's orange, for what is selected or switched on.
pub(crate) const ACCENT: Color32 = Color32::from_rgb(242, 150, 52);
/// The light green of FL's readouts.
pub(crate) const READOUT: Color32 = Color32::from_rgb(160, 225, 175);

/// Put the colours on the dark style, once per context. The windows are
/// drawn the same with it or without it, so a context that never gets it
/// still works.
pub(crate) fn apply(ctx: &Context) {
    let id = egui::Id::new("fl-rs theme");
    if ctx.data(|data| data.get_temp::<bool>(id)).is_some() {
        return;
    }
    ctx.data_mut(|data| data.insert_temp(id, true));
    ctx.style_mut_of(Theme::Dark, |style| {
        let visuals = &mut style.visuals;
        visuals.panel_fill = Color32::from_rgb(52, 58, 64);
        visuals.window_fill = Color32::from_rgb(46, 52, 57);
        visuals.faint_bg_color = Color32::from_rgb(58, 65, 71);
        visuals.extreme_bg_color = Color32::from_rgb(25, 29, 32);
        visuals.code_bg_color = Color32::from_rgb(36, 41, 45);
        visuals.window_stroke = Stroke::new(1.0, Color32::from_rgb(30, 34, 37));
        visuals.hyperlink_color = ACCENT;
        visuals.selection.bg_fill = ACCENT;
        visuals.selection.stroke = Stroke::new(1.0, Color32::from_rgb(28, 24, 20));

        let radius = CornerRadius::same(3);
        let widgets = &mut visuals.widgets;
        widgets.noninteractive.bg_fill = visuals.panel_fill;
        widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(38, 43, 47));
        for (widget, fill) in [
            (&mut widgets.inactive, Color32::from_rgb(70, 78, 85)),
            (&mut widgets.hovered, Color32::from_rgb(86, 95, 103)),
            (&mut widgets.active, Color32::from_rgb(100, 110, 119)),
            (&mut widgets.open, Color32::from_rgb(78, 87, 94)),
        ] {
            widget.bg_fill = fill;
            widget.weak_bg_fill = fill;
            widget.corner_radius = radius;
        }
        widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.6));
    });
}
