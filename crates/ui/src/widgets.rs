//! Widgets that more than one window draws: the knob, and the drag that
//! turns it.

use egui::{
    Color32, Context, PointerButton, Pos2, Response, Sense, Shape, Stroke, Ui, Vec2, WidgetInfo,
    pos2,
};
use std::f32::consts::PI;
use std::ops::RangeInclusive;

/// How many points of drag take a knob from one end to the other.
const TRAVEL: f32 = 150.0;
/// How much slower a drag is with Ctrl held.
const FINE: f32 = 0.1;
/// Where a knob's pointer is at the low end of the range and how far it
/// turns to the high end, in radians clockwise from straight up.
const SWEEP: (f32, f32) = (-0.75 * PI, 1.5 * PI);

/// A number for a drag that starts now, for the `drag_id` of its edits.
/// The windows share the count, so that two drags never have one number.
pub(crate) fn next_drag_id(ctx: &Context) -> u64 {
    let id = egui::Id::new("fl-rs drags");
    ctx.data_mut(|data| {
        let drags = data.get_temp_mut_or_default::<u64>(id);
        *drags += 1;
        *drags
    })
}

/// A drag that changes a value.
#[derive(Clone, Copy)]
struct Dragged {
    id: u64,
    /// Where the drag has taken the value, before rounding.
    value: f32,
    /// The value that went out last, and to begin with the one the drag
    /// started from.
    sent: i32,
}

/// What a drag up or down on `response` makes of `value`: the new value
/// and the drag's number, when the drag has moved the value on from what
/// it gave last.
///
/// `travel` is how many points take the value through `range`, and with
/// Ctrl held it is ten times as many.
pub(crate) fn drag_value(
    ui: &Ui,
    response: &Response,
    value: i32,
    range: RangeInclusive<i32>,
    travel: f32,
) -> Option<(i32, u64)> {
    let id = response.id.with("value drag");
    if response.drag_started_by(PointerButton::Primary) {
        let drag = Dragged {
            id: next_drag_id(ui.ctx()),
            value: value as f32,
            sent: value,
        };
        ui.data_mut(|data| data.insert_temp(id, drag));
    }
    if !response.dragged_by(PointerButton::Primary) {
        ui.data_mut(|data| data.remove::<Dragged>(id));
        return None;
    }
    let mut drag: Dragged = ui.data(|data| data.get_temp(id))?;
    let (low, high) = (*range.start() as f32, *range.end() as f32);
    let fine = ui.input(|input| input.modifiers.command);
    let speed = (high - low) / travel * if fine { FINE } else { 1.0 };
    drag.value = (drag.value - response.drag_delta().y * speed).clamp(low, high);
    let value = drag.value.round() as i32;
    let moved = value != drag.sent;
    drag.sent = value;
    ui.data_mut(|data| data.insert_temp(id, drag));
    moved.then_some((value, drag.id))
}

/// A knob for a value in FL's own units.
///
/// A drag up turns it up and a drag down turns it down, slowly with Ctrl
/// held. A double click puts it back to `default`. The pointer over it
/// shows `hint`, which is the value as FL words it.
pub(crate) struct Knob<'a> {
    /// What a screen reader calls the knob, and a test.
    pub label: &'a str,
    pub value: i32,
    pub range: RangeInclusive<i32>,
    pub default: i32,
    pub hint: String,
    /// The colour of the arc from the default to the value.
    pub colour: Color32,
    /// Whether the knob can be turned. One that cannot is drawn faint.
    pub enabled: bool,
}

