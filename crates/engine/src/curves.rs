//! How FL's stored values turn into playback speed and gain.

use std::f64::consts::{FRAC_PI_4, SQRT_2};

/// The curves a sampler voice and the mixer are built from. The voice and
/// mix code call these and know nothing about their shape, so curves read
/// from FL's binary can replace [`Approximate`] without touching either.
pub trait Curves {
    /// Playback speed for a note `semitones` above the sample's own pitch.
    fn pitch_ratio(&self, semitones: f64) -> f64;
    /// Gain for a note velocity as the note record stores it: 0..=128,
    /// where a new note has 100.
    fn velocity_gain(&self, velocity: u8) -> f32;
    /// Gain for a channel volume as the levels struct stores it: 0..=12800,
    /// where a new channel has 10000.
    fn volume_gain(&self, volume: u32) -> f32;
    /// Gain for a channel's volume adjustment as its 229 record stores it
    /// at @4: 12800 adjusts nothing. Curves that do not say leave it out.
    fn volume_adjust_gain(&self, _raw: i32) -> f32 {
        1.0
    }
    /// What an audio clip's own gain is multiplied by, on top of what a
    /// note gets. Curves that do not say play a clip as they play a note
    /// at their unity.
    fn clip_gain(&self) -> f32 {
        1.0
    }
    /// Left and right gain for a pan from -1 (left) to 1 (right).
    fn pan_gains(&self, pan: f32) -> (f32, f32);
    /// Gain for an insert volume, or for the main volume, as its 225 record
    /// stores it: 0..=16000, where 12800 is 0 dB.
    fn insert_gain(&self, raw: i32) -> f32;
    /// Left and right gain for an insert pan as its 225 record stores it:
    /// -6400 (left) to 6400 (right).
    fn insert_pan(&self, raw: i32) -> (f32, f32);
    /// What an insert pan feeds across: the gain of the right input in the
    /// left output, and of the left input in the right output. Curves that
    /// do not say feed nothing across.
    fn insert_cross(&self, _raw: i32) -> (f32, f32) {
        (0.0, 0.0)
    }
    /// The direct gains and the gains across of an insert pan and stereo
    /// separation, as their 225 records store them: the pan -6400..=6400
    /// and the separation -64..=64, where below 0 separates and above 0
    /// merges. Without a separation that is [`Curves::insert_pan`] and
    /// [`Curves::insert_cross`].
    ///
    /// FL adds the separation's share to the share of each side's gain that
    /// its pan feeds across, up to all of it, and splits that side's gain
    /// anew (`FUN_0127b050`). Curves that do not say do that to the shares
    /// their own pan has.
    fn insert_spread(&self, pan: i32, separation: i32) -> ((f32, f32), (f32, f32)) {
        let (direct, across) = (self.insert_pan(pan), self.insert_cross(pan));
        if separation == 0 {
            return (direct, across);
        }
        let share = separation_share(separation);
        let side = |direct: f32, across: f32| {
            let gain = direct + across;
            let fed = if gain == 0.0 {
                0.0
            } else {
                across / gain * 2.0
            };
            split(gain, (fed + share).min(1.0))
        };
        let (left, left_across) = side(direct.0, across.0);
        let (right, right_across) = side(direct.1, across.1);
        ((left, right), (left_across, right_across))
    }
    /// Gain for a send level as its 225 record stores it: 0..=16000, where
    /// 12800 is unity.
    fn send_gain(&self, raw: i32) -> f32;
    /// How much of an effect slot's output is the plugin's, for a mix level
    /// as its 225 record stores it: 0..=12800, where 12800 is all of it.
    /// The rest is the slot's input.
    fn slot_mix(&self, raw: i32) -> f32;
    /// How far a tracking knob moves a level, for the knob as a channel's
    /// 228 record stores it: -256..=256, where a new channel has 0. Curves
    /// that do not say leave tracking out.
    fn tracking_amount(&self, _raw: i32) -> f32 {
        0.0
    }
}

