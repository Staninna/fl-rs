//! fl-rs without its window: the project that is open, the stream that
//! plays it, and the commands that change either.
//!
//! [`App`] lives on the UI thread and never waits there. Reading a project,
//! decoding its samples or the one sample of a preview, rendering and the
//! file dialogs each run on a thread of their own and report back over a
//! channel, which [`App::poll`] empties once a frame.
//!
//! The project is never changed in place. An edit makes a changed copy the
//! open project and hands it to the transport, and the project it replaced
//! goes on the undo stack.

use engine::{Approximate, PreparedSamples, RenderOptions, Transport};
use model::{Arrangement, Channel, ChannelKind, Note, Pattern, Project};
use std::mem::{Discriminant, discriminant};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use ui::{Command, Dialog, Edit, Ticks, UiState};

/// A render stops here, in seconds, even when the song goes on.
const RENDER_LIMIT: f32 = 3600.0;
/// The rate of a render when no output stream is open to take it from.
const RENDER_RATE: u32 = 44_100;
/// What is left of a meter's peak a frame later.
const PEAK_FALL: f32 = 0.93;
/// How many steps back `Undo` goes. Each one keeps a whole project.
const UNDO_STEPS: usize = 100;

/// A project read from disk with its samples decoded.
pub struct Loaded {
    pub project: Project,
    pub samples: PreparedSamples,
    /// What is wrong with the samples, when the project is open all the
    /// same: files that are not there, or that cannot be decoded.
    pub notice: Option<String>,
}

/// Read and parse a project.
pub fn read(path: &Path) -> Result<Project, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let file = flp::Project::parse(&bytes)
        .map_err(|e| format!("{} is not a project FL wrote: {e}", path.display()))?;
    Ok(model::from_flp(&file))
}

/// Read a project and decode its samples, which are looked up under
/// `sample_root`. A project whose samples cannot be decoded still opens,
/// with those channels silent, and the notice names them.
pub fn load(path: &Path, sample_root: &Path) -> Result<Loaded, String> {
    let project = read(path)?;
    let (samples, notice) = match engine::prepare(&project, sample_root) {
        Ok(samples) => {
            let found = sample_notice(
                samples.unresolved().len(),
                samples.unsupported().len(),
                sample_root,
            );
            let notices: Vec<String> = silent_notice(&project, &samples)
                .into_iter()
                .chain(found)
                .collect();
            (samples, (!notices.is_empty()).then(|| notices.join("; ")))
        }
        Err(e) => (PreparedSamples::default(), Some(format!("no samples: {e}"))),
    };
    Ok(Loaded {
        project,
        samples,
        notice,
    })
}

