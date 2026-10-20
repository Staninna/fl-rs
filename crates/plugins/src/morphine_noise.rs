//! Morphine's noise layer: a sample from its `Noise Samples` directory
//! played under each voice at a ratio of its rate, looped, through a
//! two-pole filter (`FUN_180100300` sets it up each block,
//! `FUN_180100880` plays it, `FUN_180113b70` and `FUN_180113df0` are the
//! filter, `FUN_1800de050` reads the file).

/// A noise sample as the engine keeps it: each channel as f32, with four
/// samples of silence after the end for the interpolation to read.
#[derive(Debug, Clone, PartialEq)]
pub struct NoiseSample {
    channels: Vec<Vec<f32>>,
    frames: usize,
    pub rate: f32,
    pub loop_start: usize,
    pub loop_end: usize,
}

impl NoiseSample {
    /// Read a `.wav`: 8, 16, 24 or 32-bit integers or 32-bit floats, one
    /// or two channels. The loop is the whole sample, or the first loop of
    /// a `smpl` chunk after the data.
    pub fn read(bytes: &[u8]) -> Option<Self> {
        let word = |at: usize| Some(u16::from_le_bytes(*bytes.get(at..)?.first_chunk()?));
        let long = |at: usize| Some(u32::from_le_bytes(*bytes.get(at..)?.first_chunk()?));
        if bytes.get(..4)? != b"RIFF" || bytes.get(8..12)? != b"WAVE" {
            return None;
        }
        let mut at = 12;
        let mut format = None;
        let mut sample: Option<NoiseSample> = None;
        while at + 8 <= bytes.len() {
            let size = long(at + 4)? as usize;
            let body = &bytes[at + 8..];
            let body = &body[..size.min(body.len())];
            match &bytes[at..at + 4] {
                b"fmt " => {
                    format = Some((
                        word(at + 8)?,
                        word(at + 10)?,
                        long(at + 12)?,
                        word(at + 22)?,
                    ))
                }
                b"data" => {
                    let (tag, channels, rate, bits) = format?;
                    if !(1..=2).contains(&channels) {
                        return None;
                    }
                    let width = usize::from(bits / 8);
                    let frame = usize::from(channels) * width;
                    if frame == 0 {
                        return None;
                    }
                    let frames = body.len() / frame;
                    let mut data = vec![Vec::with_capacity(frames + 4); usize::from(channels)];
                    for f in body.chunks_exact(frame) {
                        for (c, s) in f.chunks_exact(width).enumerate() {
                            let v = match (tag, bits) {
                                (1, 8) => (f32::from(s[0]) - 128.0) / 128.0,
                                (1, 16) => {
                                    f32::from(i16::from_le_bytes([s[0], s[1]])) * 3.051_757_8e-5
                                }
                                (1, 24) => {
                                    (i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8) as f32
                                        / 8_388_608.0
                                }
                                (1, 32) => {
                                    i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f32
                                        / 2_147_483_648.0
                                }
                                (3, 32) => f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
                                _ => return None,
                            };
                            data[c].push(v);
                        }
                    }
                    for channel in &mut data {
                        channel.extend([0.0; 4]);
                    }
                    sample = Some(NoiseSample {
                        channels: data,
                        frames,
                        rate: rate as f32,
                        loop_start: 0,
                        loop_end: frames,
                    });
                }
                b"smpl" if size > 0x33 => {
                    if let Some(s) = sample.as_mut() {
                        let start = long(at + 8 + 0x2c)? as usize;
                        let end = long(at + 8 + 0x30)? as usize;
                        s.loop_start = if start >= s.frames { 0 } else { start };
                        s.loop_end = end.min(s.frames);
                    }
                }
                _ => {}
            }
            at += 8 + size + (size & 1);
        }
        let mut s = sample?;
        s.loop_end = s.loop_end.min(s.frames);
        s.loop_start = s.loop_start.min(s.loop_end);
        Some(s)
    }

    pub fn stereo(&self) -> bool {
        self.channels.len() == 2
    }

    /// The Catmull-Rom curve through samples `i + 1` and `i + 2`, `frac`
    /// of the way (`FUN_1800da140`).
    fn read_at(&self, channel: usize, i: usize, frac: f32) -> f32 {
        let s = &self.channels[channel];
        if i + 3 >= s.len() {
            return 0.0;
        }
        let (y0, y1, y2, y3) = (s[i], s[i + 1], s[i + 2], s[i + 3]);
        let a = (y2 - y0) * 0.5;
        let b = (y3 - y0) * 0.5 + (y1 - y2) * 1.5;
        ((((y0 - y1) + a) - b + b * frac) * frac + a) * frac + y1
    }
}

