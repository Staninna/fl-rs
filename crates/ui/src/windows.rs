//! The windows between the top bar and the status line, docked with
//! `craft-ui`.

use crate::{Command, UiState, browser, channel_rack, mixer, piano_roll, playlist};
use craft_ui::docking::{Action, DockArea, DockStyle, Layout, Node, Permissions};
use craft_ui::layout::{SplitAxis, SplitSize};
use egui::{Id, Key, Ui};

/// How wide the browser's column is when it opens.
const BROWSER: f32 = 240.0;

/// FL's five main windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Window {
    Playlist,
    ChannelRack,
    PianoRoll,
    Browser,
    Mixer,
}

impl Window {
    pub const ALL: [Window; 5] = [
        Window::Playlist,
        Window::ChannelRack,
        Window::PianoRoll,
        Window::Browser,
        Window::Mixer,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Window::Playlist => "Playlist",
            Window::ChannelRack => "Channel rack",
            Window::PianoRoll => "Piano roll",
            Window::Browser => "Browser",
            Window::Mixer => "Mixer",
        }
    }

    /// The key that shows and hides the window, as in FL.
    pub fn key(self) -> Key {
        match self {
            Window::Playlist => Key::F5,
            Window::ChannelRack => Key::F6,
            Window::PianoRoll => Key::F7,
            Window::Browser => Key::F8,
            Window::Mixer => Key::F9,
        }
    }
}

/// The id the layout is kept under in egui's memory.
fn layout_id() -> Id {
    Id::new("fl-rs windows")
}

/// The windows a click in the top bar or on the welcome screen asked for
/// this frame, which [`show`] handles as it does their keys.
fn requests_id() -> Id {
    Id::new("fl-rs window requests")
}

/// Show or hide `window` as its key does, from a button.
pub(crate) fn toggle(ctx: &egui::Context, window: Window) {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Vec<Window>>(requests_id())
            .push(window);
    });
}

/// Whether `window` is the front tab of its group, as of the last frame.
pub(crate) fn shown(ctx: &egui::Context, window: Window) -> bool {
    ctx.data(|data| data.get_temp::<Layout<Window>>(layout_id()))
        .is_some_and(|layout| in_view(&layout, window))
}

/// Whether the project has anything to show: a fresh app's has nothing.
fn has_content(project: &model::Project) -> bool {
    !project.channels.is_empty()
        || !project.patterns.is_empty()
        || project.arrangements.iter().any(|a| !a.items.is_empty())
}

pub(crate) fn show(ui: &mut Ui, state: &UiState, out: &mut Vec<Command>) {
    let id = layout_id();
    // The app starts with nothing open, on the welcome screen. The first
    // project with anything in it opens the playlist, unless a window was
    // opened by then.
    let mut layout: Layout<Window> = ui.data(|data| data.get_temp(id)).unwrap_or(Layout {
        root: None,
        floating: Vec::new(),
    });
    let greeted = Id::new("fl-rs opened a project");
    if has_content(&state.project) && ui.data(|data| data.get_temp::<bool>(greeted)).is_none() {
        ui.data_mut(|data| data.insert_temp(greeted, true));
        if layout.panels().is_empty() {
            press(&mut layout, Window::Playlist);
        }
    }

    let requested: Vec<Window> =
        ui.data_mut(|data| data.remove_temp(requests_id()).unwrap_or_default());
    for window in requested {
        press(&mut layout, window);
    }
    for window in Window::ALL {
        if ui.input(|input| input.key_pressed(window.key())) {
            press(&mut layout, window);
        }
    }

    // The piano roll takes its keys before any window is drawn, so that
    // Delete with the pointer over it removes notes and no playlist items.
    // The browser does the same for the arrows, which the channel rack
    // would take.
    if in_view(&layout, Window::PianoRoll) {
        piano_roll::keys(ui, state, out);
    }
    if in_view(&layout, Window::Browser) {
        browser::keys(ui, out);
    }

    if layout.panels().is_empty() {
        welcome(ui, &mut layout, out);
    } else {
        let style = DockStyle::from_ui(ui);
        let drawn = DockArea::new(id).show(
            ui,
            &layout,
            &style,
            |window| window.name().to_owned(),
            |_| Permissions::default(),
            |ui, window| match window {
                Window::Playlist => playlist::show(ui, state, out),
                Window::ChannelRack => channel_rack::show(ui, state, out),
                Window::Mixer => mixer::show(ui, state, out),
                Window::PianoRoll => piano_roll::show(ui, state, out),
                Window::Browser => browser::show(ui, state, out),
            },
        );
        for action in drawn.actions {
            // A move the layout turns down leaves it as it was.
            let _ = layout.apply(action);
        }
    }
    ui.data_mut(|data| data.insert_temp(id, layout));
}