impl Knob<'_> {
    /// Draw the knob in a square of `size` points. Gives the new value
    /// when the user turned the knob, with the drag's number, and `None`
    /// for that when a double click put the default back.
    pub fn show(self, ui: &mut Ui, size: f32) -> Option<(i32, Option<u64>)> {
        let sense = if self.enabled {
            Sense::click_and_drag()
        } else {
            Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), sense);
        let (label, value) = (self.label, f64::from(self.value));
        response.widget_info(|| WidgetInfo::slider(self.enabled, value, label));

        let turned = if response.double_clicked() {
            (self.value != self.default).then_some((self.default, None))
        } else {
            let range = self.range.clone();
            let dragged = drag_value(ui, &response, self.value, range, TRAVEL);
            dragged.map(|(value, drag)| (value, Some(drag)))
        };

        if ui.is_rect_visible(rect) {
            let visuals = ui.visuals();
            let (body, line) = if self.enabled {
                let widget = ui.style().interact(&response);
                (widget.bg_fill, widget.fg_stroke.color)
            } else {
                let widget = &visuals.widgets.noninteractive;
                (widget.bg_fill, visuals.weak_text_color())
            };
            let colour = if self.enabled { self.colour } else { line };
            let (centre, radius) = (rect.center(), size / 2.0 - 1.0);
            let painter = ui.painter_at(rect);
            painter.circle_filled(centre, radius - 2.5, body);
            let from = self.turn(self.default);
            let to = self.turn(self.value);
            let arc = arc(centre, radius - 1.0, from.min(to), from.max(to));
            painter.add(Shape::line(arc, Stroke::new(2.0, colour)));
            let tip = on_circle(centre, radius - 2.5, to);
            painter.line_segment([centre, tip], Stroke::new(1.5, line));
        }
        // A tooltip hides while a button is down, and a drag is when the
        // value is wanted most.
        if response.dragged() {
            response.show_tooltip_text(self.hint);
        } else {
            response.on_hover_text(self.hint);
        }
        turned
    }

    /// The angle of the pointer for a value, in radians clockwise from
    /// straight up.
    fn turn(&self, value: i32) -> f32 {
        let (low, high) = (*self.range.start(), *self.range.end());
        let span = (high - low).max(1) as f32;
        let along = (value.clamp(low, high) - low) as f32 / span;
        SWEEP.0 + along * SWEEP.1
    }
}

/// The point on a circle at an angle clockwise from straight up.
fn on_circle(centre: Pos2, radius: f32, angle: f32) -> Pos2 {
    pos2(
        centre.x + radius * angle.sin(),
        centre.y - radius * angle.cos(),
    )
}

/// An arc as a line with a point every eighth of a radian.
fn arc(centre: Pos2, radius: f32, from: f32, to: f32) -> Vec<Pos2> {
    let steps = ((to - from) * 8.0).ceil().max(1.0) as usize;
    let angle = |step: usize| from + (to - from) * step as f32 / steps as f32;
    (0..=steps)
        .map(|step| on_circle(centre, radius, angle(step)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn knob(value: i32) -> Knob<'static> {
        Knob {
            label: "",
            value,
            range: 0..=12_800,
            default: 10_000,
            hint: String::new(),
            colour: Color32::WHITE,
            enabled: true,
        }
    }

    #[test]
    fn a_knob_turns_three_quarters_of_a_circle() {
        assert_eq!(knob(0).turn(0), -0.75 * PI);
        assert_eq!(knob(0).turn(6400), 0.0);
        assert_eq!(knob(0).turn(12_800), 0.75 * PI);
        // A value outside the range stops at its end.
        assert_eq!(knob(0).turn(99_999), 0.75 * PI);
        assert_eq!(knob(0).turn(-5), -0.75 * PI);
    }

    #[test]
    fn the_pointer_starts_at_the_lower_left_and_goes_over_the_top() {
        let centre = pos2(10.0, 10.0);
        let near = |a: Pos2, b: Pos2| (a - b).length() < 1e-4;
        assert!(near(on_circle(centre, 5.0, 0.0), pos2(10.0, 5.0)));
        assert!(near(on_circle(centre, 5.0, PI / 2.0), pos2(15.0, 10.0)));
        let low = on_circle(centre, 5.0, SWEEP.0);
        assert!(low.x < 10.0 && low.y > 10.0, "{low:?}");

        let arc = arc(centre, 5.0, 0.0, PI / 2.0);
        assert_eq!(arc.len(), 14);
        assert!(near(arc[0], pos2(10.0, 5.0)) && near(arc[13], pos2(15.0, 10.0)));
        // An arc from an angle to itself is still a line to draw.
        assert_eq!(super::arc(centre, 5.0, 1.0, 1.0).len(), 2);
    }

    #[test]
    fn every_drag_has_a_number_of_its_own() {
        let ctx = Context::default();
        assert_eq!(next_drag_id(&ctx), 1);
        assert_eq!(next_drag_id(&ctx), 2);
        assert_eq!(next_drag_id(&Context::default()), 1);
    }
}
