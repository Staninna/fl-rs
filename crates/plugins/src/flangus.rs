//! Fruity Flangus: up to nine delayed copies of each side, each delay swung
//! by its own slow LFO, mixed back under the dry signal, as
//! `docs/plugins/fruity-flangus.md` has it.
//!
//! The state is 32 bytes with no version: eight little-endian i32, the
//! plugin's parameters.
//!
//! | parameter | range | default |
//! |---|---|---|
//! | 0 Order | 1 to 9 | 4 |
//! | 1 Depth | 0 to 1024 | 512 |
//! | 2 Speed | 0 to 1024 | 768 |
//! | 3 Delay | 0 to 128 | 8 |
//! | 4 Spread | 0 to 128 | 128 |
//! | 5 Cross | -128 to 128 | -32 |
//! | 6 Dry level | -128 to 128 | 96 |
//! | 7 Wet level | -128 to 128 | 96 |
//!
//! The order is how many voices read each side. A voice is a tap on a
//! delay line of its own side, linear between two samples, whose delay goes
//! from `max(centre, 8)` samples to that plus twice its depth and back, on
//! a curve that is flat at both ends. Depth and delay are the knob curve
//! `11^x - 1` times 109.25 and 441 samples at 44.1 kHz, and the first voice
//! has all of both, the last `1 / order` of them. The LFO goes at the speed
//! over the depth, so that every voice bends the pitch about as far. The
//! spread gives each voice a rate, a centre and a starting phase of its own
//! from a table of 20 numbers. At no spread all voices keep one rate and
//! the two sides are half a cycle apart.
//!
//! A voice plays at 1 into its own side and at the cross times its share
//! into the other, and all of them are scaled by what the sum of those
//! gains comes to. The dry and the wet level are the knob curve
//! `(11^x - 1) / 10` with the wheel's sign. Nothing feeds back, and the
//! plugin is on time.
//!
//! FL has two routines for a voice and picks one by its CPU. This is the
//! x87 one, which cuts the delay down to a whole sample. The other rounds
//! it, and nothing in the DLL was found to choose it.

use crate::fl_fx::{FlangerSettings, StereoFlanger};
use crate::parts::glide;
use crate::{Effect, StateError, knob_gain, knob_value, read_state};

/// The plugin's event 201 name.
pub const NAME: &str = "Fruity Flangus";

/// The range of each parameter's wheel.
const RANGES: [(i32, i32); 8] = [
    (1, 9),
    (0, 1024),
    (0, 1024),
    (0, 128),
    (0, 128),
    (-128, 128),
    (-128, 128),
    (-128, 128),
];
const DEFAULTS: [i32; 8] = [4, 512, 768, 8, 128, -32, 96, 96];

/// The gain of a level wheel: the knob curve with the wheel's sign, and 1
/// at 128 without working it out, as in FL.
pub fn level(value: i32) -> f32 {
    let size = if value.abs() == 128 {
        1.0
    } else {
        knob_gain(f64::from(value.abs()) / 128.0) as f32
    };
    size * value.signum() as f32
}

/// Fruity Flangus. A new one has FL's defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct Flangus {
    params: [i32; 8],
    hz: f64,
    /// The delay lines and their voices, FL's `TStereoFlanger`.
    flanger: StereoFlanger,
    /// The dry gain and the wet gain of each side, as the last block left
    /// them.
    dry: f32,
    wet: [f32; 2],
}

impl Default for Flangus {
    fn default() -> Self {
        let mut plugin = Flangus {
            params: DEFAULTS,
            hz: 44_100.0,
            flanger: StereoFlanger::default(),
            dry: 0.0,
            wet: [0.0; 2],
        };
        plugin.flanger.make_room(plugin.hz);
        plugin.set(DEFAULTS);
        plugin.settle();
        plugin
    }
}

impl Flangus {
    /// Take parameters that are inside their ranges, or a new rate.
    ///
    /// FL's delay line is as long as its longest delay, and it starts over
    /// when that length changes: what it held is gone and every LFO is back
    /// where it starts. So does this. Moving the spread always does that,
    /// and the speed and the cross never do.
    fn set(&mut self, params: [i32; 8]) {
        let wheels = [0, 1, 2, 3, 4, 5].map(|i| params[i]);
        // The class's own output gain is 1: the plugin has its wheels.
        let settings = FlangerSettings::from_wheels(wheels, 1.0);
        self.flanger.set(&settings, self.hz);
        self.params = params;
    }