/// The noise of one voice.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoiseRun {
    pos: f64,
    dir: f64,
    speed: f64,
    left: f32,
    right: f32,
    dleft: f32,
    dright: f32,
    mode: i32,
    start: i32,
    end: i32,
    mono: bool,
    filter: Filter,
}

/// The filter: off, low-pass, band-pass or high-pass, with the left
/// channel's state and the right's.
#[derive(Debug, Clone, Copy, Default)]
struct Filter {
    c: [f32; 3],
    mode: i32,
    state: [f32; 8],
}

fn flush(v: f32) -> f32 {
    if v < 1e-9 && v > -1e-9 { 0.0 } else { v }
}

impl Filter {
    /// `FUN_180113b70`: the cutoff runs over 9 octaves from 35 Hz and with
    /// the key at `(note - 57) * 0.158 * key_track` octaves; the resonance
    /// sets `q = 30 r^2 + 1`.
    fn design(&mut self, cutoff: f32, res: f32, key_track: f32, mode: i32, note: f64, rate: f64) {
        self.mode = mode;
        let w = 2f64.powf(
            (note - 57.0) * 0.157_894_736_842_105_25 * f64::from(key_track)
                + f64::from(cutoff) * 9.0,
        ) * 219.911_485_751_285_5
            / rate;
        let w = w.clamp(0.0, 3.1);
        let q = (f64::from(res * res) * 30.0 + 1.0).clamp(1.0, 31.0);
        let (sin, cos) = w.sin_cos();
        let (c0, c1, c2) = match mode {
            1 | 3 => {
                let top = if mode == 1 { 1.0 - cos } else { cos + 1.0 };
                let c0 = (top / (sin / q + 2.0)) as f32;
                let d = 1.0 / (sin + q);
                (c0, cos * d * (q * -2.0), (q - sin) * d)
            }
            2 => {
                let alpha = 0.5 / q * sin;
                let d = 1.0 / (alpha + 1.0);
                let c0 = f64::from((sin * 0.5) as f32) * d;
                let c2 = f64::from((1.0 - alpha) as f32) * d;
                let c1 = f64::from((cos + cos) as f32) * d;
                (c0 as f32, c1, c2)
            }
            _ => return,
        };
        self.c = [flush(c0), flush(c1 as f32), flush(c2 as f32)];
    }

    fn run(&mut self, l: &mut f32, r: &mut f32) {
        let [c0, c1, c2] = self.c;
        let mode = self.mode;
        for (x, s) in [l, r]
            .into_iter()
            .zip(self.state.as_chunks_mut::<4>().0.iter_mut())
        {
            let (x2, x1, y2, y1) = (s[0], s[1], s[2], s[3]);
            let input = *x;
            let y = match mode {
                1 => ((x1 + x1 + input + x2) * c0 - c1 * y1) - c2 * y2,
                2 => ((input - x2) * c0 + c1 * y1) - c2 * y2,
                3 => (((input - (x1 + x1)) + x2) * c0 - c1 * y1) - c2 * y2,
                _ => return,
            };
            s[0] = flush(x1);
            s[1] = flush(input);
            s[2] = flush(y1);
            s[3] = flush(y);
            *x = y;
        }
    }
}

/// What a voice's noise is set to for a block.
pub struct NoiseBlock {
    /// The mix, pan and the rest with what the matrix sends them.
    pub mix: f32,
    pub pan: f32,
    pub ratio: f32,
    pub cutoff: f32,
    pub res: f32,
    pub key_track: f32,
    pub mute: bool,
    pub vel_sense: f32,
    pub loop_mode: i32,
    pub filter_mode: i32,
    pub mono: bool,
    /// The voice's pan, velocity, level this block and note.
    pub voice_pan: f32,
    pub velocity: f32,
    pub level: f32,
    pub note: f64,
}

impl NoiseRun {
    pub fn start(&mut self) {
        self.pos = 0.0;
        self.dir = 1.0;
        self.filter.state = [0.0; 8];
    }

    /// Set the noise up for a block of `n` samples (`FUN_180100300`).
    pub fn block(&mut self, sample: &NoiseSample, b: &NoiseBlock, n: usize, rate: f64) {
        let mix = b.mix * b.mix;
        let mut mix = mix * mix;
        mix += mix;
        if b.mute {
            mix = 0.0;
        }
        let pan = ((b.pan + b.pan + b.voice_pan) * 0.5).clamp(0.0, 1.0);
        let mix = (1.0 - (1.0 - b.velocity) * b.vel_sense) * mix;
        let step = 1.0 / n as f32;
        // Both channels at half gain, doubled (`FUN_1800e9f30`).
        let right = pan * mix * b.level;
        let left = (1.0 - pan) * mix * b.level;
        self.dright = (right - self.right) * step;
        self.dleft = (left - self.left) * step;
        self.speed = 2f64.powf((f64::from(b.ratio) - 0.5) * 4.0) * f64::from(sample.rate) / rate;
        self.mode = b.loop_mode;
        let last = sample.frames as i32 - 1;
        let mut start = (sample.loop_start as i32).clamp(0, last.max(0));
        let mut end = (sample.loop_end as i32).clamp(0, last.max(0));
        if start == end {
            start = 0;
            end = last;
        }
        self.start = start;
        self.end = end;
        self.mono = b.mono;
        self.filter
            .design(b.cutoff, b.res, b.key_track, b.filter_mode, b.note, rate);
    }

