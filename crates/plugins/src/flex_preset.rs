//! A FLEX v1 preset (`.flexpreset`) once unmasked: the reader of
//! `engine_x64.dll` (`FUN_1803f1300`) written down field by field, as
//! `docs/plugins/flex.md` has it.
//!
//! The preset is a stream of little-endian fields with no chunk sizes, so
//! every field up to the last layer is read, whether it plays or not.

use crate::flex_pack::MAGIC;

/// The newest preset version the engine reads.
pub(crate) const NEWEST: i32 = 0x23;
/// The most layers a preset has.
pub(crate) const LAYERS: usize = 16;

/// A reader over the preset's bytes that runs dry instead of panicking.
struct Stream<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Stream<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let out = self.bytes.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(out)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }

    fn i32(&mut self) -> Option<i32> {
        Some(i32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn i64(&mut self) -> Option<i64> {
        Some(i64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn f64(&mut self) -> Option<f64> {
        Some(f64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    /// A count, which is never negative and never more than the bytes
    /// left could hold.
    fn count(&mut self, each: usize) -> Option<usize> {
        let n = usize::try_from(self.i32()?).ok()?;
        (n.saturating_mul(each.max(1)) <= self.bytes.len() - self.at).then_some(n)
    }

    /// `FUN_1801b64a0`: a length with the trailing NUL, then the bytes.
    fn string(&mut self) -> Option<String> {
        let n = self.count(1)?;
        let raw = self.take(n)?;
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        Some(String::from_utf8_lossy(&raw[..end]).into_owned())
    }

    fn floats(&mut self) -> Option<Vec<f32>> {
        let n = self.count(4)?;
        (0..n).map(|_| self.f32()).collect()
    }
}

/// One of a zone's four sample slots (`FUN_18041f770`).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Slot {
    pub(crate) flag: bool,
    /// The loop mode at `+0x10`: 0 none, 1 forward (`inferred`).
    pub(crate) mode: i32,
    pub(crate) start: i64,
    pub(crate) loop_start: i64,
    pub(crate) loop_end: i64,
    /// The file as the preset names it, a `.wav` that the pack has as a
    /// `.flac` of the same stem.
    pub(crate) file: String,
    pub(crate) tag: i32,
    pub(crate) gain: f64,
}

/// A wavetable read over a slot's sample (`FUN_18041c970`).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Table {
    pub(crate) name: String,
    /// Frames in one cycle (`+0x14`).
    pub(crate) wavelength: i32,
    /// Cycles in the table (`+0x10`), at most 256.
    pub(crate) cycles: i32,
}

/// A point envelope (`FUN_1803c0d30`): x and y, a curve type and its bend.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Graph {
    pub(crate) flag: u8,
    pub(crate) points: Vec<(f64, f64, u8, f32)>,
}

/// A zone (`FUN_180427bf0`): a key and velocity range, four sample slots
/// and the 744 parameters of its voice.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Zone {
    pub(crate) version: u8,
    pub(crate) name: String,
    pub(crate) root: i32,
    pub(crate) low_key: i32,
    pub(crate) high_key: i32,
    pub(crate) low_velocity: i32,
    pub(crate) high_velocity: i32,
    pub(crate) slots: Vec<Slot>,
    pub(crate) params: Vec<f32>,
    pub(crate) graphs: Vec<Graph>,
    pub(crate) tables: Vec<Table>,
    /// Version 9 on: the two custom waves of each oscillator by name.
    pub(crate) waves: Vec<(String, String)>,
}

/// One row of a modulation matrix (`FUN_180375750`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Route {
    pub(crate) source: u16,
    pub(crate) on: bool,
    pub(crate) target: i32,
    pub(crate) flag: bool,
    pub(crate) amount: f64,
    pub(crate) curve: f32,
    pub(crate) last: bool,
}

/// An effect of a chain (`FUN_1803bd0c0`): its type, whether it is on,
/// and its parameters, or for one of FL's own plugins the state FL saves
/// for it.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Effect {
    pub(crate) kind: i32,
    pub(crate) bypass: bool,
    pub(crate) on: bool,
    pub(crate) params: Vec<f32>,
    pub(crate) state: Option<Vec<u8>>,
}

