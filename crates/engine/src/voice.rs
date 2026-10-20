//! One voice: a sample read at a fixed speed into a stereo bus. A note on a
//! sampler channel is one, and so is an audio clip on the playlist.

use crate::Curves;
use crate::sample::Sample;
use dsp::resample::read_hermite;
use model::{Channel, Project};
use std::sync::Arc;

/// A channel's volume and pan as its 219 stores them: volume 0..=12800,
/// and pan 0..=12800 with 6400 in the centre (PyFLP; every sampler channel
/// in the corpus is inside both ranges). A channel without a 219 has the
/// 10000 and the centre of a new one.
pub(crate) fn levels(channel: &Channel) -> (u32, u32) {
    (
        channel.volume().unwrap_or(10_000),
        channel.pan().unwrap_or(6400),
    )
}

/// The channel's volume adjustment: the i32 at @4 of its 229
/// `LevelAdjusts`, which [`Curves::volume_adjust_gain`] takes. 12800
/// adjusts nothing, and that is what a channel without a 229 has. FL's
/// loader multiplies the stored value by 100 for a writer below 5.3.7, and
/// so does this. FL calls it the volume multiplier.
pub(crate) fn adjust(project: &Project, channel: &Channel) -> i32 {
    adjusts(project, channel).1
}

/// The channel's pan offset, the i32 at @0 of its 229, from -6400 to
/// 6400: what [`gains`] adds to the channel's pan. 0 for a channel without
/// a 229. A writer below 5.3.7 stores a hundredth, as for [`adjust`].
pub(crate) fn pan_offset(project: &Project, channel: &Channel) -> i32 {
    adjusts(project, channel).0
}

/// The pan offset and the volume multiplier of a channel's 229, as FL's
/// loader leaves them (`FUN_00f46490`, the 229 branch).
fn adjusts(project: &Project, channel: &Channel) -> (i32, i32) {
    let Some(stored) = channel.level_adjusts else {
        return (0, 12_800);
    };
    let version = project.writer_version.as_deref();
    let scale = match version.and_then(flp::event::version_number) {
        Some(..5_003_007) => 100,
        _ => 1,
    };
    let field = |bytes| i32::from_le_bytes(bytes).saturating_mul(scale);
    (field(stored.unknown_0), field(stored.unknown_4))
}

/// Left and right gain of one voice: its own level and its channel's
/// volume, then the channel's pan plus its own. `volume` is the channel's
/// from [`levels`], and `channel_pan` its pan from there plus its
/// [`pan_offset`], as `FUN_00fe9bf0` adds them. `level` is a note's
/// velocity gain or a clip's gain, and `pan` is the note's or the clip's,
/// from -1 to 1.
pub(crate) fn gains(
    volume: u32,
    channel_pan: i32,
    level: f32,
    pan: f32,
    curves: &dyn Curves,
) -> (f32, f32) {
    let gain = level * curves.volume_gain(volume);
    let channel_pan = channel_pan as f32 / 6400.0 - 1.0;
    let (left, right) = curves.pan_gains((channel_pan + pan).clamp(-1.0, 1.0));
    (gain * left, gain * right)
}

/// What a voice needs to work its gains and its speed out again when
/// automation moves its channel's volume, pan or pitch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Follow {
    /// The channel, by the index its 64 has.
    pub channel: u16,
    /// The voice's own level and pan, as [`gains`] takes them. The level
    /// is without the channel's volume multiplier, which automation may
    /// move as well.
    pub level: f32,
    pub pan: f32,
    /// For a voice whose channel has its pitch automated: the lowest
    /// speed the automation can take it to, as a share of the speed it was
    /// made with. `None` when the pitch stays.
    pub slowest: Option<f64>,
}