/// What a stereo separation adds to the share `f` of each side's gain that
/// an insert feeds across, for the separation as its 225 record stores
/// it, -64..=64. `FUN_0127b050`, in its floats:
///
/// ```text
/// raw < 0:  raw / 64                         down to -1, which separates
/// raw > 0:  (5^(raw / 64) - 1) / 4           up to 1, which merges
/// ```
///
/// A side then keeps `1 - f / 2` of its gain and feeds `f / 2` across,
/// so at 64 both outputs are the mean of the two inputs, and at -64 each
/// is its own input at 1.5 less the other at 0.5. FL's hint calls below
/// 0 "separated" and above 0 "merged" (`FUN_006a4ac0`).
pub(crate) fn separation_share(raw: i32) -> f32 {
    let at = raw.clamp(-64, 64) as f32 * (1.0 / 64.0);
    match raw {
        0 => 0.0,
        ..0 => at,
        _ => ((f64::from(at) * 5f64.ln()).exp() - 1.0) as f32 * 0.25,
    }
}

/// A side's gain split into what it keeps and what it feeds across, for
/// the share `f` of `FUN_0127b050`.
fn split(gain: f32, share: f32) -> (f32, f32) {
    let half = share * 0.5;
    (gain * (1.0 - half), gain * half)
}

/// The raw value of a mixer fader at 0 dB.
const UNITY: i32 = 12_800;
/// The raw value at the top of a mixer fader.
const TOP: i32 = 16_000;
/// The gain at the top of a mixer fader, in dB. [`FlExact`] gives 5.59, and
/// an Image-Line forum post about FL 9 reads +5.6 dB off the hint bar there.
const TOP_DB: f32 = 5.6;

/// Plain curves that are not FL's. Pitch is equal temperament. Velocity and
/// volume are linear with the value FL gives a new note or channel at unity.
/// Pan is `dsp::pan_linear`, which leaves a centred sample at full level on
/// both sides.
///
/// Insert volume and send level are a straight line in dB through 0 dB at
/// 12800 and 5.6 dB at 16000, where the fader ends. The line is at -22.4 dB
/// when it gets to 0, and FL's fader is silent there, so 0 is silence here
/// too. The step up from it is not FL's. A slot's mix level is on the same
/// line and ends at 12800.
#[derive(Debug, Clone, Copy, Default)]
pub struct Approximate;

impl Approximate {
    fn fader(raw: i32) -> f32 {
        if raw <= 0 {
            return 0.0;
        }
        let db = (raw.min(TOP) - UNITY) as f32 * TOP_DB / (TOP - UNITY) as f32;
        10f32.powf(db / 20.0)
    }
}

impl Curves for Approximate {
    fn pitch_ratio(&self, semitones: f64) -> f64 {
        2f64.powf(semitones / 12.0)
    }

    fn velocity_gain(&self, velocity: u8) -> f32 {
        f32::from(velocity) / 100.0
    }

    fn volume_gain(&self, volume: u32) -> f32 {
        volume as f32 / 10_000.0
    }

    fn pan_gains(&self, pan: f32) -> (f32, f32) {
        dsp::pan_linear(pan)
    }

    fn insert_gain(&self, raw: i32) -> f32 {
        Self::fader(raw)
    }

    fn insert_pan(&self, raw: i32) -> (f32, f32) {
        dsp::pan_linear((raw as f32 / 6400.0).clamp(-1.0, 1.0))
    }

    fn send_gain(&self, raw: i32) -> f32 {
        Self::fader(raw)
    }

    fn slot_mix(&self, raw: i32) -> f32 {
        Self::fader(raw.min(UNITY))
    }
}