/// The FL plugin an effect type hosts (`FUN_1803bbc80`'s jump table).
pub(crate) fn hosted_plugin(kind: i32) -> Option<&'static str> {
    Some(match kind {
        11 => "Fruity Convolver",
        13 => "Maximus",
        14 => "Fruity WaveShaper",
        15 => "EQUO",
        16 => "Fruity Flangus",
        17 => "Fruity Stereo Enhancer",
        18 => "Patcher",
        19 => "Fruity Love Philter",
        _ => return None,
    })
}

/// A link of a chain: a macro or source moving an effect parameter.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Link {
    pub(crate) a: i32,
    pub(crate) b: i32,
    pub(crate) amount: f32,
    pub(crate) d: f32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Chain {
    pub(crate) version: i32,
    pub(crate) effects: Vec<Effect>,
    pub(crate) links: Vec<Link>,
}

/// A layer (`FUN_1803c9aa0`): its 300 parameters, its zones, its
/// modulation matrices and its effects.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Layer {
    pub(crate) version: i32,
    pub(crate) params: Vec<f32>,
    pub(crate) zones: Vec<Zone>,
    pub(crate) matrices: Vec<Vec<Route>>,
    pub(crate) chain: Chain,
}

/// A whole preset as far as its last layer.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Preset {
    pub(crate) version: i32,
    /// The 44 or 45 shell parameters the preset sets, as the state's
    /// chunk 2 has them.
    pub(crate) shell: Vec<f32>,
    pub(crate) layers: Vec<Layer>,
}

fn slot(s: &mut Stream) -> Option<Slot> {
    let version = s.i32()?;
    let mut slot = Slot {
        flag: s.u8()? > 0,
        mode: s.i32()?,
        start: s.i64()?,
        loop_start: s.i64()?,
        loop_end: s.i64()?,
        file: s.string()?,
        tag: 1,
        gain: 0.0,
    };
    if version >= 2 {
        slot.tag = s.i32()?;
    }
    if version >= 3 {
        slot.gain = s.f64()?;
    }
    Some(slot)
}

fn graph(s: &mut Stream) -> Option<Graph> {
    let flag = s.u8()?;
    let n = s.count(21)?;
    let points = (0..n)
        .map(|_| Some((s.f64()?, s.f64()?, s.u8()?, s.f32()?)))
        .collect::<Option<_>>()?;
    Some(Graph { flag, points })
}

fn zone(s: &mut Stream) -> Option<Zone> {
    let mut zone = Zone::default();
    if s.bytes.get(s.at) == Some(&0xff) {
        s.at += 1;
        zone.version = s.u8()?;
    }
    zone.name = s.string()?;
    zone.root = s.i32()?;
    zone.low_key = s.i32()?;
    zone.high_key = s.i32()?;
    zone.low_velocity = s.i32()?;
    zone.high_velocity = s.i32()?;
    s.i32()?;
    if zone.version >= 2 {
        s.i32()?;
        s.i32()?;
    }
    let n = s.count(30)?;
    zone.slots = (0..n).map(|_| slot(s)).collect::<Option<_>>()?;
    zone.params = s.floats()?;
    if zone.version >= 1 {
        zone.graphs = (0..4).map(|_| graph(s)).collect::<Option<_>>()?;
    }
    if zone.version >= 5 {
        zone.tables = (0..zone.slots.len())
            .map(|_| {
                s.i32()?;
                Some(Table {
                    name: s.string()?,
                    wavelength: s.i32()?,
                    cycles: s.i32()?,
                })
            })
            .collect::<Option<_>>()?;
    }
    if zone.version >= 9 {
        zone.waves = (0..4)
            .map(|_| Some((s.string()?, s.string()?)))
            .collect::<Option<_>>()?;
    }
    Some(zone)
}

fn matrix(s: &mut Stream) -> Option<Vec<Route>> {
    s.u16()?;
    s.i32()?;
    (0..32)
        .map(|_| {
            Some(Route {
                source: s.u16()?,
                on: s.u8()? > 0,
                target: s.i32()?,
                flag: s.u8()? > 0,
                amount: s.f64()?,
                curve: s.f32()?,
                last: s.u8()? > 0,
            })
        })
        .collect()
}

