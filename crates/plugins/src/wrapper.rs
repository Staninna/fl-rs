//! Fruity Wrapper holding the VST build of one of FL's own effects, as
//! `docs/plugins/fruity-wrapper.md` has it.
//!
//! FL 11 to 20 shipped some of its effects as VSTs too, and a project that
//! loaded one saved it in a Fruity Wrapper slot. FL no longer has those
//! VSTs: when its loader meets such a slot it makes the effect's own build
//! in its place, at its defaults, and moves its parameters to the values
//! the VST's chunk holds (`FLEngine_x64.dll` `0xaa38d0`, `0xaa28e0`,
//! `0xf46490`). [`Wrapper`] does the same with the effect [`crate::by_name`]
//! makes, and is the effect from then on.
//!
//! A wrapper that holds any other plugin, a third-party VST or one of FL's
//! effects not built here, turns its state down, and its slot is passed
//! through as before.

use crate::{Effect, Fidelity, StateError};
use std::path::Path;

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Wrapper";

/// The effects FL's loader puts in place of their VST builds: the names a
/// wrapper's state gives, each the name of the effect itself.
/// `FLEngine_x64.dll` `0xaa2cb0` registers them, kind 0, with the
/// conversion at `0xaa28e0`. Its 7 Band EQ entries, Newtone and the
/// Soundfont Player are left out: the first is not built and the others
/// convert another way.
pub const NATIVE: [&str; 15] = [
    "Fruity Balance",
    "Fruity Bass Boost",
    "Fruity Blood Overdrive",
    "Fruity Center",
    "Fruity Chorus",
    "Fruity Compressor",
    "Fruity Delay",
    "Fruity Fast LP",
    "Fruity Filter",
    "Fruity Flanger",
    "Fruity Free Filter",
    "Fruity Mute 2",
    "Fruity Phase Inverter",
    "Fruity Phaser",
    "Fruity Reeverb",
];

/// Tags of the wrapper's state, read by `0xa9f840`.
const TAG_KIND: u32 = 0x32;
const TAG_CHUNK: u32 = 0x35;
const TAG_NAME: u32 = 0x36;

/// What a wrapper's state says: the plugin it held and the VST parameters
/// the plugin's chunk saved, each from 0 to 1.
#[derive(Debug, Clone, PartialEq)]
pub struct Wrapped {
    /// The first byte of tag 0x32, the kind FL's loader looks the name up
    /// under. 0 for an effect.
    pub kind: u8,
    /// Tag 0x36, the name of the plugin.
    pub name: String,
    /// The parameters out of tag 0x35, `None` when there is no such tag or
    /// it is of a form that has none.
    pub params: Option<Vec<f32>>,
}

/// Read a wrapper's state as `0xa9fb30` does: a version, then for the
/// versions 5 to 12 a run of tags, each a 32-bit tag, a 64-bit length and
/// that many bytes. Tag 0 is skipped. Versions below 5 are an older layout
/// (`0xa9f7f0`) that is not read here, and turned down as unhosted; FL
/// takes 13 and up, and -1, for no wrapper state at all.
pub fn read(bytes: &[u8]) -> Result<Wrapped, StateError> {
    let version = i32::from_le_bytes(word(bytes, 0)?);
    if version >= 13 || version == -1 {
        return Err(StateError::Version { found: version });
    }
    // None of FL's projects has one of the older layout.
    if version < 5 {
        return Err(StateError::Unhosted);
    }
    let mut wrapped = Wrapped {
        kind: 0,
        name: String::new(),
        params: None,
    };
    let mut at = 4;
    while at < bytes.len() {
        let (mut tag, mut len) = header(bytes, at)?;
        // A tag 0 is a header to read past, length and all (`0x8e9180`).
        if tag == 0 {
            at += 12;
            (tag, len) = header(bytes, at)?;
        }
        let start = at + 12;
        let end = usize::try_from(len)
            .ok()
            .and_then(|len| start.checked_add(len))
            .filter(|end| *end <= bytes.len())
            .ok_or(short(bytes, start.saturating_add(len as usize)))?;
        let data = &bytes[start..end];
        match tag {
            TAG_KIND => wrapped.kind = data.first().copied().unwrap_or(0),
            TAG_NAME => wrapped.name = data.iter().map(|&b| char::from(b)).collect(),
            TAG_CHUNK => wrapped.params = chunk_params(data),
            _ => {}
        }
        at = end;
    }
    Ok(wrapped)
}

