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
/// so does this.
pub(crate) fn adjust(project: &Project, channel: &Channel) -> i32 {
    let Some(stored) = channel.level_adjusts else {
        return 12_800;
    };
    let stored = i32::from_le_bytes(stored.unknown_4);
    let version = project.writer_version.as_deref();
    match version.and_then(flp::event::version_number) {
        Some(..5_003_007) => stored.saturating_mul(100),
        _ => stored,
    }
}

/// Left and right gain of one voice: its own level and its channel's
/// volume, then the channel's pan plus its own. `volume` and `channel_pan`
/// are the channel's [`levels`]. `level` is a note's velocity gain or a
/// clip's gain, and `pan` is the note's or the clip's, from -1 to 1.
pub(crate) fn gains(
    volume: u32,
    channel_pan: u32,
    level: f32,
    pan: f32,
    curves: &dyn Curves,
) -> (f32, f32) {
    let gain = level * curves.volume_gain(volume);
    let channel_pan = channel_pan as f32 / 6400.0 - 1.0;
    let (left, right) = curves.pan_gains((channel_pan + pan).clamp(-1.0, 1.0));
    (gain * left, gain * right)
}

/// What a voice needs to work its gains out again when automation moves
/// its channel's volume or pan.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Follow {
    /// The channel, by the index its 64 has.
    pub channel: u16,
    /// The voice's own level and pan, as [`gains`] takes them.
    pub level: f32,
    pub pan: f32,
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
    gains: (f32, f32),
    /// Set for a voice on a channel whose volume or pan a clip drives.
    follow: Option<Follow>,
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
            gains,
            follow: None,
        }
    }

    /// The voice is on a channel whose volume or pan a clip drives, and
    /// its gains came from this level and pan of its own.
    pub fn following(mut self, follow: Follow) -> Self {
        self.follow = Some(follow);
        self
    }

    pub fn follows(&self) -> Option<Follow> {
        self.follow
    }

    /// Left and right gain from the next sample on.
    pub fn set_gains(&mut self, gains: (f32, f32)) {
        self.gains = gains;
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
        let count = (self.frames - self.done).min(left.len()).min(right.len());
        let outs = left[..count].iter_mut().zip(&mut right[..count]);
        for (i, (l, r)) in outs.enumerate() {
            // A product and not a running sum, so the position does not
            // depend on where the blocks fall.
            let pos = self.origin + (self.done + i) as f64 * self.step;
            let a = read_hermite(&self.sample.left, pos);
            let b = match &self.sample.right {
                Some(right) => read_hermite(right, pos),
                None => a,
            };
            *l += a * self.gains.0;
            *r += b * self.gains.1;
        }
        self.done += count;
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
