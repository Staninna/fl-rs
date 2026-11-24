//! The single-cycle waves FLEX's oscillators read (`FUN_18038e890`): a
//! saw, a triangle, a sine and a square drawn in code, then the 24 cycles
//! of the float WAV `engine_x64.dll` carries at `0x1804f40a0` (Beast,
//! Crunch, .. Vocal, 1024 frames each), each band-limited into levels of
//! 2048 frames that halve the harmonics they keep, as the engine's 48
//! mip levels at `DAT_180514a80` do.
//!
//! The WAV is read from the install's DLL at run time; without it the 24
//! are silent.

use std::f64::consts::TAU;
use std::path::Path;

/// Frames in a cycle of every level.
pub(crate) const CYCLE: usize = 2048;
/// Levels: level `l` keeps the harmonics up to `1024 >> l`.
pub(crate) const LEVELS: usize = 11;
/// The waves drawn in code before the DLL's.
const DRAWN: usize = 4;
/// The DLL's cycles and their length.
const SLICES: usize = 24;
const SLICE: usize = 1024;
pub(crate) const WAVES: usize = DRAWN + SLICES;

/// Every wave at every level: `[wave][level]`, each `CYCLE + 1` frames with
/// the first repeated at the end for the interpolation.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Waves {
    tables: Vec<Vec<Vec<f32>>>,
    /// Whether the DLL's cycles were found.
    pub(crate) found: bool,
}

/// The four waves FLEX draws (`FUN_18038e890`): 0 a rising saw from -1,
/// 1 a triangle, 2 a sine, 3 a square that is -1 in the first half.
fn drawn(wave: usize) -> Vec<f64> {
    (0..CYCLE)
        .map(|i| {
            let x = i as f64 / CYCLE as f64;
            match wave {
                0 => 2.0 * x - 1.0,
                1 => {
                    if x < 0.5 {
                        4.0 * x - 1.0
                    } else {
                        3.0 - 4.0 * x
                    }
                }
                2 => (TAU * x).sin(),
                _ => {
                    if i < CYCLE / 2 {
                        -1.0
                    } else {
                        1.0
                    }
                }
            }
        })
        .collect()
}

/// The 24 cycles of the WAV inside `engine_x64.dll`: a 32-bit float mono
/// `RIFF` with a `cue ` and `LIST adtl` naming the slices, 24 576 frames.
pub(crate) fn dll_cycles(dll: &[u8]) -> Option<Vec<Vec<f64>>> {
    let mut at = 0;
    while let Some(found) = dll[at..].windows(4).position(|w| w == b"RIFF") {
        let start = at + found;
        at = start + 4;
        if dll.get(start + 8..start + 16) != Some(b"WAVEfmt ") {
            continue;
        }
        let format = u16::from_le_bytes(dll.get(start + 20..start + 22)?.try_into().ok()?);
        let bits = u16::from_le_bytes(dll.get(start + 34..start + 36)?.try_into().ok()?);
        if format != 3 || bits != 32 {
            continue;
        }
        let mut chunk = start + 12;
        while chunk + 8 <= dll.len() {
            let id = &dll[chunk..chunk + 4];
            let len = u32::from_le_bytes(dll[chunk + 4..chunk + 8].try_into().ok()?) as usize;
            if id == b"data" {
                let data = dll.get(chunk + 8..chunk + 8 + len)?;
                if data.len() < SLICES * SLICE * 4 {
                    return None;
                }
                let frames: Vec<f64> = data
                    .chunks_exact(4)
                    .map(|b| f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]])))
                    .collect();
                return Some(
                    frames
                        .chunks_exact(SLICE)
                        .take(SLICES)
                        .map(<[f64]>::to_vec)
                        .collect(),
                );
            }
            chunk += 8 + len + (len & 1);
        }
    }
    None
}

/// One cycle's spectrum at `CYCLE` points, whatever its length.
fn spectrum(cycle: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let n = cycle.len();
    let mut re = cycle.to_vec();
    let mut im = vec![0.0; n];
    crate::three_osc::fft(&mut re, &mut im, false);
    // Lay the harmonics of a cycle of any power-of-two length into one of
    // CYCLE points, the scale kept.
    let mut out_re = vec![0.0; CYCLE];
    let mut out_im = vec![0.0; CYCLE];
    let half = (n / 2).min(CYCLE / 2);
    let scale = CYCLE as f64 / n as f64;
    for h in 1..half {
        out_re[h] = re[h] * scale;
        out_im[h] = im[h] * scale;
        out_re[CYCLE - h] = re[n - h] * scale;
        out_im[CYCLE - h] = im[n - h] * scale;
    }
    (out_re, out_im)
}

