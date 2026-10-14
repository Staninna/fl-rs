//! The browser: the files fl-rs can use, as a tree under FL's data
//! directory and under the folder of the open project.
//!
//! A folder is read from disk when it is first opened, and what it held
//! then is what the tree shows from there on. Of its files the tree lists
//! the samples the engine decodes, WavPack samples, which it does not,
//! projects and presets. The search box lists what is loaded by name: it
//! reads no folder that was not opened.
//!
//! A click opens or closes a folder and selects a file. A double click
//! plays a sample or opens a project, and so does Enter for the selected
//! row, with the arrows walking the tree while the pointer is over it. A
//! sample can be dragged out, and the channel rack takes it.
//!
//! This is the one window that reads the disk. It only lists folders, one
//! per click, and what it finds goes out as a [`Command`] like everything
//! else.

use crate::{Command, UiState};
use egui::{
    Align2, Color32, Context, DragAndDrop, FontId, Id, Key, LayerId, Modifiers, Order, Pos2, Rect,
    ScrollArea, Sense, Shape, Stroke, TextEdit, Ui, WidgetInfo, WidgetType, pos2, vec2,
};
use std::path::{Path, PathBuf};

/// The height of a row.
const ROW: f32 = 18.0;
/// How far a folder's entries are set in from the folder.
const INDENT: f32 = 12.0;
/// How many seconds apart the two clicks of a double click can be. This
/// is egui's own default.
const DOUBLE_CLICK: f64 = 0.3;

/// A sample on its way from the browser to another window: the file's path.
pub(crate) struct SampleDrag(pub PathBuf);

/// What a row of the tree is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Folder,
    /// A sample the engine decodes: `.wav`, `.flac`, `.mp3` or `.ogg`.
    Sample,
    /// A `.wv` sample, which is WavPack and stays silent.
    WavPack,
    /// An `.flp`.
    Project,
    /// An `.fst`. It is listed, and nothing loads it yet.
    Preset,
}

impl Kind {
    /// What a file is, by its extension. `None` for a file the tree leaves
    /// out.
    fn of(path: &Path) -> Option<Kind> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match extension.as_str() {
            "wav" | "flac" | "mp3" | "ogg" => Kind::Sample,
            "wv" => Kind::WavPack,
            "flp" => Kind::Project,
            "fst" => Kind::Preset,
            _ => return None,
        })
    }

    /// The colour of the dot in front of a file's name.
    fn colour(self) -> Color32 {
        match self {
            Kind::Folder | Kind::WavPack => Color32::from_gray(110),
            Kind::Sample => Color32::from_rgb(120, 220, 110),
            Kind::Project => Color32::from_rgb(240, 160, 70),
            Kind::Preset => Color32::from_rgb(110, 160, 230),
        }
    }
}

#[derive(Clone)]
struct Node {
    path: PathBuf,
    name: String,
    kind: Kind,
    parent: Option<usize>,
    /// What the folder holds, once it has been opened: places in
    /// [`Tree::nodes`], folders first and each lot by name.
    children: Option<Vec<usize>>,
    open: bool,
}

/// What the browser keeps from one frame to the next.
#[derive(Clone, Default)]
struct Tree {
    /// Every folder and file read so far. Nothing is ever taken out, so a
    /// place in the list names its node for good.
    nodes: Vec<Node>,
    /// FL's data directory and the project's folder, whichever there are.
    roots: Vec<usize>,
    selected: Option<usize>,
    /// The row the last click was on and when, in egui's seconds. The
    /// next click is the second of a double click when it is on that row
    /// soon enough. egui counts clicks wherever they land, so by its count
    /// a double click right after a click on another row is a triple one.
    clicked: Option<(usize, f64)>,
    search: String,
    /// Where the browser was drawn and in which frame, for [`keys`], which
    /// runs before it.
    area: Option<(Rect, u64)>,
    /// How far the rows were scrolled and how high their view was.
    view: (f32, f32),
    /// The row to scroll into view: the one the keys selected.
    reveal: Option<usize>,
}

