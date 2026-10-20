//! The instrument tab of FL's Delphi generators, played as 3x Osc plays
//! it (`docs/plugins/3x-osc.md`), for the other generators that have the
//! same tab: five envelope and LFO blocks for the pan, the volume, mod X,
//! mod Y and the pitch, and a filter after the voice.
//!
//! FL links the same unit into each of these plugins: Fruity
//! Granulizer's copy of it is the voice engine at `0x7451f0` (a tick of
//! a voice's envelopes, `0x745680`, and its pan, volume and filter over a
//! block, `0x7456c0`) with the pan law at `0x60db70` and the ramp limit at
//! `0x60da50`, which are 3x Osc's.

use crate::NoteOn;
use crate::fruit_kick::Articulation;
use crate::three_osc::{
    Block, Envelope, FILTERS, Filter, HostTables, Kind, MOD_X, MOD_Y, OVER, PITCH, RISE, VOLUME,
    fade_out, pan_law, ramp,
};

/// The tab worked out for a rate and a tempo.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Tab {
    blocks: [Block; 5],
    filter: (Kind, bool),
    cutoff: f32,
    resonance: f32,
    /// `44100 / rate`.
    s: f32,
    rate: f32,
    tempo: f64,
    ppq: f64,
    /// The 10 ms a note with no volume envelope fades out by.
    fade: Vec<f32>,
}

impl Tab {
    /// This allocates.
    pub(crate) fn new(tab: &Articulation, rate: f32, tempo: f64, ppq: f64) -> Self {
        HostTables::get();
        Tab {
            blocks: std::array::from_fn(|k| Block::of(&tab.blocks[k], tempo, ppq)),
            filter: FILTERS[tab.filter.clamp(0, 7) as usize],
            cutoff: tab.cutoff as f32 * (1.0 / 256.0),
            resonance: tab.resonance as f32 * (1.0 / 256.0),
            s: (44_100.0 / f64::from(rate)) as f32,
            rate,
            tempo,
            ppq,
            fade: fade_out(rate),
        }
    }

    /// The samples of a tick of the song.
    pub(crate) fn tick_len(&self) -> f64 {
        (f64::from(self.rate) * 60.0 / (self.tempo * self.ppq)).max(1.0)
    }

    /// The most frames a note sounds for after its note off: the volume
    /// envelope's release where that is on, the tick it may wait for, and
    /// the fade.
    pub(crate) fn tail(&self) -> usize {
        let volume = &self.blocks[VOLUME];
        let mut ticks = 1.0;
        if volume.enabled {
            ticks += (-1.0 / f64::from(volume.release)).ceil() + 1.0;
        }
        (ticks * self.tick_len()).ceil() as usize + self.fade.len() + 1
    }
}

/// The song's ticks as the tab counts them, and the LFOs the notes share.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct Clock {
    /// The samples to the next tick, and the part of a sample the tick
    /// after it is later by.
    pub(crate) to_tick: usize,
    part: f64,
    pub(crate) globals: [u32; 5],
}

impl Clock {
    /// Start the next tick when the last is over. Whether one started
    /// comes back.
    pub(crate) fn tick(&mut self, tab: &Tab) -> bool {
        if self.to_tick != 0 {
            return false;
        }
        self.part += tab.tick_len();
        self.to_tick = self.part as usize;
        self.part -= self.to_tick as f64;
        for (global, block) in self.globals.iter_mut().zip(&tab.blocks) {
            *global = global.wrapping_add(block.lfo_step);
        }
        true
    }
}

/// What the tab does to one note: its envelopes, its filter and its gains.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Articulator {
    /// The note's own pan, gain and filter offsets.
    pan: f32,
    vol: f32,
    cut: f32,
    res: f32,
    envelopes: [Envelope; 5],
    filter: Filter,
    /// The gains now, what a sample adds to them, and where that gets
    /// them by the next tick.
    gains: [f32; 2],
    gain_steps: [f32; 2],
    gain_ends: [f32; 2],
    /// It has played nothing yet.
    first: bool,
    /// A tick has passed since it last took its envelopes' levels.
    pub(crate) stale: bool,
    ticked: bool,
    /// How far into the fade out it is, once that has started.
    fade: Option<usize>,
}

impl Articulator {
    pub(crate) fn new(note: &NoteOn, tab: &Tab) -> Self {
        let (kind, twice) = tab.filter;
        Articulator {
            pan: note.pan,
            vol: note.velocity,
            cut: note.mod_x,
            res: note.mod_y,
            envelopes: std::array::from_fn(|k| Envelope::new(&tab.blocks[k])),
            filter: Filter::new(kind, twice),
            gains: [0.0; 2],
            gain_steps: [0.0; 2],
            gain_ends: [0.0; 2],
            first: true,
            stale: true,
            ticked: false,
            fade: None,
        }
    }

    /// A tick of the song has passed.
    pub(crate) fn tick(&mut self) {
        (self.stale, self.ticked) = (true, true);
    }