/// The screen with no window open: the keys that open each one, which
/// open it on a click too, and how to open a project.
fn welcome(ui: &mut Ui, layout: &mut Layout<Window>, out: &mut Vec<Command>) {
    use egui::{RichText, vec2};
    let rect = ui.max_rect();
    ui.painter()
        .rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
    let width = 320.0_f32.min(rect.width() - 32.0).max(120.0);
    let height = 330.0;
    let top = rect.top() + ((rect.height() - height) / 2.0).max(16.0);
    let card = egui::Rect::from_min_size(
        egui::pos2(rect.center().x - width / 2.0, top),
        vec2(width, height),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(card), |ui| {
        egui::Frame::new()
            .fill(ui.visuals().panel_fill)
            .stroke(ui.visuals().window_stroke)
            .corner_radius(6.0)
            .inner_margin(18.0)
            .show(ui, |ui| {
                ui.set_width(width - 36.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new("fl-rs")
                            .size(28.0)
                            .strong()
                            .color(crate::theme::ACCENT),
                    );
                    ui.weak("No window is open");
                });
                ui.add_space(12.0);
                for window in Window::ALL {
                    let key = format!("{:?}", window.key());
                    let row = ui.horizontal(|ui| {
                        egui::Frame::new()
                            .fill(ui.visuals().extreme_bg_color)
                            .corner_radius(3.0)
                            .inner_margin(vec2(6.0, 2.0))
                            .show(ui, |ui| {
                                ui.set_min_width(28.0);
                                ui.label(
                                    RichText::new(key)
                                        .monospace()
                                        .strong()
                                        .color(crate::theme::READOUT),
                                );
                            });
                        ui.label(window.name());
                    });
                    let click = ui.interact(
                        row.response.rect,
                        Id::new(("welcome", window.name())),
                        egui::Sense::click(),
                    );
                    if click.hovered() {
                        ui.painter().rect_stroke(
                            row.response.rect.expand(2.0),
                            3.0,
                            egui::Stroke::new(1.0, crate::theme::ACCENT),
                            egui::StrokeKind::Outside,
                        );
                    }
                    if click.clicked() {
                        press(layout, window);
                    }
                }
                ui.add_space(12.0);
                ui.separator();
                ui.vertical_centered(|ui| {
                    if ui.button("Open a project…").clicked() {
                        out.push(Command::Choose(crate::Dialog::Open));
                    }
                    ui.weak("or double-click one in the browser (F8)");
                    ui.weak("Space plays and stops");
                });
            });
    });
}

/// What a window's key does: close the window when it is in view, and
/// otherwise open it or bring its tab to the front.
///
/// The browser opens in a column of its own down the left edge, where FL
/// has it, so that a sample can be dragged from it onto the window beside
/// it. Every other window opens as a tab beside the first window that is
/// not the browser. A change the layout turns down leaves it as it was.
fn press(layout: &mut Layout<Window>, window: Window) {
    if in_view(layout, window) {
        let _ = layout.apply(Action::Close { panel: window });
        return;
    }
    let is_browser = window == Window::Browser;
    let mut others = layout.panels().into_iter().copied();
    let beside = others.find(|panel| *panel != Window::Browser);
    // The browser splits what is docked in two when it opens, and so does
    // the first window to open beside a browser that is on its own.
    let splits = !layout.contains(&window) && (is_browser || beside.is_none());
    let Some(root) = layout.root.clone().filter(|_| splits) else {
        let anchor = beside.filter(|_| !is_browser);
        let _ = layout.apply(Action::Open {
            panel: window,
            anchor,
        });
        return;
    };
    let opened = Box::new(Node::Tabs {
        panels: vec![window],
        active: 0,
    });
    let (first, second) = if is_browser {
        (opened, Box::new(root))
    } else {
        (Box::new(root), opened)
    };
    let mut split = layout.clone();
    split.root = Some(Node::Split {
        axis: SplitAxis::Horizontal,
        size: SplitSize::FixedFirst(BROWSER),
        first,
        second,
    });
    if split.validate().is_ok() {
        *layout = split;
    }
}