/// The curves read from FL's binary: the faders from `docs/rec-225.md`, and
/// velocity, channel volume and pan from `docs/plugins/host-voice.md` and
/// the functions named here.
///
/// Insert volume, main volume and send level are
///
/// ```text
/// gain = (exp(raw / 12800 * K) - 1) * 0.1
/// ```
///
/// with 0 and 12800 hard-wired to 0 and 1, as `FUN_0127b050` has them. The
/// send handler in the REC dispatcher computes the same thing. `K` is the
/// float at `0x015f49ec`, which FL sets to `(float)ln 11` when it starts
/// (`0x6a6925`, in the initialiser that also sets ln 2 and ln 601). That puts
/// 16000, where the fader ends, at 5.59 dB. Values past 16000 read as
/// 16000.
///
/// A slot's mix level is taken to be on the same curve up to 12800, where
/// its knob ends. `docs/rec-225.md` has the range of that record and not
/// its curve.
///
/// Velocity is the curve `FUN_00d29aa0` fills `InitLevels.Vol` with:
///
/// ```text
/// gain = (11^(velocity / 100) - 1) / 10
/// ```
///
/// with 100 hard-wired to 1. That is the fader's curve with the 100 of a
/// new note where the fader has 12800.
///
/// A channel's volume is the fader's curve twice (`FUN_00fe9d20`, which
/// fills the float at `chan+0x19c`): once for the volume of its 219 and
/// once for the adjustment at @4 of its 229, each over 12800. A new channel
/// has 10000 and 12800, which is 0.551. [`Curves::volume_gain`] is the
/// first and [`Curves::volume_adjust_gain`] the second.
///
/// Pan is FL's `ComputeLRVol` (`FUN_006a3a00`) with its circular law, which
/// FL uses unless its audio settings say triangular:
///
/// ```text
/// left = cos((1 + pan) * pi/4) * vol      right = sin((1 + pan) * pi/4) * vol
/// ```
///
/// An insert calls it with `raw / 6400` for the pan and its fader's gain
/// times `1 / sin(pi/4)` for `vol` (`FUN_0127b050`; the factor is the float
/// at `0x015f49a8`). So a centred insert is at its fader's gain on both
/// sides, and one panned all the way to a side has 3 dB more on that side.
/// `FUN_0127b050` then splits the gain of that side in two. With `far` for
/// the other side's gain over the fader's, which is 1 in the centre and 0
/// at a hard pan:
///
/// ```text
/// direct = gain * (1 - (1 - far) / 2)      across = gain * (1 - far) / 2
/// ```
///
/// [`Curves::insert_pan`] is the direct gains and [`Curves::insert_cross`]
/// the gains across. An insert panned all the way left puts both of its
/// inputs into its left output at -3 dB.
///
/// The share of the near side, `1 - far`, is the `f` a stereo separation
/// adds its own share to in [`Curves::insert_spread`], on both sides, and
/// `FUN_0127b050` stops it at 1. The gain across goes from the far input:
/// `0x6a13a0`, which runs the matrix, puts `+0x88` on the right input in
/// the left output and `+0x8c` on the left input in the right output.
///
/// A voice's pan is [`Curves::pan_gains`]: `ComputeLRVol` at the voice's
/// own level, with no factor and nothing across. So a note at velocity 100
/// in the centre of a new channel plays its sample at 0.551 / sqrt(2) of
/// the file's level, which is 0.39, or 8.2 dB down.
///
/// An audio clip is at the file's level there. `FUN_00ffa0a0`, which
/// starts the voice of a clip on the playlist, sets its `InitLevels.Vol` to
/// `(1 / sin(pi/4)) / 0.551`, and that is [`Curves::clip_gain`].
///
/// Where this is not FL, or may not be:
///
/// - FL's sampler is not read. A note and a clip are taken to go through
///   it alike, with `FinalLevels.Vol` and `ComputeLRVol` and nothing more.
/// - FL works an insert's gains out with its volume in them and glides
///   them to a new setting over a block (`FUN_0067fd90`). Here the
///   volume goes on after and a new setting holds from the block it is
///   read in.
/// - A tracking knob at 256 is taken to be 1. The scale of that knob is
///   not read.
/// - Pitch is [`Approximate`]'s.
/// - FL works the two sides out with a sine and a cosine of its own
///   (`FUN_006649c0`), so the last bit of a gain may not be FL's.
#[derive(Debug, Clone, Copy, Default)]
pub struct FlExact;

/// FL's `ComputeLRVol` with the circular law: left and right gain for a pan
/// from -1 (left) to 1 (right), which is kept inside that range, at the
/// level `vol`. The centre is `vol / sqrt(2)` on both sides.
fn compute_lr_vol(pan: f32, vol: f64) -> (f32, f32) {
    let pan = f64::from(pan.clamp(-1.0, 1.0));
    // cos((1 + pan) * pi/4) is sin((1 - pan) * pi/4). As two sines, the
    // far side of a hard pan is 0 and the two sides mirror each other.
    let side = |pan: f64| (((1.0 + pan) * FRAC_PI_4).sin() * vol) as f32;
    (side(-pan), side(pan))
}