/// A clone shares the sample and starts where the original is.
#[derive(Clone)]
pub(crate) struct Voice {
    sample: Arc<Sample>,
    /// The loader's number of the insert the voice plays into.
    pub insert: u16,
    /// Output samples the voice lasts.
    frames: usize,
    /// Output samples it has played.
    done: usize,
    /// The source frame the first output sample reads.
    origin: f64,
    /// Source frames per output sample, below zero for a voice that reads
    /// backwards.
    step: f64,
    /// `step` as the voice was made, before automation moved its pitch.
    made: f64,
    /// The output sample `origin` is at: 0 until automation moves the
    /// pitch, and where it last moved it after.
    since: usize,
    /// The note's length in output samples, `None` for one that plays the
    /// sample to its end.
    held: Option<usize>,
    gains: (f32, f32),
    /// Set for a voice on a channel whose volume or pan a clip drives.
    follow: Option<Follow>,
    /// What automation and the note's slides make of the speed it was made
    /// with, which `step` is the product of.
    automated: f64,
    slid: f64,
    /// The note's slides: from its output sample `.0` on it plays at `.1`
    /// times the speed it was made with, before automation. The next is at
    /// `next_bend`.
    bends: Option<Arc<[(usize, f64)]>>,
    next_bend: usize,
}

impl Voice {
    /// `held` is the note's length in output samples, `None` for a note
    /// that plays the sample to its end. `step` is the pitch ratio times
    /// the sample's rate over the output rate.
    pub fn new(
        sample: Arc<Sample>,
        insert: u16,
        held: Option<usize>,
        step: f64,
        gains: (f32, f32),
    ) -> Self {
        let whole = (sample.left.len() as f64 / step).ceil() as usize;
        Voice {
            sample,
            insert,
            frames: held.map_or(whole, |held| held.min(whole)),
            done: 0,
            origin: 0.0,
            step,
            made: step,
            since: 0,
            held,
            gains,
            follow: None,
            automated: 1.0,
            slid: 1.0,
            bends: None,
            next_bend: 0,
        }
    }

    /// The voice is on a channel whose volume, pan or pitch a clip
    /// drives, and its gains came from this level and pan of its own. Call
    /// it last, on a new voice. A voice whose pitch may move lasts as long
    /// as it would at the slowest speed, and ends early where it runs off
    /// the sample.
    pub fn following(mut self, follow: Follow) -> Self {
        self.follow = Some(follow);
        self.lengthen();
        self
    }

    /// The note slides: from output sample `at` of the voice on it plays at
    /// `ratio` times the speed it was made with, for every `(at, ratio)` of
    /// `bends`, which are in order. It lasts as long as it would at the
    /// slowest of them, and ends early where it runs off the sample. Call
    /// it last, on a new voice.
    pub fn bending(mut self, bends: Arc<[(usize, f64)]>) -> Self {
        self.bends = Some(bends);
        self.lengthen();
        self
    }

    /// Whether the voice's speed may move, so that it knows its end only
    /// when it gets there.
    fn pitched(&self) -> bool {
        self.bends.is_some() || self.follow.is_some_and(|f| f.slowest.is_some())
    }

    /// Make a voice whose speed may move as long as it lasts at the
    /// slowest it can go.
    fn lengthen(&mut self) {
        if !self.pitched() {
            return;
        }
        let automated = self.follow.and_then(|f| f.slowest).unwrap_or(1.0);
        let bends = self.bends.as_deref().unwrap_or(&[]);
        let slid = bends.iter().map(|b| b.1).fold(1.0, f64::min);
        let len = self.sample.left.len() as f64;
        let left = if self.step < 0.0 {
            self.origin + 1.0
        } else {
            len - self.origin
        };
        let slowest = automated.min(1.0) * slid;
        let most = (left / (self.step.abs() * slowest)).ceil() as usize;
        self.frames = self.held.map_or(most, |held| held.min(most));
    }

    pub fn follows(&self) -> Option<Follow> {
        self.follow
    }

    /// Left and right gain from the next sample on.
    pub fn set_gains(&mut self, gains: (f32, f32)) {
        self.gains = gains;
    }

