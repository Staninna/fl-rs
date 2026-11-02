//! A sample file that is broken never panics the decoder and never makes it
//! ask for memory the file cannot account for.
//!
//! The files are built in code: PCM and float WAV of several widths and
//! channel counts, WAV headers with odd fields (no channels, a width of 0,
//! 12 or 64 bits, a block of 0 bytes, no sample rate, a chunk that claims
//! 4 GB), Ogg pages inside a WAV, and WavPack from wavicle. Each is cut
//! short at every length, flipped a bit at a time and overwritten at
//! random, and random bytes behind each format's magic go in too. Whatever
//! decodes must be a sample the engine can play: a rate above 0 when it has
//! frames, both sides of one length, and nothing that is not finite.
//!
//! The cases come from a small fixed-seed generator, so a failure repeats.

use super::{Sample, decode_bytes, first_ogg_stream, ogg_in_wav, read_audio, wavpack_blocks};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, RefCell};
use std::panic::{self, AssertUnwindSafe};
use std::sync::Once;

/// The system allocator, keeping the largest single request this thread
/// made while [`largest_allocation`] watched. It is the allocator of the
/// whole lib test binary, and does nothing but hand on outside a watch.
struct Watching;

thread_local! {
    static WATCH: Cell<bool> = const { Cell::new(false) };
    static LARGEST: Cell<usize> = const { Cell::new(0) };
}

fn note(size: usize) {
    // `try_with`: the thread's locals may already be gone while it exits.
    let _ = WATCH.try_with(|watch| {
        if watch.get() {
            let _ = LARGEST.try_with(|largest| largest.set(largest.get().max(size)));
        }
    });
}

// A global allocator is unsafe to write; this one only hands on to the
// system's.
#[allow(unsafe_code)]
unsafe impl GlobalAlloc for Watching {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Watching = Watching;

/// Run `f` and give the largest single allocation it made on this thread,
/// 0 when it made none. Other tests of the crate use it too.
pub(crate) fn largest_allocation<T>(f: impl FnOnce() -> T) -> (T, usize) {
    LARGEST.with(|largest| largest.set(0));
    WATCH.with(|watch| watch.set(true));
    let out = f();
    WATCH.with(|watch| watch.set(false));
    (out, LARGEST.with(Cell::get))
}

thread_local! {
    /// Where the last panic on this thread came from, while
    /// [`watching_panics`] runs.
    static PANICKED_AT: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Run `f`, and say where in the source the last panic in it came from.
/// Such a panic is not printed. The hook this installs hands every panic
/// it does not watch to the hook that was there before.
fn watching_panics<T>(f: impl FnOnce() -> T) -> (T, Option<String>) {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let before = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let watched = PANICKED_AT.with(|at| {
                let mut at = at.borrow_mut();
                let watched = at.is_some();
                if watched {
                    *at = Some(info.location().map_or_else(String::new, |l| l.to_string()));
                }
                watched
            });
            if !watched {
                before(info);
            }
        }));
    });
    PANICKED_AT.with(|at| *at.borrow_mut() = Some(String::new()));
    let out = f();
    let at = PANICKED_AT.with(|at| at.borrow_mut().take());
    (out, at.filter(|at| !at.is_empty()))
}

/// Run the decoder without [`decode_bytes`]'s guard, and say where in the
/// source it panicked, if it did.
fn unguarded(bytes: &[u8], extension: &str) -> Option<String> {
    let (read, at) = watching_panics(|| {
        panic::catch_unwind(AssertUnwindSafe(|| {
            read_audio(bytes.to_vec(), Some(extension))
        }))
    });
    read.err().and(at)
}

/// Whether a panic came from a crate the engine depends on rather than
/// from its own code. [`decode_bytes`] catches those; the engine's own
/// code must not panic at all.
fn in_a_dependency(at: &str) -> bool {
    at.contains(".cargo/registry") || at.contains(".cargo\\registry")
}

/// SplitMix64: small, fast and the same on every machine.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number below `n`, which must not be 0.
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn byte(&mut self) -> u8 {
        self.next() as u8
    }

    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.byte()).collect()
    }
}

/// The fields of a WAV `fmt ` chunk, as the file states them.
#[derive(Clone, Copy)]
struct Format {
    tag: u16,
    channels: u16,
    rate: u32,
    block_align: u16,
    bits: u16,
}

impl Format {
    fn pcm(channels: u16, bits: u16) -> Self {
        Format {
            tag: 1,
            channels,
            rate: 44_100,
            block_align: channels.wrapping_mul(bits.div_ceil(8)),
            bits,
        }
    }
}