/// The levels of one cycle: harmonics above each level's limit dropped,
/// and the DC with them.
fn levels(cycle: &[f64]) -> Vec<Vec<f32>> {
    let (re, im) = spectrum(cycle);
    (0..LEVELS)
        .map(|level| {
            let keep = (CYCLE / 2) >> level;
            let mut r = vec![0.0; CYCLE];
            let mut i = vec![0.0; CYCLE];
            for h in 1..=keep.min(CYCLE / 2 - 1) {
                r[h] = re[h];
                i[h] = im[h];
                r[CYCLE - h] = re[CYCLE - h];
                i[CYCLE - h] = im[CYCLE - h];
            }
            crate::three_osc::fft(&mut r, &mut i, true);
            let mut out: Vec<f32> = r.iter().map(|v| (v / CYCLE as f64) as f32).collect();
            out.push(out[0]);
            out
        })
        .collect()
}

impl Waves {
    /// Draw the four and band-limit the DLL's 24 when `dll` holds them.
    pub(crate) fn build(dll: Option<&[u8]>) -> Self {
        let cycles = dll.and_then(dll_cycles);
        let found = cycles.is_some();
        let mut tables: Vec<Vec<Vec<f32>>> = (0..DRAWN).map(|w| levels(&drawn(w))).collect();
        match cycles {
            Some(cycles) => tables.extend(cycles.iter().map(|c| levels(c))),
            None => tables.extend((0..SLICES).map(|_| vec![vec![0.0; CYCLE + 1]; LEVELS])),
        }
        Waves { tables, found }
    }

    /// Read from the install: `generators` is FL's `Plugins/Fruity/Generators`.
    pub(crate) fn load(generators: Option<&Path>) -> Self {
        let dll = generators.and_then(|dir| std::fs::read(dir.join("FLEX/engine_x64.dll")).ok());
        Waves::build(dll.as_deref())
    }

    /// The level that keeps a wave at `cycles_per_sample` below Nyquist.
    pub(crate) fn level(cycles_per_sample: f64) -> usize {
        let limit = 0.5 / cycles_per_sample.abs().max(1e-9);
        (0..LEVELS)
            .find(|&l| ((CYCLE / 2) >> l) as f64 <= limit)
            .unwrap_or(LEVELS - 1)
    }

    /// Wave `wave` at `level`, read at `phase` turns with a straight line
    /// between frames.
    #[inline]
    pub(crate) fn read(&self, wave: usize, level: usize, phase: f64) -> f32 {
        let table = &self.tables[wave.min(WAVES - 1)][level.min(LEVELS - 1)];
        let x = phase * CYCLE as f64;
        let i = (x as usize).min(CYCLE - 1);
        let frac = (x - i as f64) as f32;
        table[i] + (table[i + 1] - table[i]) * frac
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_drawn_waves_are_band_limited_and_keep_their_level() {
        let waves = Waves::build(None);
        assert!(!waves.found);
        // The sine is its own first level, at full scale.
        let quarter = waves.read(2, 0, 0.25);
        assert!((quarter - 1.0).abs() < 1e-3, "{quarter}");
        // The square's top level is only its fundamental: 4 / pi.
        let top = waves.read(3, LEVELS - 1, 0.75);
        assert!((top - 4.0 / std::f32::consts::PI).abs() < 1e-3, "{top}");
        // A saw at 1 kHz and 48 kHz keeps 24 harmonics at most.
        let level = Waves::level(1000.0 / 48_000.0);
        assert!((CYCLE / 2) >> level <= 24);
        assert!((CYCLE / 2) >> (level - 1) > 24);
        // The lowest level is the fundamental alone.
        let sine = waves.read(0, LEVELS - 1, 0.25);
        assert!((sine + 2.0 / std::f32::consts::PI).abs() < 1e-3, "{sine}");
    }

    #[test]
    fn the_dll_wave_is_found_by_its_header() {
        let mut dll = vec![0u8; 16];
        let frames = SLICES * SLICE;
        dll.extend_from_slice(b"RIFF");
        dll.extend_from_slice(&((frames * 4 + 36) as u32).to_le_bytes());
        dll.extend_from_slice(b"WAVEfmt ");
        dll.extend_from_slice(&16u32.to_le_bytes());
        dll.extend_from_slice(&3u16.to_le_bytes());
        dll.extend_from_slice(&1u16.to_le_bytes());
        dll.extend_from_slice(&44_100u32.to_le_bytes());
        dll.extend_from_slice(&176_400u32.to_le_bytes());
        dll.extend_from_slice(&4u16.to_le_bytes());
        dll.extend_from_slice(&32u16.to_le_bytes());
        dll.extend_from_slice(b"data");
        dll.extend_from_slice(&((frames * 4) as u32).to_le_bytes());
        for i in 0..frames {
            dll.extend_from_slice(&((i / SLICE) as f32).to_le_bytes());
        }
        let cycles = dll_cycles(&dll).unwrap();
        assert_eq!(cycles.len(), SLICES);
        assert_eq!(cycles[5][0], 5.0);
        assert!(dll_cycles(&dll[..dll.len() / 2]).is_none());
    }
}
