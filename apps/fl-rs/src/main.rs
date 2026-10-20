//! The fl-rs window.
//!
//! `fl-rs [project.flp]` opens the project and starts playing it.
//! `FL_RS_DATA` names the directory samples are looked up in, which for the
//! projects FL ships is its `Data` directory. The browser shows it. Without
//! it samples are looked up beside the project. With `FL_RS_TRACE` set, the
//! position and the peaks go to stderr once a second while a project plays.
//!
//! `fl-rs --render project.flp out.wav` renders the song to a WAV without
//! opening a window, as the app's Render does.

use eframe::egui;
use fl_rs::App;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use ui::Command;

struct Desktop {
    app: App,
    /// The window's title as it was last set.
    title: String,
    /// When the last trace line went out, with `FL_RS_TRACE` set.
    traced: Option<Instant>,
}

impl eframe::App for Desktop {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.app.poll();
        // Closing the window is a quit, which asks first when the project
        // has changes. The window stays until the app says it may go.
        if ctx.input(|input| input.viewport().close_requested()) && !self.app.quit_requested() {
            self.app.apply(Command::Quit);
            if !self.app.quit_requested() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
        }
        // A star after the name says there are changes to save.
        let star = if self.app.is_modified() { "*" } else { "" };
        let title = match self.app.path().and_then(|path| path.file_name()) {
            Some(name) => format!("{}{star} - fl-rs", name.to_string_lossy()),
            None => "fl-rs".to_owned(),
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let state = self.app.state();
        let mut commands = Vec::new();
        ui::shell(ui, &state, &mut commands);
        for command in commands {
            self.app.apply(command);
        }
        if self.app.quit_requested() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }

        let (left, right) = state.peaks;
        // The playhead moves and the meters fall without any input.
        let loud = |(left, right): &(f32, f32)| left.max(*right) > 0.001;
        if state.playing || loud(&state.peaks) || state.insert_peaks.iter().any(loud) {
            ui.ctx().request_repaint();
        }
        if let Some(traced) = &mut self.traced
            && state.playing
            && traced.elapsed() >= Duration::from_secs(1)
        {
            *traced = Instant::now();
            eprintln!("tick {} peaks {left:.3} {right:.3}", self.app.position());
        }
    }
}

/// Render `project` to `out`, with the samples under `FL_RS_DATA` or beside
/// the project, at the rate the app renders at with no output open.
fn render(project: &Path, out: &Path) -> Result<(), String> {
    let root = std::env::var_os("FL_RS_DATA").map_or_else(
        || project.parent().unwrap_or(Path::new(".")).to_path_buf(),
        PathBuf::from,
    );
    let notice = fl_rs::render(&fl_rs::read(project)?, &root, 44_100, out)?;
    if let Some(notice) = notice {
        eprintln!("{notice}");
    }
    Ok(())
}

fn main() -> eframe::Result {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--render") {
        let [_, project, out] = args.as_slice() else {
            eprintln!("usage: fl-rs --render <project.flp> <out.wav>");
            std::process::exit(2);
        };
        if let Err(e) = render(Path::new(project), Path::new(out)) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return Ok(());
    }

    let mut app = App::new(std::env::var_os("FL_RS_DATA").map(PathBuf::from));
    let traced = std::env::var_os("FL_RS_TRACE").map(|_| Instant::now());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("fl-rs")
            .with_inner_size([1280.0, 720.0]),
        ..Default::default()
    };
    eframe::run_native(
        "fl-rs",
        options,
        Box::new(move |creation| {
            creation.egui_ctx.set_theme(egui::Theme::Dark);
            let ctx = creation.egui_ctx.clone();
            app.set_waker(move || ctx.request_repaint());
            if let Some(path) = std::env::args_os().nth(1) {
                app.apply(Command::Open(PathBuf::from(path)));
                app.apply(Command::Play);
            }
            Ok(Box::new(Desktop {
                app,
                title: "fl-rs".to_owned(),
                traced,
            }))
        }),
    )
}