/// A RIFF chunk: its ID, the size it states and its body.
fn chunk(out: &mut Vec<u8>, id: &[u8; 4], size: u32, body: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(body);
    if body.len() % 2 == 1 {
        out.push(0);
    }
}

/// A WAV file of `format` around `data`. `data_size` is the size its
/// `data` chunk states, `None` for the true one.
fn wav(format: Format, data: &[u8], data_size: Option<u32>) -> Vec<u8> {
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&format.tag.to_le_bytes());
    fmt.extend_from_slice(&format.channels.to_le_bytes());
    fmt.extend_from_slice(&format.rate.to_le_bytes());
    let byte_rate = format.rate.wrapping_mul(u32::from(format.block_align));
    fmt.extend_from_slice(&byte_rate.to_le_bytes());
    fmt.extend_from_slice(&format.block_align.to_le_bytes());
    fmt.extend_from_slice(&format.bits.to_le_bytes());
    if format.tag == 0xFFFE {
        // WAVE_FORMAT_EXTENSIBLE: valid bits, channel mask, PCM's GUID.
        fmt.extend_from_slice(&22u16.to_le_bytes());
        fmt.extend_from_slice(&format.bits.to_le_bytes());
        fmt.extend_from_slice(&3u32.to_le_bytes());
        fmt.extend_from_slice(&[
            1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71,
        ]);
    }
    let mut body = b"WAVE".to_vec();
    chunk(&mut body, b"fmt ", fmt.len() as u32, &fmt);
    let size = data_size.unwrap_or(data.len() as u32);
    chunk(&mut body, b"data", size, data);
    let mut file = Vec::new();
    chunk(&mut file, b"RIFF", body.len() as u32, &body);
    file
}

/// Frames of a ramp in `bits`-wide little-endian integers, or in f32 for
/// a float format.
fn frames(frames: usize, channels: usize, bits: u16, float: bool) -> Vec<u8> {
    let mut out = Vec::new();
    for i in 0..frames * channels {
        let x = (i as f64 * 0.37).sin();
        if float {
            out.extend_from_slice(&(x as f32).to_le_bytes());
        } else {
            let width = usize::from(bits.div_ceil(8));
            let scaled = (x * ((1i64 << (width * 8 - 1)) - 1) as f64) as i64;
            out.extend_from_slice(&scaled.to_le_bytes()[..width]);
        }
    }
    out
}

/// An Ogg page with one segment: `serial`, then `body`.
fn page(serial: u32, body: &[u8]) -> Vec<u8> {
    let mut page = b"OggS".to_vec();
    page.extend_from_slice(&[0; 10]);
    page.extend_from_slice(&serial.to_le_bytes());
    page.extend_from_slice(&[0; 8]);
    page.extend_from_slice(&[1, body.len() as u8]);
    page.extend_from_slice(body);
    page
}

/// Files that decode, of every kind built here.
fn good_files() -> Vec<(Vec<u8>, &'static str)> {
    let mut files = Vec::new();
    for (channels, bits) in [(1, 8), (2, 16), (1, 24), (2, 32), (6, 16)] {
        let data = frames(40, usize::from(channels), bits, false);
        files.push((wav(Format::pcm(channels, bits), &data, None), "wav"));
    }
    let float = Format {
        tag: 3,
        ..Format::pcm(2, 32)
    };
    files.push((wav(float, &frames(40, 2, 32, true), None), "wav"));
    let extensible = Format {
        tag: 0xFFFE,
        ..Format::pcm(2, 24)
    };
    files.push((wav(extensible, &frames(40, 2, 24, false), None), "wav"));
    let params = |channels, bits_per_sample| wavicle::EncodeParams {
        channels,
        sample_rate: 44_100,
        bits_per_sample,
    };
    let ramp: Vec<i32> = (0..80).map(|i| (i * 300) - 12_000).collect();
    files.push((wavicle::encode_int(params(2, 16), &ramp).unwrap(), "wv"));
    files.push((wavicle::encode_int(params(1, 24), &ramp).unwrap(), "wv"));
    let floats: Vec<f32> = (0..40).map(|i| i as f32 / 40.0).collect();
    files.push((wavicle::encode_float(1, 48_000, &floats).unwrap(), "wv"));
    files
}

