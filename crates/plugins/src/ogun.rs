//! Ogun, FL's metallic additive synth, as `docs/plugins/ogun.md` reads it
//! out of `Ogun_x64.dll`.
//!
//! Autogun is Ogun with random presets on top: the two DLLs carry the same
//! state loader, timbre engine, voice, articulators and post chain,
//! function for function. So Ogun plays on [`Autogun`] of [`Kind::Ogun`],
//! which differs where the DLLs do: Ogun saves no random-preset seed, has
//! no magic dust, and its master level is four times Autogun's. Its
//! presets go back to state version 0, which the shared loader reads as
//! `SaveRestoreState` does.

use crate::autogun::{Autogun, Kind, State};
use crate::{Fidelity, Generator, NoteOn, StateError};
use std::path::Path;

/// The plugin's event 201 name.
pub const NAME: &str = "Ogun";

/// Ogun.
pub struct Ogun(Autogun);

impl Default for Ogun {
    fn default() -> Self {
        Ogun(Autogun::new(Kind::Ogun))
    }
}

impl Ogun {
    pub fn state(&self) -> &State {
        self.0.state()
    }
}

impl Generator for Ogun {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        self.0.set_state(bytes)
    }

    fn set_sample_rate(&mut self, hz: f32) {
        self.0.set_sample_rate(hz);
    }

    fn set_tempo(&mut self, bpm: f64) {
        self.0.set_tempo(bpm);
    }

    fn set_resource_dir(&mut self, dir: &Path) {
        self.0.set_resource_dir(dir);
    }

    fn note_on(&mut self, note: NoteOn) {
        self.0.note_on(note);
    }

    fn note_off(&mut self, voice: u32) {
        self.0.note_off(voice);
    }

    fn all_notes_off(&mut self) {
        self.0.all_notes_off();
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.0.process(left, right);
    }

    fn param_count(&self) -> usize {
        self.0.param_count()
    }

    fn param(&self, index: usize) -> f32 {
        self.0.param(index)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        self.0.set_param(index, value);
    }

    fn reset(&mut self) {
        self.0.reset();
    }

    fn tail(&self) -> usize {
        self.0.tail()
    }

    fn latency(&self) -> usize {
        self.0.latency()
    }

    fn fidelity(&self) -> Fidelity {
        self.0.fidelity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{corpus, every_preset, projects};

    fn note(key: i16) -> NoteOn {
        NoteOn {
            voice: 1,
            key,
            velocity: 1.0,
            pan: 0.0,
            fine_pitch: 0.0,
            release: 0.5,
            mod_x: 0.0,
            mod_y: 0.0,
            slide: false,
            porta: false,
        }
    }

    fn render(plugin: &mut dyn Generator, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (vec![0.0; frames], vec![0.0; frames]);
        plugin.process(&mut l, &mut r);
        (l, r)
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    /// Both plugins at the same small engine and seed.
    fn small(kind: Kind) -> Autogun {
        let mut plugin = Autogun::new(kind);
        plugin.set_param(11, 77.0);
        plugin.set_sample_rate(48_000.0);
        plugin
    }

    #[test]
    fn ogun_s_master_level_is_four_times_autogun_s() {
        // `FUN_0086a570` keeps `(3^x - 1) / 2`; Autogun's `FUN_00882700`
        // takes a quarter of it. Nothing else differs.
        let mut ogun = small(Kind::Ogun);
        let mut autogun = small(Kind::Autogun);
        ogun.note_on(note(60));
        autogun.note_on(note(60));
        let (o, _) = render(&mut ogun, 9600);
        let (a, _) = render(&mut autogun, 9600);
        for (x, y) in o.iter().zip(&a).skip(4800) {
            assert!((x - 4.0 * y).abs() <= 1e-5 * x.abs().max(1e-3), "{x} {y}");
        }
        assert!(rms(&o[4800..]) > 0.01);
    }

    #[test]
    fn ogun_has_no_magic_dust_and_needs_no_file() {
        let mut ogun = Ogun::default();
        ogun.set_param(11, 77.0);
        ogun.set_resource_dir(Path::new("/nowhere"));
        assert_eq!(ogun.latency(), 0);
        assert_eq!(
            ogun.fidelity(),
            Fidelity::Approximate("Ogun: timbre engine not null-tested")
        );
    }

    #[test]
    fn every_state_fl_ships_parses_to_the_last_byte() {
        let Some(root) = corpus() else { return };
        let mut saved = projects(&root);
        saved.extend(every_preset(&root, NAME));
        let ours: Vec<_> = saved
            .iter()
            .filter(|s| s.plugin.eq_ignore_ascii_case(NAME))
            .collect();
        // 23 in projects, 233 in presets, from version 0 to 19.
        assert!(ours.len() >= 250, "{}", ours.len());
        let mut versions = std::collections::BTreeSet::new();
        for s in ours {
            let state =
                State::parse_as(&s.state, Kind::Ogun).unwrap_or_else(|e| panic!("{}: {e}", s.file));
            versions.insert(state.version);
            assert_eq!(state.seed, 0, "{}", s.file);
            assert!(
                (6..=16).contains(&state.order),
                "{}: {}",
                s.file,
                state.order
            );
        }
        assert_eq!(versions.first(), Some(&0));
        assert_eq!(versions.last(), Some(&19));
    }

    #[test]
    fn every_project_state_plays_a_sane_note() {
        let Some(root) = corpus() else { return };
        for s in projects(&root)
            .iter()
            .filter(|s| s.plugin.eq_ignore_ascii_case(NAME))
        {
            let mut plugin = Ogun::default();
            plugin.set_sample_rate(48_000.0);
            plugin.set_state(&s.state).unwrap();
            plugin.note_on(note(60));
            let (l, r) = render(&mut plugin, 24_000);
            assert!(l.iter().chain(&r).all(|v| v.is_finite()), "{}", s.file);
            let level = rms(&l).max(rms(&r));
            let peak = l.iter().chain(&r).fold(0.0f32, |m, v| m.max(v.abs()));
            eprintln!("{}: rms {level:.4} peak {peak:.4}", s.file);
            assert!(level > 0.003 && peak < 8.0, "{}: {level} {peak}", s.file);
        }
    }

    #[test]
    fn every_preset_plays_without_nan() {
        let Some(root) = corpus() else { return };
        for s in every_preset(&root, NAME) {
            let mut plugin = Ogun::default();
            plugin.set_sample_rate(44_100.0);
            plugin.set_state(&s.state).unwrap();
            plugin.note_on(note(60));
            let (l, r) = render(&mut plugin, 4410);
            assert!(l.iter().chain(&r).all(|v| v.is_finite()), "{}", s.file);
        }
    }
}