impl FlExact {
    /// `(11^g - 1) / 10`: FL's curve for a fader at `g` of its unity.
    fn curve(g: f64) -> f64 {
        ((g * 11f64.ln()).exp() - 1.0) * 0.1
    }

    fn fader(raw: i32) -> f32 {
        match raw {
            ..=0 => 0.0,
            UNITY => 1.0,
            _ => Self::curve(f64::from(raw.min(TOP)) / f64::from(UNITY)) as f32,
        }
    }

    /// The direct gains of an insert pan, and the gains across.
    fn insert_matrix(raw: i32) -> ((f32, f32), (f32, f32)) {
        let (left, right) = compute_lr_vol(raw as f32 / 6400.0, SQRT_2);
        // The share of the near side's gain that goes across.
        let across = |far: f32| (1.0 - far) * 0.5;
        match raw {
            0 => ((left, right), (0.0, 0.0)),
            ..0 => {
                let share = across(right);
                ((left * (1.0 - share), right), (left * share, 0.0))
            }
            _ => {
                let share = across(left);
                ((left, right * (1.0 - share)), (0.0, right * share))
            }
        }
    }

    /// [`FlExact::insert_matrix`] with a stereo separation on it, the way
    /// `FUN_0127b050` has both: the pan gives the near side the share
    /// `1 - far`, the separation adds its own to the share of both sides,
    /// and no share goes past 1.
    fn spread_matrix(pan: i32, separation: i32) -> ((f32, f32), (f32, f32)) {
        let (left, right) = compute_lr_vol(pan as f32 / 6400.0, SQRT_2);
        let added = separation_share(separation);
        let left_share = match pan {
            ..0 => 1.0 - right,
            _ => 0.0,
        };
        let right_share = match pan {
            1.. => 1.0 - left,
            _ => 0.0,
        };
        let (left, left_across) = split(left, (left_share + added).min(1.0));
        let (right, right_across) = split(right, (right_share + added).min(1.0));
        ((left, right), (left_across, right_across))
    }
}

impl Curves for FlExact {
    fn pitch_ratio(&self, semitones: f64) -> f64 {
        Approximate.pitch_ratio(semitones)
    }

    fn velocity_gain(&self, velocity: u8) -> f32 {
        // In the floats FL has it in: 0.01 and ln 11 are f32 there.
        let v = f32::from(velocity) * 0.01;
        if v == 1.0 {
            return 1.0;
        }
        let ln_11 = 11f64.ln() as f32;
        ((f64::from(v * ln_11).exp() - 1.0) * f64::from(0.1f32)) as f32
    }

    fn volume_gain(&self, volume: u32) -> f32 {
        Self::curve(f64::from(volume) / f64::from(UNITY)) as f32
    }

    fn volume_adjust_gain(&self, raw: i32) -> f32 {
        match raw {
            ..=0 => 0.0,
            UNITY => 1.0,
            _ => Self::curve(f64::from(raw) / f64::from(UNITY)) as f32,
        }
    }

    fn clip_gain(&self) -> f32 {
        (SQRT_2 / Self::curve(10_000.0 / f64::from(UNITY))) as f32
    }

    fn pan_gains(&self, pan: f32) -> (f32, f32) {
        compute_lr_vol(pan, 1.0)
    }

    fn insert_gain(&self, raw: i32) -> f32 {
        Self::fader(raw)
    }

    fn insert_pan(&self, raw: i32) -> (f32, f32) {
        Self::insert_matrix(raw).0
    }

    fn insert_cross(&self, raw: i32) -> (f32, f32) {
        Self::insert_matrix(raw).1
    }

    fn insert_spread(&self, pan: i32, separation: i32) -> ((f32, f32), (f32, f32)) {
        match separation {
            0 => Self::insert_matrix(pan),
            _ => Self::spread_matrix(pan, separation),
        }
    }