fn tree_id() -> Id {
    Id::new("fl-rs browser")
}

/// Take the tree out of egui's memory, work on it, and put it back. It is
/// moved and not copied: it can hold every folder of FL's sample packs.
fn with_tree<R>(ctx: &Context, work: impl FnOnce(&mut Tree) -> R) -> R {
    let mut tree: Tree = ctx
        .data_mut(|data| data.remove_temp(tree_id()))
        .unwrap_or_default();
    let result = work(&mut tree);
    ctx.data_mut(|data| data.insert_temp(tree_id(), tree));
    result
}

/// What a folder holds that the tree shows: its folders, then its files of
/// a [`Kind`], each lot by name without regard to case. A folder that
/// cannot be read holds nothing.
fn list(folder: &Path) -> Vec<(PathBuf, String, Kind)> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let entry = |entry: std::fs::DirEntry| {
        let path = entry.path();
        let kind = if path.is_dir() {
            Kind::Folder
        } else {
            Kind::of(&path)?
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        Some((path, name, kind))
    };
    let mut listed: Vec<_> = entries.filter_map(Result::ok).filter_map(entry).collect();
    listed.sort_by_cached_key(|(_, name, kind)| (*kind != Kind::Folder, name.to_lowercase()));
    listed
}

impl Tree {
    /// Make the roots what the app says they are: FL's data directory and
    /// the folder of the open project. A root that is new is opened, and
    /// one that stays keeps what was opened under it.
    fn sync(&mut self, state: &UiState) {
        let wanted = [
            ("FL Studio data", &state.data_root),
            ("Current project", &state.project_dir),
        ];
        let mut roots = Vec::new();
        for (name, path) in wanted {
            let Some(path) = path else {
                continue;
            };
            let is_root = |node: &usize| self.nodes[*node].path == *path;
            if roots.iter().any(is_root) {
                continue;
            }
            let root = self.roots.iter().copied().find(is_root).unwrap_or_else(|| {
                self.nodes.push(Node {
                    path: path.clone(),
                    name: name.to_owned(),
                    kind: Kind::Folder,
                    parent: None,
                    children: None,
                    open: false,
                });
                let root = self.nodes.len() - 1;
                self.set_open(root, true);
                root
            });
            roots.push(root);
        }
        if roots != self.roots {
            self.roots = roots;
            self.selected = None;
        }
    }

    /// Open or close a folder. One that is opened for the first time is
    /// read from disk.
    fn set_open(&mut self, folder: usize, open: bool) {
        if open && self.nodes[folder].children.is_none() {
            let first = self.nodes.len();
            for (path, name, kind) in list(&self.nodes[folder].path) {
                self.nodes.push(Node {
                    path,
                    name,
                    kind,
                    parent: Some(folder),
                    children: None,
                    open: false,
                });
            }
            self.nodes[folder].children = Some((first..self.nodes.len()).collect());
        }
        self.nodes[folder].open = open;
    }

    fn searching(&self) -> bool {
        !self.search.trim().is_empty()
    }

    /// The rows to draw, from the top: a node and how deep it is in the
    /// tree. With something in the search box they are the loaded folders
    /// and files with that in their name, whatever folder they are in, all
    /// at the same depth.
    fn rows(&self) -> Vec<(usize, usize)> {
        let query = self.search.trim().to_lowercase();
        let mut rows = Vec::new();
        let mut walk: Vec<(usize, usize)> = self.roots.iter().rev().map(|r| (*r, 0)).collect();
        while let Some((at, depth)) = walk.pop() {
            let node = &self.nodes[at];
            if query.is_empty() {
                rows.push((at, depth));
            } else if node.parent.is_some() && node.name.to_lowercase().contains(&query) {
                rows.push((at, 0));
            }
            if (node.open || !query.is_empty())
                && let Some(children) = &node.children
            {
                walk.extend(children.iter().rev().map(|child| (*child, depth + 1)));
            }
        }
        rows
    }