    /// Play at `ratio` times the speed the voice was made with, from the
    /// next sample on. The voice goes on from where it is in the sample.
    pub fn set_pitch(&mut self, ratio: f64) {
        self.automated = ratio;
        self.retune();
    }

    /// Play at what automation and the slides make of the speed the voice
    /// was made with, from the next sample on.
    fn retune(&mut self) {
        let step = self.made * self.automated * self.slid;
        if step != self.step {
            self.origin += (self.done - self.since) as f64 * self.step;
            self.since = self.done;
            self.step = step;
        }
    }

    /// The voice started `offset` source frames into the sample. A
    /// `reversed` voice reads the sample from its last frame backwards, and
    /// `offset` then counts from that end. Call it once, on a new voice.
    pub fn starting_at(mut self, offset: f64, reversed: bool) -> Self {
        let len = self.sample.left.len() as f64;
        // No frames at all when the offset is past the sample or not a
        // number.
        let rest = ((len - offset) / self.step).ceil() as usize;
        self.frames = self.frames.min(rest);
        if reversed {
            self.origin = len - 1.0 - offset;
            self.step = -self.step;
            self.made = -self.made;
        } else {
            self.origin = offset;
        }
        self
    }

    /// Output samples the voice lasts from its first to its last.
    pub fn frames(&self) -> usize {
        self.frames
    }

    pub fn finished(&self) -> bool {
        self.done >= self.frames
    }