    /// Add the noise of `left.len()` samples.
    pub fn render(&mut self, sample: &NoiseSample, left: &mut [f32], right: &mut [f32]) {
        let len = self.end - self.start;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let pos = self.pos;
            let i = pos as i32;
            let (mut a, mut b) = (0.0, 0.0);
            if i >= 0 && (i as usize) < sample.frames {
                let frac = (pos - f64::from(i)) as f32;
                a = sample.read_at(0, i as usize, frac);
                b = if sample.stereo() {
                    sample.read_at(1, i as usize, frac)
                } else {
                    a
                };
                if self.mono {
                    a = (a + b) * 0.5;
                    b = a;
                }
            }
            let (end, start) = (f64::from(self.end), f64::from(self.start));
            match self.mode {
                0 => self.pos = pos + self.speed,
                1 => {
                    let p = pos + self.speed;
                    self.pos = if p >= end { p - f64::from(len) } else { p };
                }
                2 => {
                    let mut p = pos;
                    let mut wrap = true;
                    if p < end {
                        p += self.dir * self.speed;
                        wrap = self.dir < 0.0;
                    } else {
                        p -= self.speed;
                        self.dir = -1.0;
                    }
                    if wrap && p <= start {
                        p += f64::from(len);
                    }
                    self.pos = p;
                }
                3 => {
                    if pos >= end {
                        self.dir = -1.0;
                    }
                    if pos <= start && (pos >= end || self.dir < 0.0) {
                        self.dir = 1.0;
                    }
                    self.pos = self.dir * self.speed + pos;
                }
                _ => {}
            }
            self.filter.run(&mut a, &mut b);
            *l += a * self.left;
            *r += b * self.right;
            self.right += self.dright;
            self.left += self.dleft;
        }
    }
}

/// The peak a sine of `w` radians a sample comes out of the filter at.
#[cfg(test)]
fn gain_at(filter: &mut Filter, w: f64) -> f64 {
    let mut peak = 0.0f64;
    for n in 0..20_000 {
        let x = (w * n as f64).sin() as f32;
        let (mut l, mut r) = (x, x);
        filter.run(&mut l, &mut r);
        if n > 10_000 {
            peak = peak.max(f64::from(l.abs()));
        }
    }
    peak
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn a_wav_s_samples_and_loop_are_read() {
        let mut bytes = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0".to_vec();
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(44_100u32.to_le_bytes());
        bytes.extend([0; 6]);
        bytes.extend(16u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend(8u32.to_le_bytes());
        for v in [0i16, 16_384, -16_384, 32_767] {
            bytes.extend(v.to_le_bytes());
        }
        bytes.extend(b"smpl");
        bytes.extend(0x3cu32.to_le_bytes());
        let mut smpl = vec![0u8; 0x3c];
        smpl[0x2c..0x30].copy_from_slice(&1u32.to_le_bytes());
        smpl[0x30..0x34].copy_from_slice(&3u32.to_le_bytes());
        bytes.extend(smpl);
        let s = NoiseSample::read(&bytes).unwrap();
        assert_eq!(s.frames, 4);
        assert_eq!((s.loop_start, s.loop_end), (1, 3));
        assert_eq!(s.channels[0][1], 0.5);
        // Between samples 1 and 2 at the start of the curve: sample 1.
        assert_eq!(s.read_at(0, 0, 0.0), 0.5);
    }

    #[test]
    fn the_low_pass_passes_the_bass_and_the_high_pass_the_treble() {
        let rate = 44_100.0;
        let mut lp = Filter::default();
        lp.design(0.5, 0.0, 0.0, 1, 57.0, rate);
        let mut hp = Filter::default();
        hp.design(0.5, 0.0, 0.0, 3, 57.0, rate);
        // The cutoff at 0.5 is 35 Hz * 2^4.5, about 792 Hz.
        let low = 2.0 * PI * 100.0 / rate;
        let high = 2.0 * PI * 8_000.0 / rate;
        assert!(gain_at(&mut lp, low) > 0.9);
        assert!(gain_at(&mut lp, high) < 0.05);
        let mut hp2 = hp;
        assert!(gain_at(&mut hp, high) > 0.9);
        assert!(gain_at(&mut hp2, low) < 0.05);
    }
}
