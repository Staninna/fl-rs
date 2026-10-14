//! What the plugins' tests share: states, tones and what a filter the docs
//! give as a transfer function does to them, and the states FL's own files
//! hold.

use std::f64::consts::TAU;
use std::path::{Path, PathBuf};

/// A state of little-endian i32.
pub(crate) fn state(values: &[i32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

pub(crate) fn bits(samples: &[f32]) -> Vec<u32> {
    samples.iter().map(|v| v.to_bits()).collect()
}

/// `len` samples of a sine at full scale that turns `turns` times a sample.
pub(crate) fn sine(turns: f64, len: usize) -> Vec<f32> {
    (0..len)
        .map(|n| (TAU * turns * n as f64).sin() as f32)
        .collect()
}

/// The peak of the partial at `turns` a sample in `samples`. The samples
/// are a whole number of its periods.
pub(crate) fn partial(samples: &[f32], turns: f64) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for (n, value) in samples.iter().enumerate() {
        let angle = TAU * turns * n as f64;
        re += f64::from(*value) * angle.cos();
        im += f64::from(*value) * angle.sin();
    }
    2.0 * re.hypot(im) / samples.len() as f64
}

/// The size of a polynomial in `z^-1` on the unit circle, at `turns` a
/// sample.
fn size(coefficients: &[f64], turns: f64) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for (k, c) in coefficients.iter().enumerate() {
        let angle = TAU * turns * k as f64;
        re += c * angle.cos();
        im -= c * angle.sin();
    }
    re.hypot(im)
}

/// The gain of the filter `num(z^-1) / den(z^-1)` at `turns` a sample.
pub(crate) fn response(num: &[f64], den: &[f64], turns: f64) -> f64 {
    size(num, turns) / size(den, turns)
}

/// One state FL's own files hold.
pub(crate) struct Saved {
    /// The preset's file name with no `.fst`, or the project's file name.
    pub(crate) file: String,
    /// The plugin's event 201 name.
    pub(crate) plugin: String,
    /// The plugin's event 213.
    pub(crate) state: Vec<u8>,
}

/// FL's `Data/` directory, where `FL_RS_CORPUS` names one. Without it the
/// tests that want FL's files pass and say what they skipped.
pub(crate) fn corpus() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("FL_RS_CORPUS")?);
    assert!(
        root.is_dir(),
        "FL_RS_CORPUS={} is not a directory",
        root.display()
    );
    Some(root)
}

/// The presets FL ships for the plugin of this name, in the order of their
/// file names. None where the install has no directory for the plugin.
pub(crate) fn presets(plugin: &str) -> Option<Vec<Saved>> {
    let Some(root) = corpus() else {
        eprintln!("FL_RS_CORPUS unset, presets of {plugin} skipped");
        return None;
    };
    let dir = root.join("Patches/Plugin presets/Effects").join(plugin);
    let mut saved = Vec::new();
    if dir.is_dir() {
        for path in files(&dir, "fst") {
            saved.extend(states(&path));
        }
    }
    Some(saved)
}

/// Every state FL ships in a preset for the plugin of this name, wherever
/// under `root` the preset is: the plugin's own presets, the mixer presets
/// and the plugin database.
pub(crate) fn every_preset(root: &Path, plugin: &str) -> Vec<Saved> {
    let saved = files(root, "fst").into_iter().flat_map(|p| states(&p));
    saved
        .filter(|saved| saved.plugin.eq_ignore_ascii_case(plugin))
        .collect()
}

/// The envelope maps FL ships under `root`: each `.fnv` file's name and
/// what is in it.
pub(crate) fn maps(root: &Path) -> Vec<(String, Vec<u8>)> {
    let read = |path: &PathBuf| {
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        (name, std::fs::read(path).unwrap())
    };
    files(root, "fnv").iter().map(read).collect()
}

/// Every plugin state in every project under `root`.
pub(crate) fn projects(root: &Path) -> Vec<Saved> {
    files(root, "flp").iter().flat_map(|p| states(p)).collect()
}

/// The files with this extension under `dir`, sorted.
fn files(dir: &Path, extension: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(extension))
            {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// The plugin states of a preset or a project: each event 213 with the
/// event 201 name before it.
fn states(path: &Path) -> Vec<Saved> {
    let bytes = std::fs::read(path).unwrap();
    let file = path.file_stem().unwrap().to_string_lossy().into_owned();
    assert_eq!(&bytes[..4], b"FLhd", "{file}");
    let header = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let mut at = 8 + header;
    assert_eq!(&bytes[at..at + 4], b"FLdt", "{file}");
    at += 8;

    let mut saved = Vec::new();
    let mut plugin = None;
    while at < bytes.len() {
        let id = bytes[at];
        at += 1;
        // An event is a byte, a word, a dword or a run of bytes with its
        // length before it, seven bits at a time.
        let len = match id {
            0..64 => 1,
            64..128 => 2,
            128..192 => 4,
            _ => {
                let (mut len, mut shift) = (0, 0);
                loop {
                    let byte = bytes[at];
                    at += 1;
                    len |= usize::from(byte & 0x7f) << shift;
                    shift += 7;
                    if byte & 0x80 == 0 {
                        break len;
                    }
                }
            }
        };
        let data = &bytes[at..at + len];
        at += len;
        match id {
            201 => plugin = Some(text(data)),
            213 => {
                if let Some(plugin) = plugin.take() {
                    saved.push(Saved {
                        file: file.clone(),
                        plugin,
                        state: data.to_vec(),
                    });
                }
            }
            _ => {}
        }
    }
    saved
}

/// A name as FL saves it: UTF-16 in its newer files and one byte a letter
/// in its older ones.
fn text(data: &[u8]) -> String {
    let wide = data.len() >= 2 && data[1] == 0;
    let text = if wide {
        let units: Vec<u16> = data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        data.iter().map(|byte| char::from(*byte)).collect()
    };
    text.trim_end_matches('\0').to_owned()
}
