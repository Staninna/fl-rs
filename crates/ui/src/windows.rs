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

pub(crate) fn show(ui: &mut Ui, state: &UiState, out: &mut Vec<Command>) {
    let id = Id::new("fl-rs windows");
    let mut layout: Layout<Window> = ui.data(|data| data.get_temp(id)).unwrap_or(Layout {
        root: Some(Node::Tabs {
            panels: vec![Window::Playlist],
            active: 0,
        }),
        floating: Vec::new(),
    });

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
        ui.centered_and_justified(|ui| {
            ui.weak("F5 playlist, F6 channel rack, F7 piano roll, F8 browser, F9 mixer");
        });
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