fn chain(s: &mut Stream) -> Option<Chain> {
    if s.take(4)? != b"hcxf" {
        return None;
    }
    let version = s.i32()?;
    if version > 5 {
        return None;
    }
    let n = s.count(4)?;
    let mut effects = Vec::with_capacity(n);
    for _ in 0..n {
        let kind = s.i32()?;
        let bypass = version >= 2 && s.u8()? > 0;
        let on = version < 5 || s.u8()? > 0;
        effects.push(Effect {
            kind,
            bypass,
            on,
            ..Effect::default()
        });
    }
    for effect in &mut effects {
        let own = version >= 4 && s.u8()? > 0;
        if own {
            // An FL plugin saves its state, a length and the bytes; the
            // engine's own effects save nothing of their own.
            if hosted_plugin(effect.kind).is_some() {
                // Maximus (`FUN_1803d3570`) wraps the state in a version,
                // three floats and from version 1 an int of its own.
                let maximus = effect.kind == 13;
                let version = if maximus { s.u16()? } else { 0 };
                let n = s.count(1)?;
                effect.state = Some(s.take(n)?.to_vec());
                if maximus {
                    s.take(12)?;
                    if version >= 1 {
                        s.i32()?;
                    }
                }
            }
        } else {
            effect.params = s.floats()?;
        }
    }
    let n = s.count(12)?;
    let links = (0..n)
        .map(|_| {
            Some(Link {
                a: s.i32()?,
                b: s.i32()?,
                amount: s.f32()?,
                d: if version >= 3 { s.f32()? } else { 1.0 },
            })
        })
        .collect::<Option<_>>()?;
    Some(Chain {
        version,
        effects,
        links,
    })
}

fn layer(s: &mut Stream) -> Option<Layer> {
    if s.take(4)? != b"mgrp" {
        return None;
    }
    let version = s.i32()?;
    if version > 4 {
        return None;
    }
    let params = s.floats()?;
    let n = s.count(40)?;
    let zones = (0..n).map(|_| zone(s)).collect::<Option<_>>()?;
    let matrices = if version <= 3 {
        vec![matrix(s)?]
    } else {
        let n = s.count(678)?;
        (0..n).map(|_| matrix(s)).collect::<Option<_>>()?
    };
    let chain = chain(s)?;
    if version >= 2 {
        for _ in 0..4 {
            graph(s)?;
        }
    }
    if version >= 3 {
        s.take(99 * 9)?;
    }
    Some(Layer {
        version,
        params,
        zones,
        matrices,
        chain,
    })
}

/// Read an unmasked preset as far as its last layer.
pub(crate) fn parse(bytes: &[u8]) -> Option<Preset> {
    let mut s = Stream { bytes, at: 0 };
    if s.take(8)? != MAGIC {
        return None;
    }
    let version = s.i32()?;
    if version > NEWEST {
        return None;
    }
    let shell = s.floats()?;
    let n = s.count(16)?;
    let layers = (0..n).map(|_| layer(&mut s)).collect::<Option<Vec<_>>>()?;
    Some(Preset {
        version,
        shell,
        layers: layers.into_iter().take(LAYERS).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preset_without_layers_is_its_header() {
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&17i32.to_le_bytes());
        bytes.extend_from_slice(&2i32.to_le_bytes());
        bytes.extend_from_slice(&0.5f32.to_le_bytes());
        bytes.extend_from_slice(&1.0f32.to_le_bytes());
        bytes.extend_from_slice(&0i32.to_le_bytes());
        let preset = parse(&bytes).unwrap();
        assert_eq!(preset.version, 17);
        assert_eq!(preset.shell, [0.5, 1.0]);
        assert!(preset.layers.is_empty());
        // A version past the newest, and bytes cut short, are not read.
        let mut newer = bytes.clone();
        newer[8] = 0x24;
        assert!(parse(&newer).is_none());
        assert!(parse(&bytes[..bytes.len() - 1]).is_none());
    }
}
