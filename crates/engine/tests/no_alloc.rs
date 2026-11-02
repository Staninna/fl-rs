//! [`Engine::process`] allocates nothing once the engine is built and has
//! played its first block.
//!
//! The project is built in code, as `fixtures.rs` builds its own: a
//! sampler channel playing a click into insert 1, which holds Fruity Fast
//! Dist, Fruity Delay 2 and Fruity Peak Controller, and TestSine on a
//! second channel into the master. The song loops, so the clock goes back
//! and the voices start and end while it is counted.
//!
//! The engine runs inserts on a pool of worker threads, so the count here
//! is for the whole process while the engine plays, and not one thread's.

use engine::Engine;
use flp::event::Id;
use flp::event::layout::{InsertFlags, Levels, PluginWrapper};
use model::{
    Arrangement, Channel, ChannelKind, Insert, Item, ItemSource, Note, Pattern, Placed,
    PlaylistItem, Project, Slot,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// The system allocator, counting what it is asked for while the engine
/// plays.
struct Counting;

static INSIDE: AtomicBool = AtomicBool::new(false);
static COUNT: AtomicUsize = AtomicUsize::new(0);
/// With `NO_ALLOC_TRACE` set, the first allocation prints where it came
/// from.
static TRACE: AtomicBool = AtomicBool::new(false);

fn note_allocation() {
    if INSIDE.load(Ordering::Relaxed) {
        let first = COUNT.fetch_add(1, Ordering::Relaxed) == 0;
        if first && TRACE.load(Ordering::Relaxed) {
            // Printing allocates: stop counting while it does.
            INSIDE.store(false, Ordering::SeqCst);
            eprintln!("{}", std::backtrace::Backtrace::force_capture());
            INSIDE.store(true, Ordering::SeqCst);
        }
    }
}

// A global allocator is unsafe to write; this one only hands on to the
// system's.
#[allow(unsafe_code)]
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_allocation();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_allocation();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_allocation();
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const RATE: u32 = 48_000;
const BEAT: i32 = 96;
const BAR: i32 = 4 * BEAT;

/// A directory under the system's temp directory with `click.wav` in it,
/// removed on drop.
struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        let name = format!("fl-rs-engine-{}-no-alloc", std::process::id());
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: RATE,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(dir.join("click.wav"), spec).unwrap();
        for frame in 0..2048 {
            writer
                .write_sample(if frame == 0 { 1f32 } else { 0.0 })
                .unwrap();
        }
        writer.finalize().unwrap();
        Dir(dir)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Events in the order FL writes them, by the number each stands at.
fn placed(pos: usize, id: Id) -> Placed {
    Placed {
        pos,
        item: Item::Field(id),
    }
}

/// The 213 of a plugin whose state is its parameters.
fn params(values: &[i32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn wrapper(insert: i32, slot: i32, plugin_kind: i32, flags: u32) -> PluginWrapper {
    PluginWrapper {
        insert,
        slot,
        plugin_kind,
        unknown_12: [0; 4],
        flags,
        page: 0,
        unknown_21: [0; 15],
        editor_position: 0,
        editor_size: 0,
        len: 52,
    }
}

fn song() -> Project {
    let mut project = Project::new(flp::Header {
        format: 0,
        channel_count: 2,
        ppq: BEAT as u16,
    });
    project.writer_version = Some("20.8.4.2576".to_owned());
    project.tempo_milli = Some(120_000);
    project.order = vec![placed(0, Id::FLVersion), placed(1, Id::Tempo)];

    // Clicks on every eighth, and sine notes that overlap and run past
    // the end of the bar.
    let mut notes: Vec<Note> = (0..8)
        .map(|n| Note {
            position: n * BEAT / 2,
            key: 60,
            ..Note::default()
        })
        .collect();
    notes.extend((0..6).map(|n| Note {
        position: n * BEAT / 2 + 3,
        length: BEAT,
        key: 57 + 5 * n as i16,
        channel: 1,
        ..Note::default()
    }));
    project.patterns.push(Pattern {
        index: 1,
        notes,
        order: vec![placed(2, Id::PatternNew), placed(3, Id::PatternNotes)],
        ..Pattern::default()
    });

    let levels = Levels {
        pan: 6400,
        volume: 10_000,
        unknown_8: [0; 4],
        mod_x: 0,
        mod_y: 0,
        unknown_20: [0; 4],
        len: 24,
    };
    project.channels.push(Channel {
        index: 0,
        kind: Some(ChannelKind::Sampler),
        sample_path: Some("click.wav".to_owned()),
        levels: Some(levels),
        routed_to: Some(1),
        order: [
            Id::ChannelNew,
            Id::ChannelType,
            Id::SamplePath,
            Id::Levels,
            Id::RoutedTo,
        ]
        .into_iter()
        .enumerate()
        .map(|(n, id)| placed(10 + n, id))
        .collect(),
        ..Channel::default()
    });
    project.channels.push(Channel {
        index: 1,
        kind: Some(ChannelKind::Plugin),
        levels: Some(levels),
        routed_to: Some(0),
        plugin_name: Some("TestSine".to_owned()),
        wrapper: Some(wrapper(0, 0, -1, 0x50)),
        plugin_state: Some(params(&[100])),
        order: [
            Id::ChannelNew,
            Id::ChannelType,
            Id::Levels,
            Id::RoutedTo,
            Id::PluginInternalName,
            Id::PluginWrapper,
            Id::PluginData,
        ]
        .into_iter()
        .enumerate()
        .map(|(n, id)| placed(100 + n, id))
        .collect(),
        ..Channel::default()
    });
    project.arrangements.push(Arrangement {
        index: 0,
        items: [0, BAR]
            .map(|at| PlaylistItem::new(ItemSource::Pattern(1), at, BAR))
            .to_vec(),
        order: vec![
            placed(200, Id::ArrangementNew),
            placed(201, Id::ArrangementPlaylist),
        ],
        ..Arrangement::default()
    });

    // The master and inserts 1 and 2, each on and sending to the master.
    let mut pos = 1000;
    let mut field = |id: Id| {
        pos += 10;
        placed(pos, id)
    };
    project.order.push(field(Id::MixerParams));
    for index in 0..3 {
        project.inserts.push(Insert {
            flags: Some(InsertFlags {
                unknown_0: [0; 4],
                flags: 0b1100,
                unknown_8: [0; 4],
                len: 12,
            }),
            routing: Some(vec![u8::from(index != 0), 0, 0]),
            output: Some(-1),
            order: vec![
                field(Id::InsertFlags),
                field(Id::InsertRouting),
                field(Id::InsertOutput),
            ],
            ..Insert::default()
        });
    }
    // Fast Dist as FL sets a new one, Delay 2 with an echo of a dotted
    // eighth and some feedback, and Peak Controller as its default.
    let effects: [(&str, Vec<u8>); 3] = [
        ("Fruity Fast Dist", params(&[128, 10, 0, 128, 128])),
        ("Fruity Delay 2", params(&[0, 128, 128, 64, 144, 0, 0, 128])),
        ("Fruity Peak Controller", {
            let mut state = params(&[0, 179, 0, 100, 0, 0, 0, 32_950, 0, 0]);
            state.extend([0, 0]);
            state
        }),
    ];
    let target = &mut project.inserts[1];
    let close = target.order.last().unwrap().pos;
    for (slot, (name, state)) in effects.into_iter().enumerate() {
        let first = close - 9 + 3 * slot;
        target.slots.push(Slot {
            index: slot as i32,
            plugin_name: Some(name.to_owned()),
            wrapper: Some(wrapper(1, slot as i32, 2, 0x140)),
            plugin_state: Some(state),
            order: [Id::PluginInternalName, Id::PluginWrapper, Id::PluginData]
                .into_iter()
                .enumerate()
                .map(|(n, id)| placed(first + n, id))
                .collect(),
            ..Slot::default()
        });
    }
    project
}

#[test]
fn the_engine_allocates_nothing_while_it_plays() {
    let trace = std::env::var_os("NO_ALLOC_TRACE").is_some();
    TRACE.store(trace, Ordering::SeqCst);
    let dir = Dir::new();
    let song = song();
    let samples = engine::prepare(&song, &dir.0).unwrap();
    let mut engine = Engine::new(&song, &samples, RATE).unwrap();
    engine.set_loop(true);
    engine.play();
    let (mut left, mut right) = (vec![0.0f32; 512], vec![0.0f32; 512]);
    // The first blocks start the pool's workers, which then set
    // themselves up on their own threads: give them the time to.
    for _ in 0..4 {
        engine.process(&mut left, &mut right);
    }
    std::thread::sleep(std::time::Duration::from_millis(200));

    let blocks = [512, 1, 37, 256, 500, 64];
    let mut loud = 0.0f32;
    COUNT.store(0, Ordering::SeqCst);
    INSIDE.store(true, Ordering::SeqCst);
    // Five times through the two bars.
    let mut played = 0;
    while played < 5 * 2 * 96_000 {
        for frames in blocks {
            engine.process(&mut left[..frames], &mut right[..frames]);
            loud = loud.max(left[0].abs());
            played += frames;
        }
    }
    INSIDE.store(false, Ordering::SeqCst);
    let count = COUNT.load(Ordering::SeqCst);
    assert!(loud > 0.0, "the song played silence");
    assert_eq!(count, 0, "the engine allocated {count} times as it played");
}