/// The parameters out of a VST chunk as `0xa9f3c0` reads it: a first word
/// `v`; from `v < -5` a byte; from `v < -4` a word that is -2 before a
/// 64-bit program number and the number itself otherwise; from `v < -5`
/// one more word. The count is the next word when `v` is -2, or `v < -4`
/// and the program number is 0 or -1; else it is `v` itself. A count of 0
/// or more is followed by that many f32. 123456 and anything from -12 down
/// are no chunk.
pub fn chunk_params(chunk: &[u8]) -> Option<Vec<f32>> {
    let mut at = 0;
    let mut next = |n: usize| {
        let bytes = chunk.get(at..at + n)?;
        at += n;
        Some(bytes)
    };
    let int = |b: &[u8]| i32::from_le_bytes(b.try_into().unwrap());
    let v = int(next(4)?);
    if v == 123_456 || v <= -12 {
        return None;
    }
    let mut count = v;
    if v < -5 {
        next(1)?;
    }
    let mut program: i64 = if v < 0 { -3 } else { -1 };
    if v < -4 {
        let word = int(next(4)?);
        program = if word == -2 {
            i64::from_le_bytes(next(8)?.try_into().unwrap())
        } else {
            i64::from(word)
        };
    }
    if v < -5 {
        next(4)?;
    }
    if v == -2 || (v < -4 && (program == 0 || program == -1)) {
        count = int(next(4)?);
    }
    let count = usize::try_from(count).ok()?;
    let floats = next(count.checked_mul(4)?)?;
    let floats = floats.as_chunks::<4>().0;
    Some(floats.iter().map(|b| f32::from_le_bytes(*b)).collect())
}

/// A parameter's integer from the VST's value, as FL hands it over: the
/// loader scales it by 2^30 into a `REC_FromMIDI` value, rounding half to
/// even (`0xf46490`, the effect said yes to `FPD_UseIncreasedMIDIResolution`),
/// and the effect's `ProcessParam` scales that back across its control's
/// range, `min + Round(value * 2^-30 * (max - min))`, rounding the same
/// way.
pub fn from_vst(value: f32, (min, max): (i32, i32)) -> i32 {
    const SCALE: f64 = (1u64 << 30) as f64;
    // `cvtsd2si` into 64 bits, of which the call keeps the low 32. A value
    // it cannot hold comes out as the 64-bit integer indefinite.
    let wide = f64::from(value * SCALE as f32).round_ties_even();
    let midi = if wide.is_finite() && wide.abs() < 9.2e18 {
        wide as i64 as i32
    } else {
        0
    };
    let step = (f64::from(midi) / SCALE * f64::from(max.wrapping_sub(min))).round_ties_even();
    let step = if step.is_finite() && step.abs() < 9.2e18 {
        step as i64 as i32
    } else {
        0
    };
    min.wrapping_add(step)
}

/// The tag and the length at `at`.
fn header(bytes: &[u8], at: usize) -> Result<(u32, u64), StateError> {
    let tag = u32::from_le_bytes(word(bytes, at)?);
    let len = bytes
        .get(at + 4..at + 12)
        .and_then(|b| b.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or(short(bytes, at + 12))?;
    Ok((tag, len))
}

fn word(bytes: &[u8], at: usize) -> Result<[u8; 4], StateError> {
    bytes
        .get(at..at + 4)
        .and_then(|b| b.try_into().ok())
        .ok_or(short(bytes, at + 4))
}

fn short(bytes: &[u8], wanted: usize) -> StateError {
    StateError::Length {
        expected: wanted,
        found: bytes.len(),
    }
}

/// Fruity Wrapper. A new one holds nothing and passes a block through; its
/// state puts the effect it held in it.
#[derive(Default)]
pub struct Wrapper {
    inner: Option<Box<dyn Effect>>,
    held: Option<&'static str>,
    sample_rate: Option<f32>,
    tempo: Option<f64>,
    ppq: Option<u16>,
    resource_dir: Option<std::path::PathBuf>,
}

impl Wrapper {
    /// The name of the effect the wrapper holds now.
    pub fn held(&self) -> Option<&'static str> {
        self.held
    }

    /// The effect a state puts in place of the VST it names, with that
    /// VST's parameters. `None` when FL's loader would not swap it, or the
    /// effect is not built.
    fn swap(&self, wrapped: &Wrapped) -> Option<(&'static str, Box<dyn Effect>)> {
        let name = NATIVE
            .iter()
            .find(|native| native.eq_ignore_ascii_case(&wrapped.name))?;
        if wrapped.kind != 0 {
            return None;
        }
        let mut effect = crate::by_name(name)?;
        // The ranges, from a second one so that the first is moved only
        // to the values FL moves it to.
        let mut probe = crate::by_name(name)?;
        if let Some(hz) = self.sample_rate {
            effect.set_sample_rate(hz);
        }
        if let Some(bpm) = self.tempo {
            effect.set_tempo(bpm);
        }
        if let Some(ppq) = self.ppq {
            effect.set_ppq(ppq);
        }
        // FL moves as many as both have, and a chunk without parameters
        // leaves the defaults.
        let params = wrapped.params.as_deref().unwrap_or(&[]);
        for (index, value) in params.iter().enumerate().take(effect.param_count()) {
            probe.set_param(index, f32::MIN);
            let min = probe.param(index) as i32;
            probe.set_param(index, f32::MAX);
            let max = probe.param(index) as i32;
            effect.set_param(index, from_vst(*value, (min, max)) as f32);
        }
        Some((name, effect))
    }
}