    /// Put the dry and the wet gain where the wheels have them, with no
    /// ramp.
    fn settle(&mut self) {
        self.dry = level(self.params[6]);
        self.wet = [level(self.params[7]); 2];
    }
}

impl Effect for Flangus {
    /// The levels are the state's from the first sample on, with no ramp.
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        self.set(read_state(bytes, &RANGES)?);
        self.settle();
        Ok(())
    }

    /// Asks for memory when the rings for the rate are of another length.
    /// A rate that is not a number above 0 is not taken.
    fn set_sample_rate(&mut self, hz: f32) {
        if hz.is_finite() && hz > 0.0 && f64::from(hz) != self.hz {
            self.hz = f64::from(hz);
            self.flanger.make_room(self.hz);
            self.set(self.params);
        }
    }

    /// After a level wheel has moved, its gain goes to where the wheel has
    /// it in a straight line over this block, by no more than FL's cap a
    /// sample.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        if len == 0 {
            return;
        }
        let cap = (44_100.0 / self.hz * 0.001) as f32;
        let (mut dry, dry_step) = glide(&mut self.dry, level(self.params[6]), cap, len);
        let wet = level(self.params[7]);
        let [wet_left, wet_right] = &mut self.wet;
        let (mut wet_l, wet_l_step) = glide(wet_left, wet, cap, len);
        let (mut wet_r, wet_r_step) = glide(wet_right, wet, cap, len);
        // FL adds no wet signal at a wet level of 0 that is not moving.
        let heard = [wet_l, wet_l_step, wet_r, wet_r_step] != [0.0; 4];

        let frames = left[..len].iter_mut().zip(&mut right[..len]);
        for (l, r) in frames {
            let (sum_l, sum_r) = self.flanger.frame(*l, *r);
            (*l, *r) = (*l * dry, *r * dry);
            if heard {
                *l += sum_l * wet_l;
                *r += sum_r * wet_r;
            }
            dry += dry_step;
            wet_l += wet_l_step;
            wet_r += wet_r_step;
        }
    }

    fn param_count(&self) -> usize {
        RANGES.len()
    }

    fn param(&self, index: usize) -> f32 {
        self.params.get(index).map_or(0.0, |value| *value as f32)
    }

    /// Moving the order, the depth, the delay or the spread starts the
    /// delay lines over where FL's do: see [`Flangus::set`].
    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(range) = RANGES.get(index) {
            let mut params = self.params;
            params[index] = knob_value(value, *range);
            self.set(params);
        }
    }

    /// Forgets what came in, puts every LFO where it starts and ends a
    /// ramp, as FL's flush does.
    fn reset(&mut self) {
        self.flanger.reset();
        self.settle();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fl_fx::{FlangerVoice as Voice, flanger_room as room};
    use crate::testing::{bits, every_preset, presets, sine, state};

    fn with(params: [i32; 8]) -> Flangus {
        let mut plugin = Flangus::default();
        plugin.set_state(&state(&params)).unwrap();
        plugin
    }

    /// The voices of a plugin that play.
    fn voices(plugin: &Flangus) -> &[Voice] {
        &plugin.flanger.voices[..plugin.flanger.count]
    }

    /// The shortest and the longest delay of a voice, in samples.
    fn swing(voice: &Voice) -> (f32, f32) {
        (voice.centre - voice.depth, voice.centre + voice.depth)
    }

    fn near(got: f32, wanted: f32) -> bool {
        (got - wanted).abs() < 1e-3
    }

    /// How many times a second a voice's LFO turns.
    fn turns(voice: &Voice, hz: f64) -> f64 {
        f64::from(voice.step) / 4_294_967_296.0 * hz
    }

    #[test]
    fn a_level_wheel_is_the_knob_curve_with_its_sign() {
        assert_eq!((level(128), level(-128), level(0)), (1.0, -1.0, 0.0));
        assert!((level(96) - 0.504).abs() < 5e-4);
        assert!((level(64) - (11f32.sqrt() - 1.0) / 10.0).abs() < 1e-6);
        assert_eq!(level(-96), -level(96));
    }

    #[test]
    fn with_the_wet_at_nothing_the_input_comes_out_at_the_dry_level() {
        let (left, right) = (sine(0.011, 3000), sine(0.027, 3000));
        // At 128 the dry level is 1: the input to the bit.
        let mut plugin = with([4, 512, 768, 8, 128, -32, 128, 0]);
        let (mut l, mut r) = (left.clone(), right.clone());
        for (l, r) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            plugin.process(l, r);
        }
        assert_eq!((bits(&l), bits(&r)), (bits(&left), bits(&right)));
        assert_eq!(plugin.latency(), 0);

        // Elsewhere it is the curve, and under 0 the input upside down.
        for wheel in [96, 17, -64, -128] {
            let mut plugin = with([9, 1024, 1024, 128, 128, 128, wheel, 0]);
            let (mut l, mut r) = (left.clone(), right.clone());
            plugin.process(&mut l, &mut r);
            let gain = level(wheel);
            for n in 0..left.len() {
                assert_eq!((l[n], r[n]), (left[n] * gain, right[n] * gain));
            }
        }
    }

    #[test]
    fn a_level_that_does_not_move_passes_through_every_voice() {
        // However the delays swing, a level that stays put comes out of a
        // tap as it went in. So once the lines are full, each side is the
        // input times the dry level and the wet level times the gains of
        // the voices that play into it.
        for params in [
            [1, 0, 512, 0, 0, 0, 96, 96],
            [4, 512, 768, 8, 128, -32, 96, 96],
            [7, 19, 985, 0, 0, 0, 128, 82],
            [9, 1024, 1024, 128, 128, 128, -40, 128],
        ] {
            let mut plugin = with(params);
            let into = |side: usize| -> f32 {
                let voices = voices(&plugin).iter();
                voices.map(|voice| voice.gains[side]).sum()
            };
            let want = [0, 1].map(|side| level(params[6]) + level(params[7]) * into(side));
            let (mut l, mut r) = (vec![0.5; 9000], vec![0.5; 9000]);
            plugin.process(&mut l, &mut r);
            for (side, out) in [l, r].iter().enumerate() {
                let apart = out[8000..].iter().map(|v| (v - 0.5 * want[side]).abs());
                assert!(apart.fold(0.0, f32::max) < 1e-5, "{params:?}");
            }
        }
    }

    #[test]
    fn the_default_voices_are_the_doc_s() {
        let plugin = Flangus::default();
        let voices = voices(&plugin);
        assert_eq!((voices.len(), plugin.flanger.longest), (8, 601));
        // Four on the left, then four on the right from the smallest share
        // up, each at 1 into its own side and a quarter of its share upside
        // down into the other, all times 0.5267.
        let norm = 0.5267;
        for (i, voice) in voices.iter().enumerate() {
            assert_eq!(voice.right, i >= 4);
            let share = (if i < 4 { 4 - i } else { i - 3 }) as f32 / 4.0;
            let (own, other) = (
                voice.gains[usize::from(voice.right)],
                voice.gains[usize::from(!voice.right)],
            );
            assert!((own - norm).abs() < 1e-4, "voice {i}");
            assert!((other + 0.25 * share * norm).abs() < 1e-4, "voice {i}");
        }
        // The swings and the rates the doc's model gives.
        let wanted = [
            (66.7, 592.9, 0.1842, 0x8000_0000u32),
            (57.7, 457.4, 0.2800, 0x6000_0000),
            (37.2, 310.3, 0.3956, 0x4000_0000),
            (8.0, 154.5, 0.0073, 0x2000_0000),
            (17.0, 163.6, 0.6750, 0xa000_0000),
            (38.0, 311.1, 0.4050, 0xc000_0000),
            (53.2, 452.8, 0.2578, 0xe000_0000),
            (67.2, 593.4, 0.1857, 0x0000_0000),
        ];
        for (voice, (short, long, hz, start)) in voices.iter().zip(wanted) {
            let (from, to) = swing(voice);
            assert!((from - short).abs() < 0.06 && (to - long).abs() < 0.06);
            assert!((turns(voice, 44_100.0) - hz).abs() < 6e-5);
            assert_eq!(voice.start as u32, start);
        }
    }

    #[test]
    fn with_no_spread_every_voice_has_one_rate_and_the_sides_are_half_a_cycle_apart() {
        // "Vibrate": 14 voices at 11.4 Hz, from 8 samples to 38 and less.
        let plugin = with([7, 19, 985, 0, 0, 0, 128, 82]);
        let voices = voices(&plugin);
        assert_eq!(voices.len(), 14);
        for voice in voices {
            assert!((turns(voice, 44_100.0) - 11.433).abs() < 1e-3);
            assert_eq!(voice.start, if voice.right { i32::MIN } else { 0 });
            assert!(near(swing(voice).0, 8.0));
        }
        assert!((swing(&voices[0]).1 - 37.9).abs() < 0.06);
        assert!((swing(&voices[6]).1 - 29.4).abs() < 0.06);
        // Two voices come to 0.969, the two with all the cross to 0.823,
        // and 18 with all the cross to 0.219.
        let two = with([1, 136, 312, 0, 0, 0, 0, 128]);
        assert!((two.flanger.voices[0].gains[0] - 0.9694).abs() < 1e-4);
        let crossed = with([1, 136, 312, 0, 0, 128, 0, 128]);
        assert_eq!(
            crossed.flanger.voices[0].gains,
            crossed.flanger.voices[1].gains
        );
        assert!((crossed.flanger.voices[0].gains[0] - 0.823).abs() < 1e-4);
        let many = with([9, 1024, 1024, 128, 128, 128, 0, 128]);
        assert!((many.flanger.voices[0].gains[0] - 0.2189).abs() < 1e-4);
        assert_eq!((many.flanger.count, many.flanger.longest), (18, 7084));
    }

    #[test]
    fn a_voice_s_delay_swings_between_its_ends_once_a_cycle() {
        // One voice a side and the wet alone. A ramp comes out of a tap as
        // the ramp at the delayed time, so the delay can be read off it.
        let mut plugin = with([1, 136, 1024, 0, 0, 0, 0, 128]);
        let voice = plugin.flanger.voices[0];
        let (short, long) = swing(&voice);
        let cycle = 4_294_967_296.0 / f64::from(voice.step);
        assert!(
            near(short, 8.0) && long > 100.0 && cycle < 20_000.0,
            "{cycle}"
        );

        let len = 2 * cycle as usize + 400;
        let ramp: Vec<f32> = (0..len).map(|n| n as f32 / 65_536.0).collect();
        let (mut l, mut r) = (ramp.clone(), ramp.clone());
        plugin.process(&mut l, &mut r);
        let gain = voice.gains[0];
        let delay = |n: usize| (ramp[n] - l[n] / gain) * 65_536.0;
        // The left side starts at its shortest, and is at its longest half
        // a cycle on and back after a whole one.
        let at = |cycles: f64| (cycles * cycle).round() as usize;
        assert!((delay(20) - short).abs() < 0.5, "{}", delay(20));
        assert!((delay(at(0.5)) - long).abs() < 0.5, "{}", delay(at(0.5)));
        assert!((delay(at(1.0)) - short).abs() < 0.5);
        assert!((delay(at(1.5)) - long).abs() < 0.5);
        let delays: Vec<f32> = (20..len).map(delay).collect();
        let (least, most) = delays
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), d| (a.min(*d), b.max(*d)));
        assert!(least > short - 0.5 && most < long + 0.5, "{least} {most}");
        // The right side is half a cycle on: at its longest at the start.
        let right = |n: usize| (ramp[n] - r[n] / voice.gains[0]) * 65_536.0;
        assert!((right(at(1.0)) - long).abs() < 0.5);
        assert!((right(at(0.5)) - short).abs() < 0.5);
    }

    #[test]
    fn a_click_comes_out_of_a_voice_between_two_samples() {
        // One voice a side that hardly moves. After 16,880 samples of
        // nothing its delay is about 8 samples and a half.
        let mut plugin = with([1, 0, 0, 0, 0, 0, 0, 128]);
        assert!(turns(&plugin.flanger.voices[0], 44_100.0) < 0.2);
        plugin.process(&mut vec![0.0; 16_880], &mut vec![0.0; 16_880]);
        let mut voice = plugin.flanger.voices[0];
        let delays: Vec<f64> = (0..10).map(|_| voice.delay()).collect();
        assert!((delays[8] - 8.5).abs() < 0.05, "{}", delays[8]);

        let (mut l, mut r) = (vec![0.0; 64], vec![0.0; 64]);
        l[0] = 1.0;
        plugin.process(&mut l, &mut r);
        // Nothing on the right, and on the left the click 8 and 9 samples
        // on, shared out by where between the two the delay is.
        assert!(r.iter().all(|v| *v == 0.0));
        let hits: Vec<usize> = (0..64).filter(|n| l[*n] != 0.0).collect();
        assert_eq!(hits, [8, 9]);
        let gain = f64::from(voice.gains[0]);
        assert!((f64::from(l[8]) - gain * (9.0 - delays[8])).abs() < 1e-6);
        assert!((f64::from(l[9]) - gain * (delays[9] - 8.0)).abs() < 1e-6);
    }

    #[test]
    fn the_cross_puts_a_side_s_voices_into_the_other_side() {
        let run = |cross: i32| {
            let mut plugin = with([2, 86, 632, 0, 0, cross, 0, 128]);
            let (mut l, mut r) = (vec![0.25; 600], vec![0.0; 600]);
            plugin.process(&mut l, &mut r);
            (l[599], r[599], plugin.flanger.voices[0].gains[0])
        };
        // With none, the right stays silent.
        let (left, right, norm) = run(0);
        assert!((left - 0.25 * 2.0 * norm).abs() < 1e-6 && right == 0.0);
        // With all of it, the right has the first voice whole and the
        // second by half, and at -128 the same upside down.
        let (left, right, norm) = run(128);
        assert!((left - 0.25 * 2.0 * norm).abs() < 1e-6);
        assert!((right - 0.25 * 1.5 * norm).abs() < 1e-6);
        let (_, right, norm) = run(-128);
        assert!((right + 0.25 * 1.5 * norm).abs() < 1e-6);
    }

    #[test]
    fn the_delays_scale_with_the_rate_and_the_lfos_keep_their_time() {
        let mut plugin = Flangus::default();
        let at_44 = plugin.flanger.voices;
        plugin.set_sample_rate(88_200.0);
        assert_eq!(plugin.flanger.ring[0].len(), 2 * room(44_100.0));
        for (slow, fast) in at_44.iter().zip(&plugin.flanger.voices).take(8) {
            assert!((fast.depth - 2.0 * slow.depth).abs() < 1e-3);
            assert!((turns(fast, 88_200.0) - turns(slow, 44_100.0)).abs() < 1e-4);
        }
        // The shortest delay is 8 samples at any rate.
        assert!(near(swing(&plugin.flanger.voices[3]).0, 8.0));
        // A rate that is not one leaves the plugin as it is.
        let before = plugin.clone();
        plugin.set_sample_rate(f32::NAN);
        plugin.set_sample_rate(0.0);
        assert_eq!(plugin, before);
    }

    #[test]
    fn the_lines_start_over_when_their_length_or_the_spread_changes() {
        let mut plugin = Flangus::default();
        plugin.process(&mut [0.5; 512], &mut [0.5; 512]);
        let phases = |plugin: &Flangus| -> Vec<i32> {
            let voices = voices(plugin).iter();
            voices.map(|voice| voice.phase).collect()
        };
        let starts = |plugin: &Flangus| -> Vec<i32> {
            let voices = voices(plugin).iter();
            voices.map(|voice| voice.start).collect()
        };
        let held = |plugin: &Flangus| plugin.flanger.ring[0].iter().any(|v| *v != 0.0);
        assert!(held(&plugin) && phases(&plugin) != starts(&plugin));

        // The speed, the cross and the levels leave them be.
        for (index, value) in [(2, 300.0), (5, 100.0), (6, 0.0), (7, 128.0)] {
            plugin.set_param(index, value);
            assert!(held(&plugin) && phases(&plugin) != starts(&plugin));
        }
        // The depth makes the longest delay another, and the spread always
        // starts them over.
        for (index, value) in [(1, 600.0), (4, 127.0), (3, 60.0), (0, 5.0)] {
            plugin.process(&mut [0.5; 64], &mut [0.5; 64]);
            assert!(held(&plugin));
            plugin.set_param(index, value);
            assert!(!held(&plugin), "parameter {index}");
            assert_eq!(phases(&plugin), starts(&plugin), "parameter {index}");
        }
        // And so does a reset.
        plugin.process(&mut [0.5; 64], &mut [0.5; 64]);
        plugin.reset();
        assert!(!held(&plugin) && phases(&plugin) == starts(&plugin));
    }

    #[test]
    fn a_moved_level_ramps_and_a_moved_wheel_asks_for_no_memory() {
        let mut plugin = with([4, 512, 768, 8, 128, -32, 128, 0]);
        let memory = |plugin: &Flangus| {
            (
                plugin.flanger.ring[0].as_ptr(),
                plugin.flanger.ring[1].as_ptr(),
            )
        };
        let before = memory(&plugin);
        // The cap is 0.001 a sample at 44.1 kHz, so 100 samples take the
        // dry level a tenth of the way down.
        plugin.set_param(6, 0.0);
        let (mut l, mut r) = ([1.0; 100], [1.0; 100]);
        plugin.process(&mut l, &mut r);
        assert_eq!(l[0], 1.0);
        assert!((l[99] - 0.901).abs() < 1e-4 && l[99] == r[99]);

        let moves = [9.4, 2000.0, -3.0, 128.0, 0.0, -128.2, 77.5, f32::NAN];
        for (index, value) in moves.into_iter().enumerate() {
            plugin.set_param(index, value);
            plugin.process(&mut [0.1; 300], &mut [0.1; 300]);
        }
        plugin.set_param(8, 1.0);
        assert_eq!((plugin.param(8), plugin.param_count()), (0.0, 8));
        assert_eq!(
            [0, 1, 2, 3, 4, 5, 6, 7].map(|i| plugin.param(i)),
            [9.0, 1024.0, 0.0, 128.0, 0.0, -128.0, 78.0, 0.0]
        );
        assert_eq!(memory(&plugin), before);
        plugin.reset();
        assert_eq!(plugin, with([9, 1024, 0, 128, 0, -128, 78, 0]));
    }

    #[test]
    fn a_state_the_plugin_does_not_write_is_turned_down() {
        let mut plugin = with([2, 86, 632, 0, 0, 0, 0, 120]);
        let before = plugin.clone();
        for len in [0, 31, 28, 36] {
            assert_eq!(
                plugin.set_state(&vec![0; len]),
                Err(StateError::Length {
                    expected: 32,
                    found: len
                })
            );
        }
        for (param, value) in [(0, 0), (0, 10), (1, 1025), (3, -1), (5, 129), (7, -129)] {
            let mut params = DEFAULTS;
            params[param] = value;
            assert_eq!(
                plugin.set_state(&state(&params)),
                Err(StateError::Range { param, value })
            );
        }
        assert_eq!(plugin, before);
        assert_eq!(with(DEFAULTS), Flangus::default());
    }

    #[test]
    fn fl_s_presets_load_as_the_doc_reads_them() {
        let Some(own) = presets(NAME) else { return };
        // 12 of the plugin's own and the three of the plugin database.
        let all = every_preset(&crate::testing::corpus().unwrap(), NAME);
        assert_eq!((own.len(), all.len()), (12, 15));
        let named = |file: &str| {
            let preset = own.iter().find(|p| p.file == file);
            let preset = preset.unwrap_or_else(|| panic!("no preset {file}"));
            let mut plugin = Flangus::default();
            plugin.set_state(&preset.state).unwrap();
            plugin
        };
        for preset in &all {
            assert_eq!(preset.state.len(), 32, "{}", preset.file);
            let mut plugin = Flangus::default();
            plugin.set_state(&preset.state).unwrap();
        }
        assert_eq!(named("Default"), Flangus::default());
        assert_eq!(named("Classic").params, [4, 512, 892, 8, 128, -32, 88, 128]);
        assert_eq!(
            named("Flanger 2").params,
            [1, 136, 312, 0, 0, 128, 128, -128]
        );
        assert_eq!(named("For drums").params[6..], [-128, 128]);
        let thick = named("Thick");
        let (short, long) = swing(&thick.flanger.voices[0]);
        assert!(near(short, 8.0) && near(long, 2213.0), "{short} {long}");
        let stereoized = named("Stereoized");
        assert_eq!(stereoized.flanger.count, 4);
        assert!((turns(&stereoized.flanger.voices[0], 44_100.0) - 0.775).abs() < 1e-3);
    }
}