    /// Add the voice's next samples to a bus and move the voice on by as
    /// many. A voice that ends inside the bus leaves the rest alone.
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        let Some(bends) = self.bends.clone() else {
            self.render_run(left, right);
            return;
        };
        let len = left.len().min(right.len());
        let mut at = 0;
        while at < len && !self.finished() {
            while let Some(&(from, ratio)) = bends.get(self.next_bend) {
                if from > self.done {
                    break;
                }
                self.slid = ratio;
                self.retune();
                self.next_bend += 1;
            }
            let next = bends.get(self.next_bend).map_or(usize::MAX, |b| b.0);
            let run = (len - at).min(next - self.done);
            let before = self.done;
            self.render_run(&mut left[at..at + run], &mut right[at..at + run]);
            if self.done == before {
                break;
            }
            at += self.done - before;
        }
    }

    /// [`Voice::render`] at the speed the voice has now.
    fn render_run(&mut self, left: &mut [f32], right: &mut [f32]) {
        let count = (self.frames - self.done).min(left.len()).min(right.len());
        let len = self.sample.left.len() as f64;
        let pitched = self.pitched();
        let outs = left[..count].iter_mut().zip(&mut right[..count]);
        for (i, (l, r)) in outs.enumerate() {
            // A product and not a running sum, so the position does not
            // depend on where the blocks fall. Automation moves the pitch
            // on the same samples however the blocks fall.
            let pos = self.origin + (self.done - self.since + i) as f64 * self.step;
            // A voice whose speed moves knows its end only when it gets
            // there.
            if pitched && !(pos > -1.0 && pos < len) {
                self.frames = self.done + i;
                break;
            }
            let a = read_hermite(&self.sample.left, pos);
            let b = match &self.sample.right {
                Some(right) => read_hermite(right, pos),
                None => a,
            };
            *l += a * self.gains.0;
            *r += b * self.gains.1;
        }
        self.done = (self.done + count).min(self.frames);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flp::event::layout::LevelAdjusts;

    #[test]
    fn a_channel_s_adjustment_is_12800_until_its_229_says_otherwise() {
        let mut project = Project::new(flp::Header {
            format: 0,
            channel_count: 1,
            ppq: 96,
        });
        let mut channel = Channel::default();
        assert_eq!(adjust(&project, &channel), 12_800);
        channel.level_adjusts = Some(LevelAdjusts {
            unknown_0: [0; 4],
            unknown_4: 14_640i32.to_le_bytes(),
            unknown_8: [0; 12],
        });
        assert_eq!(adjust(&project, &channel), 14_640);
        project.writer_version = Some("20.8.4.2576".to_owned());
        assert_eq!(adjust(&project, &channel), 14_640);
        // A writer below 5.3.7 stores a hundredth.
        channel.level_adjusts.as_mut().unwrap().unknown_4 = 128i32.to_le_bytes();
        project.writer_version = Some("5.3.6".to_owned());
        assert_eq!(adjust(&project, &channel), 12_800);
        project.writer_version = Some("5.3.7".to_owned());
        assert_eq!(adjust(&project, &channel), 128);
    }

    fn mono(frames: &[f32]) -> Arc<Sample> {
        Arc::new(Sample {
            left: frames.to_vec(),
            right: None,
            rate: 48_000,
        })
    }

    fn render(voice: &mut Voice, len: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (vec![0.0; len], vec![0.0; len]);
        voice.render(&mut left, &mut right);
        (left, right)
    }

    #[test]
    fn at_step_1_the_sample_comes_out_as_it_is() {
        let mut voice = Voice::new(mono(&[1.0, 0.5, -0.25]), 0, None, 1.0, (1.0, 0.5));
        assert_eq!(voice.frames(), 3);
        let (left, right) = render(&mut voice, 5);
        assert_eq!(left, [1.0, 0.5, -0.25, 0.0, 0.0]);
        assert_eq!(right, [0.5, 0.25, -0.125, 0.0, 0.0]);
        assert!(voice.finished());
    }

    #[test]
    fn step_2_reads_every_other_frame() {
        let mut voice = Voice::new(mono(&[1.0, 2.0, 3.0, 4.0, 5.0]), 0, None, 2.0, (1.0, 1.0));
        assert_eq!(voice.frames(), 3);
        assert_eq!(render(&mut voice, 4).0, [1.0, 3.0, 5.0, 0.0]);
    }

    #[test]
    fn a_held_note_stops_at_its_length() {
        let mut voice = Voice::new(mono(&[1.0; 10]), 0, Some(3), 1.0, (1.0, 1.0));
        assert_eq!(render(&mut voice, 6).0, [1.0, 1.0, 1.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn a_stereo_sample_keeps_its_sides() {
        let sample = Arc::new(Sample {
            left: vec![1.0],
            right: Some(vec![-1.0]),
            rate: 48_000,
        });
        let mut voice = Voice::new(sample, 0, None, 1.0, (1.0, 1.0));
        let (left, right) = render(&mut voice, 2);
        assert_eq!((left[0], right[0]), (1.0, -1.0));
    }

    #[test]
    fn a_voice_goes_on_in_the_next_bus_where_it_stopped() {
        let frames = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0];
        let mut whole = Voice::new(mono(&frames), 0, None, 0.7, (1.0, 1.0));
        let mut split = whole.clone();
        let want = render(&mut whole, 12).0;
        let mut got = render(&mut split, 5).0;
        assert!(!split.finished());
        got.extend(render(&mut split, 4).0);
        got.extend(render(&mut split, 3).0);
        assert_eq!(got, want);
        assert!(split.finished());
        // A finished voice adds nothing.
        assert_eq!(render(&mut split, 2).0, [0.0, 0.0]);
    }

    #[test]
    fn an_offset_skips_the_start_of_the_sample() {
        let frames = [1.0, 2.0, 3.0, 4.0, 5.0];
        let voice =
            |offset| Voice::new(mono(&frames), 0, None, 1.0, (1.0, 1.0)).starting_at(offset, false);
        let mut late = voice(2.0);
        assert_eq!(late.frames(), 3);
        assert_eq!(render(&mut late, 4).0, [3.0, 4.0, 5.0, 0.0]);
        // The length a note is held for still counts.
        let held = Voice::new(mono(&frames), 0, Some(2), 1.0, (1.0, 1.0));
        assert_eq!(
            render(&mut held.starting_at(1.0, false), 4).0,
            [2.0, 3.0, 0.0, 0.0]
        );
        for past in [5.0, 9.0, f64::INFINITY, f64::NAN] {
            assert!(voice(past).finished(), "{past}");
        }
    }

    fn pitched(slowest: f64) -> Follow {
        Follow {
            channel: 0,
            level: 1.0,
            pan: 0.0,
            slowest: Some(slowest),
        }
    }

    #[test]
    fn a_voice_whose_pitch_moves_goes_on_from_the_frame_it_is_at() {
        let frames: Vec<f32> = (0..10).map(|i| i as f32).collect();
        let mut voice = Voice::new(mono(&frames), 0, None, 1.0, (1.0, 1.0)).following(pitched(1.0));
        assert_eq!(render(&mut voice, 3).0, [0.0, 1.0, 2.0]);
        voice.set_pitch(2.0);
        // It ends where it runs off the sample, not where it would have.
        assert_eq!(render(&mut voice, 5).0, [3.0, 5.0, 7.0, 9.0, 0.0]);
        assert!(voice.finished());
        assert_eq!(voice.frames(), 7);
        // Back to its own speed, a split render is the same as a whole one.
        let mut whole = Voice::new(mono(&frames), 0, None, 0.7, (1.0, 1.0)).following(pitched(0.5));
        whole.set_pitch(0.5);
        let mut split = whole.clone();
        let want = render(&mut whole, 30).0;
        let mut got = render(&mut split, 11).0;
        got.extend(render(&mut split, 19).0);
        assert_eq!(got, want);
    }

    #[test]
    fn a_voice_whose_pitch_may_drop_lasts_as_long_as_the_slowest_speed() {
        let frames = [1.0; 10];
        let voice = Voice::new(mono(&frames), 0, None, 1.0, (1.0, 1.0));
        assert_eq!(voice.frames(), 10);
        assert_eq!(voice.clone().following(pitched(0.5)).frames(), 20);
        // A slowest speed above 1 is the speed it was made with.
        assert_eq!(voice.clone().following(pitched(2.0)).frames(), 10);
        // A held note keeps its length, and an offset counts.
        let held = Voice::new(mono(&frames), 0, Some(12), 1.0, (1.0, 1.0));
        assert_eq!(held.following(pitched(0.25)).frames(), 12);
        let late = voice
            .clone()
            .starting_at(4.0, false)
            .following(pitched(0.5));
        assert_eq!(late.frames(), 12);
        let back = voice.starting_at(4.0, true).following(pitched(0.5));
        assert_eq!(back.frames(), 12);
        // A reversed voice speeds up backwards.
        let ramp: Vec<f32> = (0..10).map(|i| i as f32).collect();
        let mut back = Voice::new(mono(&ramp), 0, None, 1.0, (1.0, 1.0))
            .starting_at(0.0, true)
            .following(pitched(1.0));
        assert_eq!(render(&mut back, 2).0, [9.0, 8.0]);
        back.set_pitch(3.0);
        assert_eq!(render(&mut back, 4).0, [7.0, 4.0, 1.0, 0.0]);
    }

    #[test]
    fn a_reversed_voice_reads_from_the_last_frame_back() {
        let frames = [1.0, 2.0, 3.0, 4.0, 5.0];
        let voice = |offset, step| {
            Voice::new(mono(&frames), 0, None, step, (1.0, 1.0)).starting_at(offset, true)
        };
        assert_eq!(
            render(&mut voice(0.0, 1.0), 6).0,
            [5.0, 4.0, 3.0, 2.0, 1.0, 0.0]
        );
        // The offset counts from the end.
        assert_eq!(render(&mut voice(2.0, 1.0), 4).0, [3.0, 2.0, 1.0, 0.0]);
        assert_eq!(render(&mut voice(0.0, 2.0), 4).0, [5.0, 3.0, 1.0, 0.0]);
        let mut whole = voice(1.0, 0.7);
        let mut split = whole.clone();
        let want = render(&mut whole, 8).0;
        let mut got = render(&mut split, 3).0;
        got.extend(render(&mut split, 5).0);
        assert_eq!(got, want);
    }
}