    /// What Enter and a double click do: open or close a folder, play a
    /// sample, open a project.
    fn activate(&mut self, at: usize, out: &mut Vec<Command>) {
        let node = &self.nodes[at];
        match node.kind {
            Kind::Folder => self.set_open(at, !node.open),
            Kind::Sample => out.push(Command::Preview(node.path.clone())),
            Kind::Project => out.push(Command::Open(node.path.clone())),
            Kind::WavPack | Kind::Preset => {}
        }
    }
}

/// The keys the browser takes when the pointer is over it: the arrows walk
/// the tree and Enter plays or opens the selected row. This runs before the
/// windows are drawn, and what it takes no other window sees.
pub(crate) fn keys(ui: &Ui, out: &mut Vec<Command>) {
    with_tree(ui.ctx(), |tree| {
        let Some((area, frame)) = tree.area else {
            return;
        };
        let pointer = ui.input(|input| input.pointer.hover_pos());
        let over = pointer.is_some_and(|pos| area.contains(pos));
        // A browser that is no longer drawn has no pointer over it.
        let drawn = frame + 1 >= ui.ctx().cumulative_frame_nr();
        if !over || !drawn || ui.memory(|egui| egui.focused().is_some()) {
            return;
        }
        let pressed = |key| ui.input_mut(|input| input.consume_key(Modifiers::NONE, key));
        let rows = tree.rows();
        let row = |tree: &Tree| {
            let selected = tree.selected?;
            rows.iter().position(|(at, _)| *at == selected)
        };
        let before = tree.selected;
        if pressed(Key::ArrowDown) && !rows.is_empty() {
            let next = row(tree).map_or(0, |row| (row + 1).min(rows.len() - 1));
            tree.selected = Some(rows[next].0);
        }
        if pressed(Key::ArrowUp) && !rows.is_empty() {
            let previous = row(tree).map_or(0, |row| row.saturating_sub(1));
            tree.selected = Some(rows[previous].0);
        }
        let folder = tree
            .selected
            .filter(|at| tree.nodes[*at].kind == Kind::Folder);
        // Search results are a flat list, with no folder to step into or
        // out of.
        let (left, right) = (pressed(Key::ArrowLeft), pressed(Key::ArrowRight));
        if !tree.searching() {
            let open = folder.is_some_and(|at| tree.nodes[at].open);
            if let Some(folder) = folder.filter(|_| right) {
                if !open {
                    tree.set_open(folder, true);
                } else if let Some(first) = tree.nodes[folder].children.as_ref() {
                    tree.selected = first.first().copied().or(tree.selected);
                }
            }
            if left {
                match folder.filter(|_| open) {
                    Some(folder) => tree.set_open(folder, false),
                    None => {
                        let parent = tree.selected.and_then(|at| tree.nodes[at].parent);
                        tree.selected = parent.or(tree.selected);
                    }
                }
            }
        }
        if pressed(Key::Enter)
            && let Some(selected) = tree.selected
        {
            tree.activate(selected, out);
        }
        if tree.selected != before {
            let rows = tree.rows();
            tree.reveal = rows.iter().position(|(at, _)| Some(*at) == tree.selected);
        }
    });
}