    fn send_gain(&self, raw: i32) -> f32 {
        Self::fader(raw)
    }

    fn slot_mix(&self, raw: i32) -> f32 {
        Self::fader(raw.min(UNITY))
    }

    fn tracking_amount(&self, raw: i32) -> f32 {
        raw as f32 / 256.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_1_SQRT_2;

    #[test]
    fn an_octave_doubles_the_speed() {
        assert_eq!(Approximate.pitch_ratio(0.0), 1.0);
        assert!((Approximate.pitch_ratio(12.0) - 2.0).abs() < 1e-12);
        assert!((Approximate.pitch_ratio(-12.0) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn defaults_are_unity() {
        assert_eq!(Approximate.velocity_gain(100), 1.0);
        assert_eq!(Approximate.velocity_gain(50), 0.5);
        assert_eq!(Approximate.volume_gain(10_000), 1.0);
        assert_eq!(Approximate.pan_gains(0.0), (1.0, 1.0));
        assert_eq!(Approximate.pan_gains(-1.0), (1.0, 0.0));
    }

    fn db(gain: f32) -> f32 {
        20.0 * gain.log10()
    }

    #[test]
    fn both_faders_are_silent_at_0_and_unity_at_12800() {
        let curves: [&dyn Curves; 2] = [&Approximate, &FlExact];
        for curves in curves {
            assert_eq!(curves.insert_gain(0), 0.0);
            assert_eq!(curves.insert_gain(-5), 0.0);
            assert_eq!(curves.insert_gain(12_800), 1.0);
            assert_eq!(curves.send_gain(0), 0.0);
            assert_eq!(curves.send_gain(12_800), 1.0);
            assert!((db(curves.insert_gain(16_000)) - 5.6).abs() < 0.02);
            assert_eq!(curves.insert_gain(i32::MAX), curves.insert_gain(16_000));
            assert_eq!(curves.insert_pan(0), (1.0, 1.0));
        }
        assert_eq!(Approximate.insert_pan(-6400), (1.0, 0.0));
        assert_eq!(Approximate.insert_pan(6400), (0.0, 1.0));
        assert_eq!(Approximate.insert_pan(-99_999), (1.0, 0.0));
    }

    #[test]
    fn compute_lr_vol_is_the_circular_law() {
        // The centre is 1/sqrt(2) of the level on both sides.
        assert_eq!(compute_lr_vol(0.0, 1.0), (FRAC_1_SQRT_2, FRAC_1_SQRT_2));
        assert_eq!(
            compute_lr_vol(0.0, 0.5),
            (FRAC_1_SQRT_2 / 2.0, FRAC_1_SQRT_2 / 2.0)
        );
        // A hard pan is the level on one side and nothing on the other.
        assert_eq!(compute_lr_vol(-1.0, 1.0), (1.0, 0.0));
        assert_eq!(compute_lr_vol(1.0, 1.0), (0.0, 1.0));
        // cos(pi/8) and sin(pi/8) a quarter of the way to the left.
        let (left, right) = compute_lr_vol(-0.5, 1.0);
        assert!((left - 0.923_879_5).abs() < 1e-7 && (right - 0.382_683_43).abs() < 1e-7);
        // Every pan keeps the power, and a pan past the end is the end.
        for pan in [-1.0, -0.7, -0.25, 0.0, 0.1, 0.5, 1.0] {
            let (left, right) = compute_lr_vol(pan, 1.0);
            assert!((left * left + right * right - 1.0).abs() < 1e-6, "{pan}");
            assert_eq!(compute_lr_vol(-pan, 1.0), (right, left), "{pan}");
        }
        assert_eq!(compute_lr_vol(-7.0, 1.0), (1.0, 0.0));
        assert_eq!(compute_lr_vol(7.0, 1.0), (0.0, 1.0));
    }

    #[test]
    fn fl_s_voice_pan_is_compute_lr_vol_as_it_is() {
        // 3 dB down in the centre, and the level itself at a hard pan.
        assert_eq!(FlExact.pan_gains(0.0), (FRAC_1_SQRT_2, FRAC_1_SQRT_2));
        assert_eq!(FlExact.pan_gains(-1.0), (1.0, 0.0));
        assert_eq!(FlExact.pan_gains(1.0), (0.0, 1.0));
        // Half way to the right: cos(3 pi/8) and sin(3 pi/8).
        let (left, right) = FlExact.pan_gains(0.5);
        assert!((left - 0.382_683_43).abs() < 1e-6 && (right - 0.923_879_5).abs() < 1e-6);
    }

    #[test]
    fn fl_s_levels_put_a_new_note_8_db_down_and_a_clip_at_the_file_s_level() {
        // A note at velocity 100 in the centre of a new channel:
        // 0.551 / sqrt(2).
        let channel = FlExact.volume_gain(10_000) * FlExact.volume_adjust_gain(12_800);
        let note = FlExact.velocity_gain(100) * channel * FlExact.pan_gains(0.0).0;
        assert!((note - 0.3896).abs() < 1e-4);
        assert!((db(note) + 8.19).abs() < 0.01);
        // A clip has sqrt(2) / 0.551 for its volume, which is unity there.
        assert!((FlExact.clip_gain() - 2.5666).abs() < 1e-4);
        assert!((FlExact.clip_gain() * channel * FlExact.pan_gains(0.0).0 - 1.0).abs() < 1e-6);
        // The other curves play both at the file's level.
        assert_eq!(Approximate.clip_gain(), 1.0);
    }

    #[test]
    fn fl_s_insert_pan_feeds_the_far_input_into_the_near_side() {
        // The centre passes both sides as they are.
        assert_eq!(FlExact.insert_pan(0), (1.0, 1.0));
        assert_eq!(FlExact.insert_cross(0), (0.0, 0.0));
        // All the way left, the left output is both inputs at -3 dB.
        assert_eq!(FlExact.insert_pan(-6400), (FRAC_1_SQRT_2, 0.0));
        assert_eq!(FlExact.insert_cross(-6400), (FRAC_1_SQRT_2, 0.0));
        assert_eq!(FlExact.insert_pan(6400), (0.0, FRAC_1_SQRT_2));
        assert_eq!(FlExact.insert_cross(6400), (0.0, FRAC_1_SQRT_2));
        assert_eq!(FlExact.insert_pan(-99_999), FlExact.insert_pan(-6400));
        // Half way: the near side's gain is ComputeLRVol's, split by half
        // of what the far side is under 1.
        for raw in [-3200, 3200] {
            let (law_left, law_right) = compute_lr_vol(raw as f32 / 6400.0, SQRT_2);
            let (direct, across) = (FlExact.insert_pan(raw), FlExact.insert_cross(raw));
            let (near, far) = (law_left.max(law_right), law_left.min(law_right));
            let share = (1.0 - far) / 2.0;
            assert!((far - 0.541_196_1).abs() < 1e-6 && (near - 1.306_563).abs() < 1e-6);
            let want = (near * (1.0 - share), far, near * share, 0.0);
            let got = match raw {
                ..0 => (direct.0, direct.1, across.0, across.1),
                _ => (direct.1, direct.0, across.1, across.0),
            };
            assert_eq!(got, want, "{raw}");
            // A sound that is alike on both sides comes out at the law's
            // gains.
            assert!((got.0 + got.2 - near).abs() < 1e-6, "{raw}");
        }
        // The other curves feed nothing across.
        assert_eq!(Approximate.insert_cross(-6400), (0.0, 0.0));
    }

    #[test]
    fn separation_is_linear_below_0_and_exponential_above() {
        assert_eq!(separation_share(0), 0.0);
        assert_eq!(separation_share(-64), -1.0);
        assert_eq!(separation_share(-32), -0.5);
        assert_eq!(separation_share(64), 1.0);
        assert!((separation_share(32) - (5f32.sqrt() - 1.0) / 4.0).abs() < 1e-7);
        assert_eq!(separation_share(-99), -1.0);
        assert_eq!(separation_share(99), 1.0);
    }

    #[test]
    fn without_a_separation_the_spread_is_the_pan() {
        let curves: [&dyn Curves; 2] = [&Approximate, &FlExact];
        for curves in curves {
            for pan in [-6400, -3200, -1, 0, 1, 2500, 6400] {
                let want = (curves.insert_pan(pan), curves.insert_cross(pan));
                assert_eq!(curves.insert_spread(pan, 0), want, "{pan}");
            }
        }
    }

    #[test]
    fn a_separation_moves_each_side_s_share_across() {
        // In the centre, all the way merged is the mean on both sides and
        // all the way separated is 1.5 of a side less 0.5 of the other.
        let curves: [&dyn Curves; 2] = [&Approximate, &FlExact];
        for curves in curves {
            assert_eq!(curves.insert_spread(0, 64), ((0.5, 0.5), (0.5, 0.5)));
            assert_eq!(curves.insert_spread(0, -64), ((1.5, 1.5), (-0.5, -0.5)));
        }
        // On FL's curves a pan's share and the separation's add up, and
        // no side feeds more than half its gain across.
        let (law_left, law_right) = compute_lr_vol(-0.5, SQRT_2);
        let added = separation_share(16);
        let ((left, right), (left_across, right_across)) = FlExact.insert_spread(-3200, 16);
        let share = (1.0 - law_right + added).min(1.0);
        assert_eq!(left, law_left * (1.0 - share * 0.5));
        assert_eq!(left_across, law_left * share * 0.5);
        assert_eq!(right, law_right * (1.0 - added * 0.5));
        assert_eq!(right_across, law_right * added * 0.5);
        let ((left, _), (left_across, _)) = FlExact.insert_spread(-6400, 64);
        assert_eq!((left, left_across), (FRAC_1_SQRT_2, FRAC_1_SQRT_2));
        // The generic way gets FL's numbers from FL's pan, to the last bit
        // or so.
        struct Plain;
        impl Curves for Plain {
            fn pitch_ratio(&self, s: f64) -> f64 {
                FlExact.pitch_ratio(s)
            }
            fn velocity_gain(&self, v: u8) -> f32 {
                FlExact.velocity_gain(v)
            }
            fn volume_gain(&self, v: u32) -> f32 {
                FlExact.volume_gain(v)
            }
            fn pan_gains(&self, p: f32) -> (f32, f32) {
                FlExact.pan_gains(p)
            }
            fn insert_gain(&self, r: i32) -> f32 {
                FlExact.insert_gain(r)
            }
            fn insert_pan(&self, r: i32) -> (f32, f32) {
                FlExact.insert_pan(r)
            }
            fn insert_cross(&self, r: i32) -> (f32, f32) {
                FlExact.insert_cross(r)
            }
            fn send_gain(&self, r: i32) -> f32 {
                FlExact.send_gain(r)
            }
            fn slot_mix(&self, r: i32) -> f32 {
                FlExact.slot_mix(r)
            }
        }
        for (pan, separation) in [(-3200, 16), (4000, -20), (6400, 64), (-100, -64)] {
            let (fl, plain) = (
                FlExact.insert_spread(pan, separation),
                Plain.insert_spread(pan, separation),
            );
            let gains = |s: ((f32, f32), (f32, f32))| [s.0.0, s.0.1, s.1.0, s.1.1];
            for (fl, plain) in gains(fl).into_iter().zip(gains(plain)) {
                assert!(
                    (fl - plain).abs() < 1e-6,
                    "{pan} {separation}: {fl} {plain}"
                );
            }
        }
    }

    #[test]
    fn fl_s_channel_volume_is_the_fader_curve() {
        assert_eq!(FlExact.volume_gain(12_800), 1.0);
        assert_eq!(FlExact.volume_gain(0), 0.0);
        // (11^(v / 12800) - 1) / 10, which is 0.551 or -5.2 dB for the
        // 10000 of a new channel.
        let curve = |v: f64| (11f64.powf(v / 12_800.0) - 1.0) / 10.0;
        assert!((FlExact.volume_gain(10_000) - 0.551).abs() < 1e-4);
        for volume in [1u32, 2500, 5000, 9999, 10_000] {
            let got = f64::from(FlExact.volume_gain(volume));
            assert!(
                (got - curve(f64::from(volume))).abs() < 1e-6,
                "{volume}: {got}"
            );
        }
        assert_eq!(FlExact.volume_gain(6400), FlExact.insert_gain(6400));

        // The adjustment of a 229 is the same curve, with 12800 at unity.
        assert_eq!(FlExact.volume_adjust_gain(12_800), 1.0);
        assert_eq!(FlExact.volume_adjust_gain(0), 0.0);
        assert_eq!(FlExact.volume_adjust_gain(-1), 0.0);
        assert_eq!(FlExact.volume_adjust_gain(6400), FlExact.insert_gain(6400));
        assert!((f64::from(FlExact.volume_adjust_gain(16_594)) - curve(16_594.0)).abs() < 1e-6);
        // The other curves leave it out.
        assert_eq!(Approximate.volume_adjust_gain(6400), 1.0);
    }

    #[test]
    fn fl_s_velocity_is_unity_at_100() {
        assert_eq!(FlExact.velocity_gain(100), 1.0);
        assert_eq!(FlExact.velocity_gain(0), 0.0);
        // (11^(velocity / 100) - 1) / 10, at the velocities
        // docs/plugins/host-voice.md works out to two or three digits.
        for (velocity, gain) in [(128, 2.05), (64, 0.364), (1, 0.0024)] {
            let got = FlExact.velocity_gain(velocity);
            assert!((got - gain).abs() < gain * 0.02, "{velocity}: {got}");
            let exact = (11f64.powf(f64::from(velocity) / 100.0) - 1.0) / 10.0;
            assert!((f64::from(got) - exact).abs() < 1e-6, "{velocity}: {got}");
        }
        // FL's Voice_ProcessEvent gives a plugin the velocity back as
        // log11(10 * vol + 1) * 100 / 128.
        for velocity in [1u8, 37, 64, 100, 128] {
            let vol = f64::from(FlExact.velocity_gain(velocity));
            let back = (10.0 * vol + 1.0).ln() / 11f64.ln() * 100.0 / 128.0;
            assert!(
                (back - f64::from(velocity) / 128.0).abs() < 1e-6,
                "{velocity}"
            );
        }
    }

    #[test]
    fn only_fl_s_curves_have_tracking() {
        assert_eq!(Approximate.tracking_amount(256), 0.0);
        assert_eq!(FlExact.tracking_amount(256), 1.0);
        assert_eq!(FlExact.tracking_amount(-64), -0.25);
        assert_eq!(FlExact.tracking_amount(0), 0.0);
    }

    #[test]
    fn a_slot_s_mix_is_the_fader_up_to_unity() {
        let curves: [&dyn Curves; 2] = [&Approximate, &FlExact];
        for curves in curves {
            assert_eq!(curves.slot_mix(0), 0.0);
            assert_eq!(curves.slot_mix(-1), 0.0);
            assert_eq!(curves.slot_mix(12_800), 1.0);
            assert_eq!(curves.slot_mix(16_000), 1.0);
            assert_eq!(curves.slot_mix(6400), curves.send_gain(6400));
        }
        assert!((FlExact.slot_mix(6400) - (11f32.sqrt() - 1.0) / 10.0).abs() < 1e-6);
    }

    #[test]
    fn the_approximate_fader_is_a_straight_line_in_db() {
        assert!((db(Approximate.insert_gain(9600)) + 5.6).abs() < 1e-3);
        assert!((db(Approximate.insert_gain(6400)) + 11.2).abs() < 1e-3);
        assert!((db(Approximate.send_gain(14_400)) - 2.8).abs() < 1e-3);
    }

    #[test]
    fn the_fl_fader_is_the_exponential_from_rec_225() {
        // (11^(raw / 12800) - 1) / 10.
        assert!((FlExact.insert_gain(6400) - (11f32.sqrt() - 1.0) / 10.0).abs() < 1e-6);
        assert!((FlExact.insert_gain(16_000) - (11f32.powf(1.25) - 1.0) / 10.0).abs() < 1e-6);
        assert!((FlExact.send_gain(12_799) - 1.0).abs() < 1e-3);
        assert!(FlExact.send_gain(12_799) < 1.0 && FlExact.send_gain(12_801) > 1.0);
    }
}