    /// Start the releases. A note with no volume envelope fades out.
    pub(crate) fn release(&mut self, tab: &Tab) {
        for (envelope, block) in self.envelopes.iter_mut().zip(&tab.blocks).skip(VOLUME) {
            envelope.release(block);
        }
        if !tab.blocks[VOLUME].enabled {
            self.fade.get_or_insert(0);
        }
    }

    /// Take what the envelopes are at for the `len` samples to the next
    /// tick. The cents the pitch block moves the note by come back.
    pub(crate) fn begin(&mut self, tab: &Tab, globals: &[u32; 5], len: usize) -> f32 {
        let starts_full = self.envelopes[VOLUME].stage > RISE;
        if self.first || self.ticked {
            let shapes = HostTables::get();
            for (k, envelope) in self.envelopes.iter_mut().enumerate() {
                envelope.step(&tab.blocks[k], globals[k], shapes);
            }
            if self.envelopes[VOLUME].stage == OVER {
                self.fade.get_or_insert(0);
            }
        }
        let [pan, volume, mod_x, mod_y, pitch] = &self.envelopes;
        let blocks = &tab.blocks;
        let pan_mod = pan.lfo * blocks[0].lfo_amount * 2.0;
        let level = (volume.level * (volume.lfo * blocks[VOLUME].lfo_amount + 1.0)).max(0.0);
        let moved = |envelope: &Envelope, block: &Block| {
            envelope.level * block.amount + envelope.lfo * block.lfo_amount
        };
        let cut = self.cut + tab.cutoff + moved(mod_x, &blocks[MOD_X]);
        let res = self.res + tab.resonance + moved(mod_y, &blocks[MOD_Y]);
        let cents = pitch.level * blocks[PITCH].cents + pitch.lfo * blocks[PITCH].lfo_cents;

        let scale = self.filter.set(cut, res, tab.s, self.first, len);
        let ends = pan_law(self.pan + pan_mod, self.vol * level).map(|gain| gain * scale);
        let from = match (self.first, starts_full) {
            (true, true) => ends,
            (true, false) => [0.0; 2],
            (false, _) => self.gain_ends,
        };
        for side in 0..2 {
            (self.gain_steps[side], self.gain_ends[side]) =
                ramp(ends[side], from[side], tab.s * 0.001, len);
        }
        self.gains = from;
        (self.first, self.stale, self.ticked) = (false, false, false);
        cents
    }

    /// Whether the note has faded out.
    pub(crate) fn over(&self, tab: &Tab) -> bool {
        self.fade.is_some_and(|at| at >= tab.fade.len())
    }

    /// Put the voice's frames through the filter and the gains and add
    /// them to `left` and `right`. Whether the note is over comes back.
    pub(crate) fn run(
        &mut self,
        tab: &Tab,
        voice: (&[f32], &[f32]),
        left: &mut [f32],
        right: &mut [f32],
    ) -> bool {
        let frames = voice.0.iter().zip(voice.1);
        for ((l, r), (vl, vr)) in left.iter_mut().zip(right.iter_mut()).zip(frames) {
            if self.over(tab) {
                return true;
            }
            let mut frame = self.filter.run([*vl, *vr]);
            if let Some(at) = &mut self.fade {
                frame = frame.map(|value| value * tab.fade[*at]);
                *at += 1;
            }
            *l += frame[0] * self.gains[0];
            *r += frame[1] * self.gains[1];
            self.gains[0] += self.gain_steps[0];
            self.gains[1] += self.gain_steps[1];
        }
        self.over(tab)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::three_osc::new_tab;

    #[test]
    fn a_new_tab_plays_a_note_at_the_pan_law_s_gain_and_fades_it_out_in_10_ms() {
        let tab = Tab::new(&new_tab(), 44_100.0, 120.0, 96.0);
        let mut clock = Clock::default();
        assert!(clock.tick(&tab));
        assert!(!clock.tick(&tab));
        let mut note = Articulator::new(&NoteOn::default(), &tab);
        assert_eq!(note.begin(&tab, &clock.globals, clock.to_tick), 0.0);
        let ones = [1.0f32; 64];
        let (mut l, mut r) = ([0.0f32; 64], [0.0f32; 64]);
        assert!(!note.run(&tab, (&ones, &ones), &mut l, &mut r));
        let centre = std::f32::consts::FRAC_1_SQRT_2;
        assert!((l[63] - centre).abs() < 1e-6 && (r[63] - centre).abs() < 1e-6);
        note.release(&tab);
        let mut out = vec![0.0f32; 1000];
        let mut other = vec![0.0f32; 1000];
        let ones = vec![1.0f32; 1000];
        assert!(note.run(&tab, (&ones, &ones), &mut out, &mut other));
        assert_eq!(tab.fade.len(), 441);
        assert!(out[0] > 0.7 && out[440] < 0.01 && out[441] == 0.0);
    }
}