pub(crate) fn show(ui: &mut Ui, state: &UiState, out: &mut Vec<Command>) {
    let ctx = ui.ctx().clone();
    with_tree(&ctx, |tree| {
        tree.area = Some((ui.max_rect(), ctx.cumulative_frame_nr()));
        tree.sync(state);
        let search = TextEdit::singleline(&mut tree.search).hint_text("Search");
        ui.add(search.desired_width(f32::INFINITY));
        if tree.roots.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.weak("Open a project, or name FL's Data directory in FL_RS_DATA");
            });
            return;
        }

        let rows = tree.rows();
        if rows.is_empty() {
            ui.weak("Nothing loaded has that in its name");
            return;
        }
        let pitch = ROW + ui.spacing().item_spacing.y;
        let mut area = ScrollArea::vertical().auto_shrink(false);
        if let Some(row) = tree.reveal.take() {
            let (top, bottom) = (row as f32 * pitch, row as f32 * pitch + ROW);
            let (offset, height) = tree.view;
            if top < offset {
                area = area.vertical_scroll_offset(top);
            } else if bottom > offset + height {
                area = area.vertical_scroll_offset(bottom - height);
            }
        }
        let drawn = area.show_rows(ui, ROW, rows.len(), |ui, range| {
            for (at, depth) in &rows[range] {
                row(ui, tree, *at, *depth, out);
            }
        });
        tree.view = (drawn.state.offset.y, drawn.inner_rect.height());
    });

    // The name of a sample that is being dragged goes along with the
    // pointer, over whatever window it is on its way to.
    let pointer = ctx.input(|input| input.pointer.interact_pos());
    if let Some(sample) = DragAndDrop::payload::<SampleDrag>(&ctx)
        && let Some(pos) = pointer
    {
        let layer = LayerId::new(Order::Tooltip, tree_id().with("drag"));
        let name = sample.0.file_name().unwrap_or_default().to_string_lossy();
        ctx.layer_painter(layer).text(
            pos + vec2(12.0, 8.0),
            Align2::LEFT_TOP,
            name,
            FontId::proportional(12.0),
            ui.visuals().strong_text_color(),
        );
    }
}

/// One row: a folder with the arrow that says whether it is open, or a
/// file with the dot of its kind.
fn row(ui: &mut Ui, tree: &mut Tree, at: usize, depth: usize, out: &mut Vec<Command>) {
    let node = &tree.nodes[at];
    let (kind, selected) = (node.kind, tree.selected == Some(at));
    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), ROW));
    // Only a sample is dragged out. The row goes by its node, so that it
    // is the same widget wherever the rows above put it.
    let sense = if kind == Kind::Sample {
        Sense::click_and_drag()
    } else {
        Sense::click()
    };
    let response = ui.interact(rect, tree_id().with(at), sense);
    let role = if kind == Kind::Folder {
        WidgetType::CollapsingHeader
    } else {
        WidgetType::Button
    };
    response.widget_info(|| WidgetInfo::selected(role, true, selected, &node.name));

    if ui.is_rect_visible(rect) {
        let visuals = ui.visuals();
        let painter = ui.painter_at(rect);
        if selected {
            painter.rect_filled(rect, 2.0, visuals.selection.bg_fill);
        } else if response.hovered() {
            painter.rect_filled(rect, 2.0, visuals.widgets.hovered.weak_bg_fill);
        }
        let mark = pos2(rect.left() + 8.0 + depth as f32 * INDENT, rect.center().y);
        let ink = match kind {
            Kind::Folder => visuals.strong_text_color(),
            Kind::WavPack => visuals.weak_text_color(),
            _ => visuals.text_color(),
        };
        if kind == Kind::Folder {
            painter.add(arrow(mark, node.open, ink));
        } else {
            painter.circle_filled(mark, 2.5, kind.colour());
        }
        painter.text(
            mark + vec2(9.0, 0.0),
            Align2::LEFT_CENTER,
            &node.name,
            FontId::proportional(12.0),
            ink,
        );
    }

    if response.drag_started() {
        DragAndDrop::set_payload(ui.ctx(), SampleDrag(node.path.clone()));
    }
    let clicked = response.clicked();
    response.on_hover_ui(|ui| {
        ui.label(node.path.display().to_string());
        if kind == Kind::WavPack {
            ui.weak("WavPack, which fl-rs does not play");
        }
    });
    if !clicked {
        return;
    }
    tree.selected = Some(at);
    let now = ui.input(|input| input.time);
    let last = tree.clicked.take();
    let twice = last.is_some_and(|(row, time)| row == at && now - time <= DOUBLE_CLICK);
    if !twice {
        tree.clicked = Some((at, now));
    }
    // A folder takes every click, so two of them in a row open and close.
    if twice || kind == Kind::Folder {
        tree.activate(at, out);
    }
}