impl Effect for Wrapper {
    /// A state that wraps an effect FL swaps in and that is built here puts
    /// that effect in the wrapper. Any other is turned down.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let wrapped = read(bytes)?;
        let (name, mut effect) = self.swap(&wrapped).ok_or(StateError::Unhosted)?;
        if let Some(dir) = &self.resource_dir {
            effect.set_resource_dir(dir);
        }
        self.inner = Some(effect);
        self.held = Some(name);
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        self.sample_rate = Some(hz);
        if let Some(inner) = &mut self.inner {
            inner.set_sample_rate(hz);
        }
    }

    fn set_tempo(&mut self, bpm: f64) {
        self.tempo = Some(bpm);
        if let Some(inner) = &mut self.inner {
            inner.set_tempo(bpm);
        }
    }

    fn set_ppq(&mut self, ppq: u16) {
        self.ppq = Some(ppq);
        if let Some(inner) = &mut self.inner {
            inner.set_ppq(ppq);
        }
    }

    fn set_resource_dir(&mut self, dir: &Path) {
        self.resource_dir = Some(dir.to_owned());
        if let Some(inner) = &mut self.inner {
            inner.set_resource_dir(dir);
        }
    }

    fn tick(&mut self, ppq_tick: u64, tempo: f64) {
        if let Some(inner) = &mut self.inner {
            inner.tick(ppq_tick, tempo);
        }
    }

    fn follows_song(&self) -> bool {
        self.inner
            .as_ref()
            .is_some_and(|inner| inner.follows_song())
    }

    fn controller_outputs(&self) -> &[f32] {
        self.inner
            .as_ref()
            .map_or(&[], |inner| inner.controller_outputs())
    }

    fn controller_changed(&mut self) -> bool {
        self.inner
            .as_mut()
            .is_some_and(|inner| inner.controller_changed())
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if let Some(inner) = &mut self.inner {
            inner.process(left, right);
        }
    }

    fn param_count(&self) -> usize {
        self.inner.as_ref().map_or(0, |inner| inner.param_count())
    }

    fn param(&self, index: usize) -> f32 {
        self.inner.as_ref().map_or(0.0, |inner| inner.param(index))
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(inner) = &mut self.inner {
            inner.set_param(index, value);
        }
    }

    fn reset(&mut self) {
        if let Some(inner) = &mut self.inner {
            inner.reset();
        }
    }

    fn fidelity(&self) -> Fidelity {
        self.inner
            .as_ref()
            .map_or(Fidelity::Exact, |inner| inner.fidelity())
    }

    fn latency(&self) -> usize {
        self.inner.as_ref().map_or(0, |inner| inner.latency())
    }

    fn max_latency(&self) -> usize {
        self.inner.as_ref().map_or(0, |inner| inner.max_latency())
    }

    fn aux_output(&self) -> Option<(&[f32], &[f32])> {
        self.inner.as_ref().and_then(|inner| inner.aux_output())
    }

    fn sidechain_output(&self) -> usize {
        self.inner
            .as_ref()
            .map_or(0, |inner| inner.sidechain_output())
    }

    fn sidechain_input(&self) -> usize {
        self.inner
            .as_ref()
            .map_or(0, |inner| inner.sidechain_input())
    }

    fn sidechain_inputs(&self) -> [usize; 2] {
        self.inner
            .as_ref()
            .map_or([0, 0], |inner| inner.sidechain_inputs())
    }

    fn set_sidechain(&mut self, index: usize, left: &[f32], right: &[f32]) {
        if let Some(inner) = &mut self.inner {
            inner.set_sidechain(index, left, right);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{corpus, projects};

    /// A wrapper state of version 8 with these tags.
    fn state(tags: &[(u32, &[u8])]) -> Vec<u8> {
        let mut bytes = 8u32.to_le_bytes().to_vec();
        for (tag, data) in tags {
            bytes.extend(tag.to_le_bytes());
            bytes.extend((data.len() as u64).to_le_bytes());
            bytes.extend(*data);
        }
        bytes
    }

    /// The chunk FL 11's VSTs save: -9, a byte, a program number 0, a
    /// word, the count, the parameters, then what is not read.
    fn chunk(params: &[f32]) -> Vec<u8> {
        let mut bytes = (-9i32).to_le_bytes().to_vec();
        bytes.push(5);
        bytes.extend(0i32.to_le_bytes());
        bytes.extend(0i32.to_le_bytes());
        bytes.extend((params.len() as i32).to_le_bytes());
        bytes.extend(params.iter().flat_map(|p| p.to_le_bytes()));
        bytes.extend(1i32.to_le_bytes());
        bytes.extend([0; 24]);
        bytes
    }

    fn wrapping(name: &str, params: &[f32]) -> Vec<u8> {
        let chunk = chunk(params);
        state(&[
            (TAG_KIND, &[0; 16]),
            (0x33, b"laBF"),
            (TAG_NAME, name.as_bytes()),
            (TAG_CHUNK, &chunk),
        ])
    }

    #[test]
    fn both_chunk_forms_give_their_parameters() {
        let params = [0.29296875, 0.6437];
        assert_eq!(chunk_params(&chunk(&params)).unwrap(), params);
        // FL 12's: the program number is -2 and then 64 bits.
        let mut bytes = (-9i32).to_le_bytes().to_vec();
        bytes.push(5);
        bytes.extend((-2i32).to_le_bytes());
        bytes.extend(0i64.to_le_bytes());
        bytes.extend(0i32.to_le_bytes());
        bytes.extend(2i32.to_le_bytes());
        bytes.extend(params.iter().flat_map(|p| p.to_le_bytes()));
        assert_eq!(chunk_params(&bytes).unwrap(), params);
        // A plain count first.
        let mut plain = 1i32.to_le_bytes().to_vec();
        plain.extend(0.5f32.to_le_bytes());
        assert_eq!(chunk_params(&plain).unwrap(), [0.5]);
        // No chunk, and one cut short.
        assert_eq!(chunk_params(&123_456i32.to_le_bytes()), None);
        assert_eq!(chunk_params(&(-12i32).to_le_bytes()), None);
        assert_eq!(chunk_params(&chunk(&params)[..20]), None);
    }

    #[test]
    fn a_vst_value_crosses_the_control_s_range_rounding_half_to_even() {
        assert_eq!(from_vst(0.0, (-128, 128)), -128);
        assert_eq!(from_vst(1.0, (-128, 128)), 128);
        assert_eq!(from_vst(0.29296875, (-128, 128)), -53);
        // Fruity Balance's render test: 0x3f24ccd0 is volume 206.
        assert_eq!(from_vst(f32::from_bits(0x3f24_ccd0), (0, 320)), 206);
        // Half a step is rounded to the even one.
        assert_eq!(from_vst(0.5, (0, 1)), 0);
        assert_eq!(from_vst(0.25, (0, 6)), 2);
        assert_eq!(from_vst(0.75, (0, 2)), 2);
        // What `cvtsd2si` cannot hold comes out as 0 steps.
        assert_eq!(from_vst(f32::NAN, (4, 300)), 4);
    }

    #[test]
    fn a_wrapped_balance_is_fl_s_balance_with_the_vst_s_values() {
        let mut wrapper = Wrapper::default();
        wrapper.set_sample_rate(44_100.0);
        let bytes = wrapping("Fruity Balance", &[0.29296875, 0.6437]);
        wrapper.set_state(&bytes).unwrap();
        assert_eq!(wrapper.held(), Some("Fruity Balance"));
        assert_eq!(wrapper.param_count(), 2);
        assert_eq!([wrapper.param(0), wrapper.param(1)], [-53.0, 206.0]);
        assert_eq!(wrapper.fidelity(), Fidelity::Exact);

        // The same as the Balance itself moved to those values.
        let mut balance = crate::balance::Balance::default();
        balance.set_param(0, -53.0);
        balance.set_param(1, 206.0);
        let tone: Vec<f32> = (0..64).map(|n| (n as f32 * 0.3).sin()).collect();
        let (mut l1, mut r1) = (tone.clone(), tone.clone());
        let (mut l2, mut r2) = (tone.clone(), tone);
        wrapper.process(&mut l1, &mut r1);
        balance.process(&mut l2, &mut r2);
        assert_eq!((l1, r1), (l2, r2));
    }

    #[test]
    fn fewer_parameters_leave_the_rest_at_their_defaults() {
        let mut wrapper = Wrapper::default();
        wrapper
            .set_state(&wrapping("Fruity Filter", &[0.0]))
            .unwrap();
        let defaults = crate::by_name("Fruity Filter").unwrap();
        assert_eq!(wrapper.param(0), 0.0);
        for index in 1..defaults.param_count() {
            assert_eq!(wrapper.param(index), defaults.param(index));
        }
        // More than the effect has are let go.
        let mut wrapper = Wrapper::default();
        let bytes = wrapping("Fruity Fast LP", &[1.0, 0.0, 0.5]);
        wrapper.set_state(&bytes).unwrap();
        assert_eq!((wrapper.param(0), wrapper.param(1)), (1024.0, 0.0));
    }

    #[test]
    fn what_fl_does_not_swap_or_is_not_built_is_turned_down() {
        let mut wrapper = Wrapper::default();
        let mut refused = |bytes: &[u8]| wrapper.set_state(bytes).unwrap_err();
        assert_eq!(refused(&wrapping("Sylenth1", &[0.5])), StateError::Unhosted);
        // One of FL's own the loader swaps, but not built here.
        let phaser = wrapping("Fruity Phaser", &[0.5]);
        assert_eq!(refused(&phaser), StateError::Unhosted);
        // Only under kind 0.
        let chunk = chunk(&[0.5]);
        let other = state(&[
            (TAG_KIND, &[2; 16]),
            (TAG_NAME, b"Fruity Balance"),
            (TAG_CHUNK, &chunk),
        ]);
        assert_eq!(refused(&other), StateError::Unhosted);
        let found = refused(&13u32.to_le_bytes());
        assert_eq!(found, StateError::Version { found: 13 });
        let cut = &wrapping("Fruity Balance", &[])[..30];
        assert!(matches!(refused(cut), StateError::Length { .. }));
        assert_eq!(wrapper.held(), None);
    }

    /// Every Fruity Wrapper in FL's projects holds one of FL's own effects
    /// in its FL 11, 12 or 13 VST build, with parameters from 0 to 1; the
    /// ones built here are hosted.
    #[test]
    fn every_wrapper_in_fl_s_projects_reads() {
        let Some(root) = corpus() else {
            return;
        };
        let saved = projects(&root);
        let wrappers: Vec<_> = saved.iter().filter(|s| s.plugin == NAME).collect();
        assert_eq!(wrappers.len(), 36);
        let mut hosted = 0;
        for saved in wrappers {
            let file = &saved.file;
            let wrapped = read(&saved.state).unwrap();
            assert_eq!(wrapped.kind, 0, "{file}");
            assert!(NATIVE.contains(&wrapped.name.as_str()), "{file}");
            let params = wrapped.params.unwrap();
            let unit = |p: &f32| (0.0..=1.0).contains(p);
            assert!(params.iter().all(unit), "{file}");
            let mut wrapper = Wrapper::default();
            if crate::by_name(&wrapped.name).is_some() {
                wrapper.set_state(&saved.state).unwrap();
                hosted += 1;
            } else {
                let refused = wrapper.set_state(&saved.state);
                assert_eq!(refused, Err(StateError::Unhosted), "{file}");
            }
        }
        assert_eq!(hosted, 23);
    }
}