/// Write a project. One that was opened and not changed comes out as the
/// bytes it was read from.
pub fn save(project: &Project, path: &Path) -> Result<(), String> {
    let file = model::to_flp(project).map_err(|e| format!("cannot save: {e}"))?;
    std::fs::write(path, file.write()).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Render the song to a stereo 32-bit float WAV, which holds the peaks
/// above full scale that the engine does not clip. `Ok` carries what is
/// wrong with the result, when something is.
pub fn render(
    project: &Project,
    sample_root: &Path,
    sample_rate: u32,
    path: &Path,
) -> Result<Option<String>, String> {
    render_with_progress(project, sample_root, sample_rate, path, &mut |_| {})
}

/// [`render`], telling `progress` how much of the song is mixed, as a
/// number from 0 to 1. Decoding the samples comes before the first call,
/// and writing the file after the last.
pub fn render_with_progress(
    project: &Project,
    sample_root: &Path,
    sample_rate: u32,
    path: &Path,
    progress: &mut dyn FnMut(f32),
) -> Result<Option<String>, String> {
    let options = RenderOptions {
        sample_rate,
        sample_root: sample_root.to_path_buf(),
        max_seconds: RENDER_LIMIT,
        plugin_dir: None,
    };
    let out = engine::render_with_progress(project, &options, &Approximate, progress)
        .map_err(|e| format!("cannot render: {e}"))?;
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let failed = |e: hound::Error| format!("cannot write {}: {e}", path.display());
    let mut writer = hound::WavWriter::create(path, spec).map_err(failed)?;
    for (left, right) in out.left.iter().zip(&out.right) {
        writer.write_sample(*left).map_err(failed)?;
        writer.write_sample(*right).map_err(failed)?;
    }
    writer.finalize().map_err(failed)?;

    let mut notices: Vec<String> =
        sample_notice(out.unresolved.len(), out.unsupported.len(), sample_root)
            .into_iter()
            .collect();
    if out.left.len() as f64 >= f64::from(RENDER_LIMIT) * f64::from(sample_rate) {
        notices.push("the render stops after an hour".to_owned());
    }
    Ok((!notices.is_empty()).then(|| notices.join("; ")))
}

/// Make an edit: on the playlist of the arrangement FL shows, on a
/// channel, on the mixer, or on the steps and the notes of `pattern`, which
/// is the pattern the top bar has selected. The records and the fields
/// change in place, so `model::to_flp` writes them back in the event they
/// came from. `data_root` is what [`stored_path`] takes for a sample.
fn apply_edit(project: &mut Project, pattern: u16, edit: &Edit, data_root: Option<&Path>) {
    let (version, ppq) = (project.version(), project.header.ppq);
    match *edit {
        Edit::SetStep { channel, step, on } => {
            // Clearing a step of a pattern the file does not have makes
            // no pattern, and neither does a step for no channel.
            let has_pattern = project.pattern(pattern).is_some();
            if project.channel(channel).is_some() && (on || has_pattern) {
                let pattern = project.pattern_to_edit(pattern);
                pattern.set_step(channel, step, on, ppq);
            }
        }
        Edit::SetStepVelocity {
            channel,
            step,
            velocity,
        } => {
            let pattern = project.patterns.iter_mut().find(|p| p.index == pattern);
            if let Some(pattern) = pattern
                && let Some(note) = pattern.step_note(channel, step, ppq)
            {
                pattern.set_note_velocity(note, velocity);
            }
        }
        Edit::AddNote {
            channel,
            position,
            length,
            key,
        } => {
            // A note for no channel makes no pattern, and neither does one
            // further along than a position goes.
            let place = i32::try_from(position).ok().zip(i32::try_from(length).ok());
            if let Some((position, length)) = place
                && project.channel(channel).is_some()
            {
                project.pattern_to_edit(pattern).add_note(Note {
                    position,
                    channel,
                    length,
                    key: i16::from(key),
                    ..Note::default()
                });
            }
        }
        Edit::AddNotes {
            channel,
            length,
            ref notes,
        } => {
            if project.channel(channel).is_some() && !notes.is_empty() {
                let pattern = project.pattern_to_edit(pattern);
                pattern.add_notes(channel, length, notes);
            }
        }
        Edit::DeleteNotes { .. }
        | Edit::SliceNotes { .. }
        | Edit::GlueNotes { .. }
        | Edit::QuantizeNotes { .. }
        | Edit::ChopNotes { .. }
        | Edit::ArpeggiateNotes { .. }
        | Edit::StrumNotes { .. }
        | Edit::SnapNotesToScale { .. }
        | Edit::MoveNotes { .. }
        | Edit::ResizeNotes { .. }
        | Edit::SetNotesMuted { .. }
        | Edit::SetNoteField { .. }
        | Edit::SelectNotes { .. } => {
            let pattern = project.patterns.iter_mut().find(|p| p.index == pattern);
            if let Some(pattern) = pattern {
                edit_notes(pattern, edit);
            }
        }
        Edit::SetChannelEnabled { channel, enabled } => {
            project.set_channel_enabled(channel, enabled);
        }
        Edit::SetChannelVolume { channel, volume } => project.set_channel_volume(channel, volume),
        Edit::SetChannelPan { channel, pan } => project.set_channel_pan(channel, pan),
        Edit::SetChannelInsert { channel, insert } => project.set_channel_insert(channel, insert),
        Edit::SetChannelSample { channel, ref path } => {
            project.set_channel_sample(channel, &stored_path(path, data_root));
        }
        Edit::AddSamplerChannel { ref path } => {
            project.add_sampler_channel(&stored_path(path, data_root));
        }
        Edit::SetInsertVolume { insert, volume } => project.set_insert_volume_raw(insert, volume),
        Edit::SetInsertPan { insert, pan } => project.set_insert_pan_raw(insert, pan),
        Edit::SetInsertEnabled { insert, enabled } => project.set_insert_enabled(insert, enabled),
        Edit::SetInsertName { insert, ref name } => project.set_insert_name(insert, name),
        Edit::SetRoute { from, to, on } => project.set_route(from, to, on),
        Edit::SetSendLevel { from, to, level } => project.set_send_level_raw(from, to, level),
        Edit::SetSlotEnabled {
            insert,
            slot,
            enabled,
        } => project.set_slot_enabled(insert, slot, enabled),
        Edit::SetSlotMix { insert, slot, mix } => project.set_slot_mix_raw(insert, slot, mix),
        Edit::MoveItems { .. }
        | Edit::ResizeItems { .. }
        | Edit::SetMuted { .. }
        | Edit::DeleteItems { .. }
        | Edit::DuplicateItems { .. }
        | Edit::SelectItems { .. } => {
            if let Some(playlist) = project.current_arrangement_mut() {
                edit_playlist(playlist, version, edit);
            }
        }
    }
}

/// The playlist's half of [`apply_edit`]. `version` is what
/// [`Arrangement::move_items`] takes.
fn edit_playlist(playlist: &mut Arrangement, version: u32, edit: &Edit) {
    match edit {
        Edit::MoveItems {
            ids,
            delta_ticks,
            delta_tracks,
        } => playlist.move_items(ids, *delta_ticks, *delta_tracks, version),
        Edit::ResizeItems { ids, new_length } => {
            playlist.resize_items(ids, i32::try_from(*new_length).unwrap_or(i32::MAX));
        }
        Edit::SetMuted { ids, muted } => playlist.mute_items(ids, *muted),
        Edit::DeleteItems { ids } => playlist.delete_items(ids),
        Edit::DuplicateItems { ids } => playlist.duplicate_items(ids),
        Edit::SelectItems { ids, add } => playlist.select_items(ids, *add),
        _ => {}
    }
}

/// The piano roll's half of [`apply_edit`], for the notes a pattern has.
fn edit_notes(pattern: &mut Pattern, edit: &Edit) {
    match edit {
        Edit::DeleteNotes { ids } => pattern.delete_notes(ids),
        Edit::MoveNotes {
            ids,
            delta_ticks,
            delta_keys,
        } => pattern.move_notes(ids, *delta_ticks, *delta_keys),
        Edit::ResizeNotes { ids, delta_ticks } => pattern.resize_notes(ids, *delta_ticks),
        Edit::SetNotesMuted { ids, muted } => pattern.set_notes_muted(ids, *muted),
        Edit::SetNoteField { id, field, value } => pattern.set_note_field(*id, *field, *value),
        Edit::SelectNotes { ids, add } => pattern.select_notes(ids, *add),
        Edit::SliceNotes { ids, at } => pattern.slice_notes(ids, *at),
        Edit::GlueNotes { ids } => pattern.glue_notes(ids),
        Edit::QuantizeNotes { ids, step, lengths } => pattern.quantize_notes(ids, *step, *lengths),
        Edit::ChopNotes { ids, step } => pattern.chop_notes(ids, *step),
        Edit::ArpeggiateNotes { ids, step } => pattern.arpeggiate_notes(ids, *step),
        Edit::StrumNotes { ids, delay } => pattern.strum_notes(ids, *delay),
        Edit::SnapNotesToScale { ids, keys } => pattern.snap_notes_to_scale(ids, *keys),
        _ => {}
    }
}

/// The channels whose sample file is there and did not decode, by name.
fn silent_notice(project: &Project, samples: &PreparedSamples) -> Option<String> {
    let name = |(index, _): &(usize, engine::Error)| {
        let channel = u16::try_from(*index).ok();
        let channel = channel.and_then(|index| project.channel(index));
        let name = channel.and_then(|channel| channel.name.clone());
        name.unwrap_or_else(|| format!("Channel {index}"))
    };
    let names: Vec<String> = samples.failed.iter().map(name).collect();
    (!names.is_empty()).then(|| format!("silent, the sample does not decode: {}", names.join(", ")))
}

fn sample_notice(unresolved: usize, unsupported: usize, root: &Path) -> Option<String> {
    let mut parts = Vec::new();
    if unresolved > 0 {
        parts.push(format!(
            "{unresolved} samples not found under {} (FL_RS_DATA names FL's Data directory)",
            root.display()
        ));
    }
    if unsupported > 0 {
        parts.push(format!("{unsupported} WavPack samples not played"));
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// The path a channel's 196 gets for a sample file.
///
/// A file under `data_root` is written as FL writes the samples it ships:
/// from `%FLStudioFactoryData%`, which stands for FL's install directory.
/// The root is that directory or the `Data` directory in it, and one named
/// `Data` is taken for the second. Any other file gets its absolute path.
pub fn stored_path(file: &Path, data_root: Option<&Path>) -> String {
    let under = data_root.and_then(|root| Some((root, file.strip_prefix(root).ok()?)));
    let Some((root, rest)) = under else {
        let absolute = std::path::absolute(file).unwrap_or_else(|_| file.to_path_buf());
        return absolute.to_string_lossy().into_owned();
    };
    let mut parts = vec!["%FLStudioFactoryData%".to_owned()];
    let named_data = |name: &std::ffi::OsStr| name.eq_ignore_ascii_case("Data");
    if root.file_name().is_some_and(named_data) {
        parts.push("Data".to_owned());
    }
    let rest = rest.components();
    parts.extend(rest.map(|part| part.as_os_str().to_string_lossy().into_owned()));
    parts.join("\\")
}

/// A project of one sampler channel that plays `sample`, a path as a 196
/// stores it, once from the first tick and to the sample's end. A preview
/// plays this, and it decodes the sample of a channel that just got one.
fn one_shot(sample: &str) -> Project {
    let mut project = Project::new(flp::Header {
        format: 0,
        channel_count: 1,
        ppq: 96,
    });
    project.tempo_milli = Some(120_000);
    project.channels.push(Channel {
        kind: Some(ChannelKind::Sampler),
        sample_path: Some(sample.to_owned()),
        ..Channel::default()
    });
    // A step has no end, so the sample plays out.
    project.patterns.push(Pattern {
        index: 1,
        notes: vec![Note {
            key: 60,
            ..Note::default()
        }],
        ..Pattern::default()
    });
    project
}

/// Decode one sample, looked up under `root` as [`load`] looks them up.
/// `Err` says why the sample will not sound.
fn decode(sample: &str, root: &Path) -> Result<PreparedSamples, String> {
    let samples = engine::prepare(&one_shot(sample), root).map_err(|e| e.to_string())?;
    if let Some((_, error)) = samples.failed.first() {
        return Err(error.to_string());
    }
    if !samples.unsupported().is_empty() {
        return Err(format!("{sample} is WavPack, which is not played"));
    }
    if !samples.unresolved().is_empty() {
        return Err(format!("{sample} was not found"));
    }
    Ok(samples)
}

/// What waits for the user's answer while the project has changes that
/// are not saved.
#[derive(Clone)]
enum After {
    Quit,
    Open(PathBuf),
}

/// What a worker thread reports when it is done.
enum Done {
    Loaded {
        /// Which load this is, see [`App::load`].
        load: u64,
        path: PathBuf,
        result: Result<Box<Loaded>, String>,
    },
    Rendered(Result<Option<String>, String>),
    Chosen(Dialog, Option<PathBuf>),
    /// What the user said to a quit or an open with changes: save them
    /// first, or with `false` drop them. `None` when the project is to
    /// stay as it is.
    Answered(After, Option<bool>),
    /// The sample of a channel that got it after the stream opened.
    Decoded(Result<PreparedSamples, String>),
    Previewed {
        /// Which preview this is, see [`App::previews`].
        preview: u64,
        /// The sample's path, as [`one_shot`] takes it.
        sample: String,
        result: Result<PreparedSamples, String>,
    },
}

/// A drag whose edits make one undo step.
struct Drag {
    id: u64,
    /// The project the drag's last edit was applied to. The next edit of
    /// the same kind is applied to it again, in that edit's place, when it
    /// is one that [`Edit::replaces_last`].
    base: Arc<Project>,
    kind: Discriminant<Edit>,
}

type Waker = Arc<dyn Fn() + Send + Sync>;

/// The open project and its playback.
pub struct App {
    project: Arc<Project>,
    /// The project as its file has it: the one that was opened or last
    /// saved. The open project has changes to save when it is another.
    saved: Arc<Project>,
    /// The projects the edits replaced, the latest last.
    undo: Vec<Arc<Project>>,
    /// The projects `Undo` replaced, until the next edit.
    redo: Vec<Arc<Project>>,
    /// The drag the last edit was part of.
    drag: Option<Drag>,
    /// The file the project came from. `None` until one is open.
    path: Option<PathBuf>,
    /// `None` until a project is open, and when no stream could be opened.
    transport: Option<Transport>,
    /// The stream of the sample that was previewed last. It is a stream of
    /// its own, so the project's clock and its voices stay as they are.
    preview: Option<Transport>,
    /// Counts the previews started. One that lands with another number was
    /// overtaken by a later one and is dropped.
    previews: u64,
    /// Where samples are looked up. `None` for the project's own directory.
    sample_root: Option<PathBuf>,
    song_mode: bool,
    /// Whether the song starts over when it ends.
    looping: bool,
    /// The pattern that plays in pattern mode, by its index.
    pattern: u16,
    /// The ticks that pattern plays round, when not the whole of it.
    loop_range: Option<(Ticks, Ticks)>,
    peaks: (f32, f32),
    /// The peak of every insert, by the loader's number. Empty while no
    /// stream is open.
    insert_peaks: Vec<(f32, f32)>,
    error: Option<String>,
    /// Counts the loads started. A load that lands with another number was
    /// overtaken by a later `Open` and is dropped.
    load: u64,
    loading: Option<String>,
    rendering: Option<String>,
    /// How much of the render is mixed, 0 to 1, as the bits of an `f32`.
    render_progress: Arc<AtomicU32>,
    /// A file dialog is up.
    choosing: bool,
    /// `Play` arrived while a load was running.
    play_when_loaded: bool,
    quit: bool,
    sender: Sender<Done>,
    receiver: Receiver<Done>,
    waker: Waker,
}

impl App {
    /// An app with nothing open. `sample_root` is FL's `Data` directory or
    /// its install directory, and `None` looks samples up beside each
    /// project.
    pub fn new(sample_root: Option<PathBuf>) -> Self {
        let (sender, receiver) = channel();
        let project = Arc::new(Project::new(flp::Header {
            format: 0,
            channel_count: 0,
            ppq: 96,
        }));
        App {
            saved: Arc::clone(&project),
            project,
            undo: Vec::new(),
            redo: Vec::new(),
            drag: None,
            path: None,
            transport: None,
            preview: None,
            previews: 0,
            sample_root,
            // The engine plays the song, so that is the mode to start in.
            song_mode: true,
            looping: false,
            pattern: 1,
            loop_range: None,
            peaks: (0.0, 0.0),
            insert_peaks: Vec::new(),
            error: None,
            load: 0,
            loading: None,
            rendering: None,
            render_progress: Arc::default(),
            choosing: false,
            play_when_loaded: false,
            quit: false,
            sender,
            receiver,
            waker: Arc::new(|| {}),
        }
    }

    /// Have the workers call this when they have something for
    /// [`App::poll`], so that an idle window wakes up for it.
    pub fn set_waker(&mut self, waker: impl Fn() + Send + Sync + 'static) {
        self.waker = Arc::new(waker);
    }

    pub fn project(&self) -> &Arc<Project> {
        &self.project
    }

    /// The file the open project came from, or was last saved to.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Where the transport's clock is.
    pub fn position(&self) -> Ticks {
        self.transport.as_ref().map_or(0, Transport::position)
    }

    /// Whether an output stream is open.
    pub fn has_audio(&self) -> bool {
        self.transport.is_some()
    }

    /// Whether the clock runs. It stops by itself at the end of the song,
    /// unless the song loops or a pattern plays on its own.
    pub fn is_playing(&self) -> bool {
        self.transport.as_ref().is_some_and(Transport::is_playing)
    }

    /// Whether a previewed sample plays. The stream of one that has played
    /// stays open, silent, until the next preview or a `Stop`.
    pub fn is_previewing(&self) -> bool {
        self.preview.as_ref().is_some_and(Transport::is_playing)
    }

    /// Whether a load or a render is running.
    pub fn is_busy(&self) -> bool {
        self.loading.is_some() || self.rendering.is_some()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Whether the project has changes its file does not.
    pub fn is_modified(&self) -> bool {
        !Arc::ptr_eq(&self.project, &self.saved)
    }

    /// Whether the app is to close: a `Quit` came in, and the user did not
    /// turn it down for the changes there are.
    pub fn quit_requested(&self) -> bool {
        self.quit
    }

    /// What the window draws this frame. Call it once a frame: it takes the
    /// peaks from the transport, which then start again from silence.
    pub fn state(&mut self) -> UiState {
        let fall = |held: f32, peak: f32| peak.max(held * PEAK_FALL);
        let (left, right) = self.transport.as_ref().map_or((0.0, 0.0), Transport::peak);
        self.peaks = (fall(self.peaks.0, left), fall(self.peaks.1, right));
        if let Some(transport) = &self.transport {
            let inserts = usize::from(model::rec::LAST_INSERT) + 1;
            self.insert_peaks.resize(inserts, (0.0, 0.0));
            for (insert, held) in self.insert_peaks.iter_mut().enumerate() {
                let (left, right) = transport.insert_peak(insert as u16);
                *held = (fall(held.0, left), fall(held.1, right));
            }
        }
        UiState {
            project: Arc::clone(&self.project),
            position: self.position(),
            peaks: self.peaks,
            insert_peaks: self.insert_peaks.clone(),
            playing: self.is_playing(),
            song_mode: self.song_mode,
            looping: self.looping,
            pattern: self.pattern,
            loop_range: self.loop_range,
            data_root: self.sample_root.clone(),
            project_dir: self.path.as_deref().and_then(Path::parent).map(|folder| {
                // A file named without a folder is in the current one.
                let here = folder.as_os_str().is_empty();
                if here { Path::new(".") } else { folder }.to_path_buf()
            }),
            sample_rate: self.transport.as_ref().map(Transport::sample_rate),
            last_error: self.error.clone(),
            busy: self.rendering.clone().or(self.loading.clone()),
            progress: self.rendering.as_ref().map(|_| {
                let done = self.render_progress.load(Ordering::Relaxed);
                f32::from_bits(done)
            }),
        }
    }

    /// Do what the user asked for. Whatever goes wrong ends up in
    /// [`App::error`].
    pub fn apply(&mut self, command: Command) {
        let result = match command {
            Command::Open(path) => self.leave(After::Open(path)),
            Command::Save => match self.open_path() {
                Ok(path) => self.save_as(path.to_path_buf()),
                Err(error) => Err(error),
            },
            Command::SaveAs(path) => self.save_as(path),
            Command::Render(path) => self.render(path),
            Command::Choose(dialog) => self.choose(dialog),
            Command::Play => self.play(),
            Command::Stop => self.stop(),
            Command::Seek(ticks) => self.seek(ticks),
            Command::SetLoopRange(range) => self.set_loop_range(range),
            Command::NoteOn {
                channel,
                key,
                velocity,
            } => self.note(|transport| transport.note_on(channel, key, velocity)),
            Command::NoteOff { key } => self.note(|transport| transport.note_off(key)),
            Command::ToggleSongMode => self.set_mode(!self.song_mode, self.looping, self.pattern),
            Command::ToggleLoop => self.set_mode(self.song_mode, !self.looping, self.pattern),
            Command::SelectPattern(index) => self.set_mode(self.song_mode, self.looping, index),
            Command::Preview(path) => {
                self.preview(&path);
                Ok(())
            }
            Command::Edit { edit, drag_id } => self.edit(&edit, drag_id),
            Command::Undo => self.undo(),
            Command::Redo => self.redo(),
            Command::Quit => self.leave(After::Quit),
        };
        if let Err(error) = result {
            self.error = Some(error);
        }
    }

    /// Take what the workers have finished.
    pub fn poll(&mut self) {
        while let Ok(done) = self.receiver.try_recv() {
            match done {
                Done::Loaded { load, .. } if load != self.load => {}
                Done::Loaded { path, result, .. } => {
                    self.loading = None;
                    match result {
                        Ok(loaded) => self.show(path, *loaded),
                        Err(error) => {
                            self.play_when_loaded = false;
                            self.error = Some(error);
                        }
                    }
                }
                Done::Rendered(result) => {
                    self.rendering = None;
                    self.error = result.unwrap_or_else(Some);
                }
                Done::Chosen(dialog, path) => {
                    self.choosing = false;
                    if let Some(path) = path {
                        self.apply(match dialog {
                            Dialog::Open => Command::Open(path),
                            Dialog::SaveAs => Command::SaveAs(path),
                            Dialog::Render => Command::Render(path),
                        });
                    }
                }
                Done::Answered(after, answer) => {
                    self.choosing = false;
                    if answer == Some(true) {
                        self.apply(Command::Save);
                    }
                    // A save that failed leaves the changes, and the
                    // project open with the error.
                    if answer.is_some_and(|save| !save || !self.is_modified()) {
                        self.go(after);
                    }
                }
                Done::Decoded(result) => {
                    let added = result.and_then(|samples| match self.transport.as_mut() {
                        Some(transport) => transport
                            .add_samples(samples)
                            .map_err(|e| format!("the sample does not play: {e}")),
                        None => Ok(()),
                    });
                    self.error = added.err().or(self.error.take());
                }
                Done::Previewed { preview, .. } if preview != self.previews => {}
                Done::Previewed { sample, result, .. } => {
                    let played = result.and_then(|samples| self.play_preview(&sample, samples));
                    self.error = played.err().or(self.error.take());
                }
            }
        }
    }

    /// Run `work` on a thread of its own and send what it returns, or what
    /// `panicked` makes of it when it panics, so that the app never waits
    /// for an answer that will not come.
    fn spawn(
        &self,
        work: impl FnOnce() -> Done + Send + 'static,
        panicked: impl FnOnce() -> Done + Send + 'static,
    ) {
        let (sender, waker) = (self.sender.clone(), Arc::clone(&self.waker));
        std::thread::spawn(move || {
            let done = catch_unwind(AssertUnwindSafe(work)).unwrap_or_else(|_| panicked());
            // The app may be gone by now.
            let _ = sender.send(done);
            waker();
        });
    }

    /// Where the samples of the project at `path` are looked up.
    fn sample_root_for(&self, path: &Path) -> PathBuf {
        let beside = || path.parent().unwrap_or(Path::new(".")).to_path_buf();
        self.sample_root.clone().unwrap_or_else(beside)
    }

    fn open(&mut self, path: PathBuf) {
        self.error = None;
        self.load += 1;
        self.loading = Some(format!("Opening {}", name(&path)));
        let (load, root) = (self.load, self.sample_root_for(&path));
        let failed = path.clone();
        self.spawn(
            move || Done::Loaded {
                load,
                result: crate::load(&path, &root).map(Box::new),
                path,
            },
            move || Done::Loaded {
                load,
                result: Err(format!("opening {} failed", name(&failed))),
                path: failed,
            },
        );
    }

    /// Make a loaded project the open one and open a stream for it.
    fn show(&mut self, path: PathBuf, loaded: Loaded) {
        let project = Arc::new(loaded.project);
        // The old stream closes before the new one opens.
        self.transport = None;
        self.peaks = (0.0, 0.0);
        self.insert_peaks.clear();
        let mut notices: Vec<String> = loaded.notice.into_iter().collect();
        match Transport::open(Arc::clone(&project), loaded.samples, None) {
            Ok(transport) => self.transport = Some(transport),
            Err(e) => notices.push(format!("no sound: {e}")),
        }
        // A stream opens in song mode on pattern 1 and does not loop. The
        // mode and the loop stay what the user chose, and the pattern goes
        // back to 1, the whole of it.
        self.loop_range = None;
        notices.extend(self.set_mode(self.song_mode, self.looping, 1).err());
        self.saved = Arc::clone(&project);
        self.project = project;
        self.undo.clear();
        self.redo.clear();
        self.drag = None;
        self.path = Some(path);
        self.error = (!notices.is_empty()).then(|| notices.join("; "));
        if std::mem::take(&mut self.play_when_loaded) && self.transport.is_some() {
            self.apply(Command::Play);
        }
    }

    fn open_path(&self) -> Result<&Path, String> {
        self.path
            .as_deref()
            .ok_or_else(|| "no project is open".to_owned())
    }

    fn save_as(&mut self, path: PathBuf) -> Result<(), String> {
        self.open_path()?;
        self.error = None;
        save(&self.project, &path)?;
        self.saved = Arc::clone(&self.project);
        self.path = Some(path);
        Ok(())
    }

    /// Make a changed copy of the project the open one.
    ///
    /// An edit is one undo step, and so is a drag: its first edit makes the
    /// step and the ones after it change what the step leads to. A move or
    /// a resize says how far the drag is from where it began, so one of the
    /// kind the drag sent last is applied to the project that one was
    /// applied to, and takes its place, and so is the note a drag draws.
    /// Any other edit of a drag sets a value, and is applied on top of the
    /// ones before it: a knob's turn by turn, and a drag across steps step
    /// by step.
    ///
    /// A selection is no step, and nothing to save or to play.
    fn edit(&mut self, edit: &Edit, drag_id: Option<u64>) -> Result<(), String> {
        self.open_path()?;
        if matches!(edit, Edit::SelectItems { .. } | Edit::SelectNotes { .. }) {
            self.drag = None;
            let mut project = (*self.project).clone();
            apply_edit(&mut project, self.pattern, edit, None);
            let project = Arc::new(project);
            if !self.is_modified() {
                self.saved = Arc::clone(&project);
            }
            self.project = project;
            return Ok(());
        }

        let kind = discriminant(edit);
        let drag = self.drag.take().filter(|drag| Some(drag.id) == drag_id);
        let continues = drag.is_some();
        let base = match drag {
            Some(drag) if drag.kind == kind && edit.replaces_last() => drag.base,
            _ => Arc::clone(&self.project),
        };
        let mut project = (*base).clone();
        let data_root = self.sample_root.as_deref();
        apply_edit(&mut project, self.pattern, edit, data_root);
        if !continues {
            if project == *base {
                return Ok(());
            }
            self.undo.push(Arc::clone(&self.project));
            if self.undo.len() > UNDO_STEPS {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
        self.drag = drag_id.map(|id| Drag { id, base, kind });
        let shown = self.show_edited(Arc::new(project));
        // The stream has the samples the project was opened with, and this
        // one is new to it.
        if let Edit::SetChannelSample { path, .. } | Edit::AddSamplerChannel { path } = edit {
            self.decode_for_stream(stored_path(path, self.sample_root.as_deref()));
        }
        shown
    }

    /// Decode a sample for the stream to play, on a thread of its own.
    fn decode_for_stream(&self, sample: String) {
        let Some(path) = self.path.as_deref().filter(|_| self.transport.is_some()) else {
            return;
        };
        let root = self.sample_root_for(path);
        self.spawn(
            move || Done::Decoded(decode(&sample, &root)),
            || Done::Decoded(Err("decoding the sample failed".to_owned())),
        );
    }

    /// Decode a sample file on a thread of its own, to play it when it
    /// lands.
    fn preview(&mut self, file: &Path) {
        self.previews += 1;
        let preview = self.previews;
        let sample = stored_path(file, None);
        let failed = sample.clone();
        self.spawn(
            move || Done::Previewed {
                preview,
                // The path is absolute, so no root is looked into.
                result: decode(&sample, Path::new("")),
                sample,
            },
            move || Done::Previewed {
                preview,
                result: Err(format!("decoding {failed} failed")),
                sample: failed,
            },
        );
    }

    /// Play a decoded sample on a stream of its own, at the rate of the
    /// project's stream when there is one.
    fn play_preview(&mut self, sample: &str, samples: PreparedSamples) -> Result<(), String> {
        // The stream of the last preview closes before this one opens.
        self.preview = None;
        let rate = self.transport.as_ref().map(Transport::sample_rate);
        let mut transport = Transport::open(Arc::new(one_shot(sample)), samples, rate)
            .map_err(|e| format!("no sound: {e}"))?;
        transport.play().map_err(|e| e.to_string())?;
        self.preview = Some(transport);
        Ok(())
    }

    fn undo(&mut self) -> Result<(), String> {
        self.drag = None;
        let Some(before) = self.undo.pop() else {
            return Ok(());
        };
        self.redo.push(Arc::clone(&self.project));
        self.show_edited(before)
    }

    fn redo(&mut self) -> Result<(), String> {
        self.drag = None;
        let Some(after) = self.redo.pop() else {
            return Ok(());
        };
        self.undo.push(Arc::clone(&self.project));
        self.show_edited(after)
    }

    /// Make an edited project the open one and play it from where the
    /// clock is.
    fn show_edited(&mut self, project: Arc<Project>) -> Result<(), String> {
        self.project = Arc::clone(&project);
        match self.transport.as_mut() {
            Some(transport) => transport
                .set_project(project)
                .map_err(|e| format!("the change does not play: {e}")),
            None => Ok(()),
        }
    }

    /// Quit or open another project, or with changes that are not saved,
    /// ask first. The answer comes back through [`App::poll`].
    fn leave(&mut self, after: After) -> Result<(), String> {
        if !self.is_modified() {
            self.go(after);
            return Ok(());
        }
        if self.choosing {
            return Ok(());
        }
        let file = name(self.open_path()?);
        let what = match &after {
            After::Quit => "quitting".to_owned(),
            After::Open(path) => format!("opening {}", name(path)),
        };
        let unanswered = after.clone();
        self.choosing = true;
        self.spawn(
            move || {
                let answer = rfd::MessageDialog::new()
                    .set_level(rfd::MessageLevel::Warning)
                    .set_title("fl-rs")
                    .set_description(format!("Save the changes to {file} before {what}?"))
                    .set_buttons(rfd::MessageButtons::YesNoCancel)
                    .show();
                let answer = match answer {
                    rfd::MessageDialogResult::Yes => Some(true),
                    rfd::MessageDialogResult::No => Some(false),
                    _ => None,
                };
                Done::Answered(after, answer)
            },
            || Done::Answered(unanswered, None),
        );
        Ok(())
    }

    /// Do what [`App::leave`] was asked for, with nothing left to save.
    fn go(&mut self, after: After) {
        match after {
            After::Quit => self.quit = true,
            After::Open(path) => self.open(path),
        }
    }

    /// Song mode or pattern mode, whether the song loops, and the pattern
    /// that plays in pattern mode. What the stream turns down stays as it
    /// was.
    fn set_mode(&mut self, song: bool, looping: bool, pattern: u16) -> Result<(), String> {
        if let Some(transport) = self.transport.as_mut() {
            let failed = |e: engine::Error| e.to_string();
            transport.set_pattern(pattern).map_err(failed)?;
            transport.set_song_mode(song).map_err(failed)?;
            transport.set_loop(looping).map_err(failed)?;
        }
        // A range is the pattern's it was set on.
        if pattern != self.pattern && self.loop_range.is_some() {
            self.set_loop_range(None)?;
        }
        (self.song_mode, self.looping, self.pattern) = (song, looping, pattern);
        Ok(())
    }

    /// The ticks the pattern plays round in pattern mode, or with `None`
    /// the whole of it. A range with no ticks is `None`.
    fn set_loop_range(&mut self, range: Option<(Ticks, Ticks)>) -> Result<(), String> {
        let range = range.filter(|(from, to)| from < to);
        if let Some(transport) = self.transport.as_mut() {
            transport.set_range(range).map_err(|e| e.to_string())?;
        }
        self.loop_range = range;
        Ok(())
    }

    /// Send a key of the keyboard to the stream, when there is one.
    fn note(
        &mut self,
        send: impl FnOnce(&mut Transport) -> Result<(), engine::Error>,
    ) -> Result<(), String> {
        match self.transport.as_mut() {
            Some(transport) => send(transport).map_err(|e| format!("the note does not play: {e}")),
            None => Ok(()),
        }
    }

    fn render(&mut self, path: PathBuf) -> Result<(), String> {
        let root = self.sample_root_for(self.open_path()?);
        if self.rendering.is_some() {
            return Err("a render is running already".to_owned());
        }
        self.error = None;
        self.rendering = Some(format!("Rendering {}", name(&path)));
        let project = Arc::clone(&self.project);
        let rate = self
            .transport
            .as_ref()
            .map_or(RENDER_RATE, Transport::sample_rate);
        let (progress, waker) = (Arc::clone(&self.render_progress), Arc::clone(&self.waker));
        progress.store(0f32.to_bits(), Ordering::Relaxed);
        self.spawn(
            move || {
                let mut report = |done: f32| {
                    progress.store(done.to_bits(), Ordering::Relaxed);
                    waker();
                };
                let rendered = render_with_progress(&project, &root, rate, &path, &mut report);
                Done::Rendered(rendered)
            },
            || Done::Rendered(Err("the render failed".to_owned())),
        );
        Ok(())
    }

    /// Show a file dialog on a thread of its own. Its answer comes back
    /// as the command the dialog stands for.
    fn choose(&mut self, dialog: Dialog) -> Result<(), String> {
        if self.choosing {
            return Ok(());
        }
        let mut ask = rfd::FileDialog::new();
        if dialog != Dialog::Open {
            let stem = self.open_path()?.file_stem().unwrap_or_default();
            let extension = if dialog == Dialog::Render {
                "wav"
            } else {
                "flp"
            };
            ask = ask.set_file_name(format!("{}.{extension}", stem.to_string_lossy()));
        }
        if let Some(folder) = self.path.as_deref().and_then(Path::parent) {
            ask = ask.set_directory(folder);
        }
        self.choosing = true;
        self.spawn(
            move || {
                let path = match dialog {
                    Dialog::Open => ask.add_filter("FL Studio project", &["flp"]).pick_file(),
                    Dialog::SaveAs => ask.add_filter("FL Studio project", &["flp"]).save_file(),
                    Dialog::Render => ask.add_filter("WAV audio", &["wav"]).save_file(),
                };
                Done::Chosen(dialog, path)
            },
            move || Done::Chosen(dialog, None),
        );
        Ok(())
    }

    fn play(&mut self) -> Result<(), String> {
        if self.loading.is_some() {
            self.play_when_loaded = true;
            return Ok(());
        }
        self.open_path()?;
        let transport = self.transport.as_mut().ok_or("there is no audio stream")?;
        transport.play().map_err(|e| e.to_string())
    }

    fn stop(&mut self) -> Result<(), String> {
        self.play_when_loaded = false;
        self.preview = None;
        let Some(transport) = self.transport.as_mut() else {
            return Ok(());
        };
        transport.stop().map_err(|e| e.to_string())?;
        transport.seek(0).map_err(|e| e.to_string())
    }

    fn seek(&mut self, ticks: Ticks) -> Result<(), String> {
        match self.transport.as_mut() {
            Some(transport) => transport.seek(ticks).map_err(|e| e.to_string()),
            None => Ok(()),
        }
    }
}

/// A path's file name, for messages.
fn name(path: &Path) -> String {
    let name = path.file_name().unwrap_or(path.as_os_str());
    name.to_string_lossy().into_owned()
}