/// A triangle that points down for an open folder and right for a closed
/// one.
fn arrow(centre: Pos2, open: bool, colour: Color32) -> Shape {
    let (long, short) = (3.5, 2.0);
    let corners = if open {
        [(-long, -short), (long, -short), (0.0, long)]
    } else {
        [(-short, -long), (long, 0.0), (-short, long)]
    };
    let points = corners.map(|(x, y)| centre + vec2(x, y)).to_vec();
    Shape::convex_polygon(points, colour, Stroke::NONE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_what_its_extension_says() {
        for (name, kind) in [
            ("Kick.wav", Some(Kind::Sample)),
            ("Loop.FLAC", Some(Kind::Sample)),
            ("a.b.mp3", Some(Kind::Sample)),
            ("Hat.ogg", Some(Kind::Sample)),
            ("Pad.wv", Some(Kind::WavPack)),
            ("Song.flp", Some(Kind::Project)),
            ("Lead.fst", Some(Kind::Preset)),
            ("Notes.txt", None),
            ("wav", None),
        ] {
            assert_eq!(Kind::of(Path::new(name)), kind, "{name}");
        }
    }

    fn node(tree: &mut Tree, name: &str, kind: Kind, parent: Option<usize>) -> usize {
        tree.nodes.push(Node {
            path: PathBuf::from(name),
            name: name.to_owned(),
            kind,
            parent,
            children: None,
            open: false,
        });
        let at = tree.nodes.len() - 1;
        if let Some(parent) = parent {
            let children = tree.nodes[parent].children.get_or_insert_default();
            children.push(at);
        }
        at
    }

    /// Data with Packs, which holds Drums with a kick and a hat, and a
    /// project beside Packs.
    fn tree() -> Tree {
        let mut tree = Tree::default();
        let data = node(&mut tree, "Data", Kind::Folder, None);
        let packs = node(&mut tree, "Packs", Kind::Folder, Some(data));
        let drums = node(&mut tree, "Drums", Kind::Folder, Some(packs));
        node(&mut tree, "Kick.wav", Kind::Sample, Some(drums));
        node(&mut tree, "Hat.wav", Kind::Sample, Some(drums));
        node(&mut tree, "Kickstart.flp", Kind::Project, Some(data));
        tree.roots = vec![data];
        tree
    }

    fn names(tree: &Tree) -> Vec<(&str, usize)> {
        let rows = tree.rows().into_iter();
        rows.map(|(at, depth)| (tree.nodes[at].name.as_str(), depth))
            .collect()
    }

    #[test]
    fn the_rows_are_what_the_open_folders_hold() {
        let mut tree = tree();
        assert_eq!(names(&tree), [("Data", 0)]);
        tree.set_open(0, true);
        tree.set_open(2, true);
        // Drums is open inside Packs, which is closed.
        assert_eq!(
            names(&tree),
            [("Data", 0), ("Packs", 1), ("Kickstart.flp", 1)]
        );
        tree.set_open(1, true);
        assert_eq!(
            names(&tree),
            [
                ("Data", 0),
                ("Packs", 1),
                ("Drums", 2),
                ("Kick.wav", 3),
                ("Hat.wav", 3),
                ("Kickstart.flp", 1)
            ]
        );
    }

    #[test]
    fn a_search_lists_what_is_loaded_by_name() {
        let mut tree = tree();
        tree.search = " kick ".to_owned();
        // Closed folders are searched too, and a root is not a result.
        assert_eq!(names(&tree), [("Kick.wav", 0), ("Kickstart.flp", 0)]);
        tree.search = "DATA".to_owned();
        assert!(names(&tree).is_empty());
        tree.search = "dru".to_owned();
        assert_eq!(names(&tree), [("Drums", 0)]);
    }

    #[test]
    fn enter_plays_a_sample_and_opens_a_project() {
        let mut tree = tree();
        let mut out = Vec::new();
        tree.activate(3, &mut out);
        tree.activate(5, &mut out);
        assert_eq!(
            out,
            [
                Command::Preview(PathBuf::from("Kick.wav")),
                Command::Open(PathBuf::from("Kickstart.flp"))
            ]
        );
        tree.activate(1, &mut out);
        assert!(tree.nodes[1].open && out.len() == 2);
        tree.activate(1, &mut out);
        assert!(!tree.nodes[1].open);
    }
}