/// Files whose header says something odd, and files that are only a
/// format's magic. Each must decode or fail, nothing else.
fn odd_files() -> Vec<(Vec<u8>, &'static str)> {
    let data = frames(20, 2, 16, false);
    let mut files = Vec::new();
    let odd = [
        Format::pcm(0, 16),
        Format {
            bits: 0,
            ..Format::pcm(2, 16)
        },
        Format::pcm(2, 12),
        Format::pcm(1, 64),
        Format::pcm(1, 1),
        Format {
            block_align: 0,
            ..Format::pcm(2, 16)
        },
        Format {
            block_align: 3,
            ..Format::pcm(2, 16)
        },
        Format {
            rate: 0,
            ..Format::pcm(2, 16)
        },
        Format {
            rate: u32::MAX,
            ..Format::pcm(2, 16)
        },
        Format::pcm(u16::MAX, 16),
        Format {
            tag: 3,
            ..Format::pcm(1, 16)
        },
        Format {
            tag: 3,
            ..Format::pcm(1, 64)
        },
        Format {
            tag: 0xFFFE,
            ..Format::pcm(0, 24)
        },
        Format {
            tag: 0x674F,
            ..Format::pcm(2, 16)
        },
    ];
    for format in odd {
        files.push((wav(format, &data, None), "wav"));
    }
    // Chunk sizes past the end, and 4 GB.
    for size in [data.len() as u32 + 1, 1 << 30, u32::MAX - 1, u32::MAX] {
        files.push((wav(Format::pcm(2, 16), &data, Some(size)), "wav"));
    }
    let mut huge_fmt = wav(Format::pcm(2, 16), &data, None);
    huge_fmt[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
    files.push((huge_fmt, "wav"));
    let mut huge_riff = wav(Format::pcm(2, 16), &data, None);
    huge_riff[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    files.push((huge_riff, "wav"));
    // A LIST chunk that claims 4 GB before the audio.
    let mut listed = b"RIFF\0\0\0\0WAVE".to_vec();
    chunk(&mut listed, b"LIST", u32::MAX, b"INFOISFT");
    files.push((listed, "wav"));
    // Ogg in a WAV, of one stream and of two.
    let ogg = [page(7, b"abc"), page(0x10, b"x"), page(7, b"def")].concat();
    let ogg_format = Format {
        tag: 0x674F,
        ..Format::pcm(2, 16)
    };
    files.push((wav(ogg_format, &ogg, None), "wav"));
    files.push((ogg.clone(), "ogg"));
    // WavPack with a block size past the end, and 4 GB.
    let params = wavicle::EncodeParams {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 16,
    };
    let wv = wavicle::encode_int(params, &[1, 2, 3, 4]).unwrap();
    for size in [wv.len() as u32, u32::MAX] {
        let mut file = wv.clone();
        file[4..8].copy_from_slice(&size.to_le_bytes());
        files.push((file, "wv"));
    }
    for magic in [
        &b"RIFF"[..],
        b"RIFF\x04\0\0\0WAVE",
        b"OggS",
        b"fLaC",
        b"fLaC\0\0\0\x22",
        b"ID3\x04\0\0\0\0\0\0",
        &[0xFF, 0xFB, 0x90, 0x64],
        b"wvpk",
        b"wvpk\x20\0\0\0",
        b"",
    ] {
        for extension in ["wav", "ogg", "flac", "mp3", "wv", "xyz"] {
            files.push((magic.to_vec(), extension));
        }
    }
    files
}

/// What a file of `len` bytes may ask for at once. A decoder turns a byte
/// into at most one f32 per channel side and keeps a copy or two of the
/// file, and it may size a buffer for the longest block its format allows:
/// FLAC's 65535 frames of 8 channels is 4 MB. The 4 GB a header can claim
/// is far past this.
fn allowance(len: usize) -> usize {
    (8 << 20) + 64 * len
}

/// A sample the engine can play.
fn check_playable(sample: &Sample, what: &str) {
    if !sample.left.is_empty() {
        assert!(sample.rate > 0, "{what}: frames at rate 0");
    }
    if let Some(right) = &sample.right {
        assert_eq!(right.len(), sample.left.len(), "{what}: sides differ");
    }
    let all = sample.left.iter().chain(sample.right.iter().flatten());
    assert!(all.clone().all(|x| x.is_finite()), "{what}: not finite");
}

/// Decode `bytes`: it may fail, but not panic, not ask for too much, and
/// not give a sample the engine cannot play.
fn check(bytes: &[u8], extension: &str) {
    let what = format!("{extension} {bytes:02x?}");
    let what = &what[..what.len().min(400)];
    // A panic the guard catches is not printed: `unguarded` below says
    // where it came from.
    let ((decoded, largest), _) = watching_panics(|| {
        largest_allocation(|| {
            // The helpers on their own, on whatever the bytes are.
            let _ = ogg_in_wav(bytes);
            let _ = first_ogg_stream(bytes);
            let _ = wavpack_blocks(bytes);
            decode_bytes(bytes.to_vec(), Some(extension))
        })
    });
    if let Ok(Some(sample)) = decoded {
        check_playable(&sample, what);
    }
    assert!(
        largest <= allowance(bytes.len()),
        "{what}: asked for {largest} bytes at once"
    );
    if let Some(at) = unguarded(bytes, extension) {
        assert!(in_a_dependency(&at), "{what}: panicked at {at}");
        DEPENDENCY_PANICS.with(|n| n.set(n.get() + 1));
    }
}

thread_local! {
    /// How many files made a dependency panic, which the guard caught.
    static DEPENDENCY_PANICS: Cell<usize> = const { Cell::new(0) };
}

/// Print how many files the guard saved, for whoever reads the output.
fn report(test: &str) {
    let n = DEPENDENCY_PANICS.with(|n| n.replace(0));
    eprintln!("{test}: {n} files made a decoder dependency panic, caught by the guard");
}

#[test]
fn the_good_files_decode() {
    for (bytes, extension) in good_files() {
        let sample = decode_bytes(bytes, Some(extension)).unwrap().unwrap();
        assert!(sample.left.len() >= 40, "{extension}");
        check_playable(&sample, extension);
    }
}

#[test]
fn odd_headers_and_bare_magic_never_panic() {
    for (bytes, extension) in odd_files() {
        check(&bytes, extension);
    }
    report("odd_headers_and_bare_magic_never_panic");
}

#[test]
fn a_file_cut_short_anywhere_never_panics() {
    for (bytes, extension) in good_files().into_iter().chain(odd_files()) {
        for len in 0..bytes.len() {
            check(&bytes[..len], extension);
        }
    }
    report("a_file_cut_short_anywhere_never_panics");
}

#[test]
fn flipped_bits_never_panic() {
    let mut rng = Rng(21);
    for (bytes, extension) in good_files() {
        // Every bit of the headers, and random ones after.
        for bit in 0..bytes.len().min(64) * 8 {
            let mut flipped = bytes.clone();
            flipped[bit / 8] ^= 1 << (bit % 8);
            check(&flipped, extension);
        }
        for _ in 0..300 {
            let mut flipped = bytes.clone();
            for _ in 0..1 + rng.below(6) {
                let i = rng.below(flipped.len());
                flipped[i] ^= 1 << rng.below(8);
            }
            check(&flipped, extension);
        }
    }
    report("flipped_bits_never_panic");
}

#[test]
fn overwritten_header_fields_never_panic() {
    // A size, a count or a width at an end of its range, anywhere in the
    // first 64 bytes, where every header here keeps its fields.
    let mut rng = Rng(22);
    for (bytes, extension) in good_files() {
        for _ in 0..300 {
            let mut mangled = bytes.clone();
            let width = [1, 2, 4][rng.below(3)];
            let at = rng.below(bytes.len().min(64) - width);
            let value = match rng.below(4) {
                0 => vec![0; width],
                1 => vec![0xff; width],
                2 => vec![0x7f; width],
                _ => rng.bytes(width),
            };
            mangled[at..at + width].copy_from_slice(&value);
            check(&mangled, extension);
        }
    }
    report("overwritten_header_fields_never_panic");
}

#[test]
fn random_bytes_behind_a_magic_never_panic() {
    let mut rng = Rng(23);
    let magics: [&[u8]; 7] = [
        b"RIFF\x00\x01\x00\x00WAVEfmt ",
        b"OggS\0\x02",
        b"fLaC",
        b"ID3\x03\0\0\0\0\0\0",
        &[0xFF, 0xFB],
        b"wvpk",
        b"",
    ];
    for _ in 0..2000 {
        let magic = magics[rng.below(magics.len())];
        let len = rng.below(300);
        let bytes = [magic, &rng.bytes(len)].concat();
        let extension = ["wav", "ogg", "flac", "mp3", "wv"][rng.below(5)];
        check(&bytes, extension);
    }
    report("random_bytes_behind_a_magic_never_panic");
}

#[test]
fn what_decodes_is_made_playable() {
    // NaN and infinities in a float file are silence.
    let float = Format {
        tag: 3,
        ..Format::pcm(1, 32)
    };
    let data: Vec<u8> = [0.5, f32::NAN, f32::INFINITY, -0.5]
        .iter()
        .flat_map(|x: &f32| x.to_le_bytes())
        .collect();
    let sample = decode_bytes(wav(float, &data, None), Some("wav"))
        .unwrap()
        .unwrap();
    assert_eq!(sample.left, [0.5, 0.0, 0.0, -0.5]);
    // Frames with no rate to play them at are an error.
    let no_rate = Format {
        rate: 0,
        ..Format::pcm(1, 16)
    };
    assert!(decode_bytes(wav(no_rate, &[0; 8], None), Some("wav")).is_err());
}