/// Whether the window is the front tab of its group.
fn in_view(layout: &Layout<Window>, window: Window) -> bool {
    fn node_shows(node: &Node<Window>, window: Window) -> bool {
        match node {
            Node::Tabs { panels, active } => panels.get(*active) == Some(&window),
            Node::Split { first, second, .. } => {
                node_shows(first, window) || node_shows(second, window)
            }
            Node::Stack { entries } => entries
                .iter()
                .any(|entry| entry.panel == window && entry.open),
        }
    }
    let floating = |group: &craft_ui::docking::Floating<Window>| {
        group.panels.get(group.active) == Some(&window)
    };
    layout
        .root
        .as_ref()
        .is_some_and(|root| node_shows(root, window))
        || layout.floating.iter().any(floating)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_opens_fronts_and_closes_its_window() {
        let mut layout = Layout {
            root: Some(Node::Tabs {
                panels: vec![Window::Playlist],
                active: 0,
            }),
            floating: Vec::new(),
        };
        press(&mut layout, Window::Mixer);
        assert!(in_view(&layout, Window::Mixer));
        assert!(layout.contains(&Window::Playlist));
        assert!(!in_view(&layout, Window::Playlist));

        // The playlist is behind the mixer, so its key brings it forward.
        press(&mut layout, Window::Playlist);
        assert!(in_view(&layout, Window::Playlist));
        assert!(layout.contains(&Window::Mixer));

        press(&mut layout, Window::Playlist);
        assert!(!layout.contains(&Window::Playlist));
        press(&mut layout, Window::Mixer);
        assert!(layout.panels().is_empty());

        // With nothing open a key starts a new group.
        press(&mut layout, Window::Browser);
        assert!(in_view(&layout, Window::Browser));
    }

    /// The windows from left to right, a group of tabs at a time.
    fn groups(layout: &Layout<Window>) -> Vec<Vec<Window>> {
        fn walk(node: &Node<Window>, out: &mut Vec<Vec<Window>>) {
            match node {
                Node::Tabs { panels, .. } => out.push(panels.clone()),
                Node::Split { first, second, .. } => {
                    walk(first, out);
                    walk(second, out);
                }
                Node::Stack { entries } => out.push(entries.iter().map(|e| e.panel).collect()),
            }
        }
        let mut out = Vec::new();
        layout.root.iter().for_each(|root| walk(root, &mut out));
        out
    }

    #[test]
    fn the_browser_has_a_column_of_its_own_on_the_left() {
        use Window::{Browser, ChannelRack, Mixer, Playlist};
        let mut layout = Layout {
            root: Some(Node::Tabs {
                panels: vec![Playlist],
                active: 0,
            }),
            floating: Vec::new(),
        };
        press(&mut layout, Browser);
        assert_eq!(groups(&layout), [vec![Browser], vec![Playlist]]);
        let Some(Node::Split { axis, size, .. }) = &layout.root else {
            panic!("{layout:?}");
        };
        assert_eq!(
            (*axis, *size),
            (SplitAxis::Horizontal, SplitSize::FixedFirst(BROWSER))
        );
        // Both are in view, and a window that opens now is a tab beside
        // the playlist.
        assert!(in_view(&layout, Browser) && in_view(&layout, Playlist));
        press(&mut layout, ChannelRack);
        assert_eq!(
            groups(&layout),
            [vec![Browser], vec![Playlist, ChannelRack]]
        );
        assert!(in_view(&layout, Browser) && in_view(&layout, ChannelRack));

        // On its own the browser keeps its column when a window opens.
        press(&mut layout, ChannelRack);
        press(&mut layout, Playlist);
        assert_eq!(groups(&layout), [vec![Browser]]);
        press(&mut layout, Mixer);
        assert_eq!(groups(&layout), [vec![Browser], vec![Mixer]]);
        press(&mut layout, Browser);
        assert_eq!(groups(&layout), [vec![Mixer]]);
    }
}
