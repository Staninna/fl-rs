//! FL's 2x resampler: a polyphase half band of two paths of three
//! first-order allpasses, run at the lower rate. FL's Delphi plugins share
//! it (Fruity WaveShaper, Delay Bank, Love Philter, EQUO, Maximus,
//! Parametric EQ 2): a stage is `0xc0` bytes, twelve pairs of doubles for
//! the two sides, and its SSE kernels (`FUN_006db400` up and `FUN_006db5d0`
//! down in `Fruity Delay Bank_x64.dll`, the same instructions at
//! `0x0082fb70` and `0x0082fd40` in WaveShaper's) read the thirteen doubles
//! of [`PATHS`] and the 0.5 that scales the sum going down.
//!
//! Each allpass is `y = x1 + (x - y1) c` in doubles, on the input widened
//! from `f32`, and the output is cut back to `f32`. Going up, one input
//! sample goes through both paths: the second path gives the first of the
//! two samples and the first path the second. Going down, the first of
//! each two goes through the first path and the second through the
//! second, and the output is `(second + first) * 0.5`.
//!
//! The pair is a low pass whose corner is a quarter of the higher rate.
//! Being recursive it has no fixed delay: at low frequencies it is 1.65
//! samples of the lower rate late going up and 1.15 going down, and the
//! plugins report none of it to the host.

/// The coefficients of the two paths, as `FUN_006dba60` copies them from
/// `0x00733fa0` (Delay Bank) and `FUN_008301d0` from `0x0089ee70`
/// (WaveShaper), where they sit interleaved, first path first.
pub const PATHS: [[f64; 3]; 2] = [
    [
        0.147_377_113_601_046_6,
        0.482_468_542_769_700_14,
        0.883_005_025_769_373_1,
    ],
    [
        0.039_151_597_734_460_045,
        0.302_646_848_328_493_4,
        0.674_615_918_546_963_9,
    ],
];

/// What FL's block flush (`FUN_005e27a0`, `FUN_0062ca40`) zeroes an
/// allpass output under: 2^-24.
pub const TINY: f64 = 5.960_464_477_539_063e-8;

/// The most stages [`Oversampler`] runs, and the frames that makes at the
/// highest rate for one at the lowest.
pub const MAX_STAGES: usize = 4;
pub const WIDEST: usize = 1 << MAX_STAGES;

/// One first-order allpass: `y = x1 + (x - y1) * c`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Allpass {
    before: f64,
    out: f64,
}

impl Allpass {
    fn step(&mut self, x: f64, c: f64) -> f64 {
        self.out = self.before + (x - self.out) * c;
        self.before = x;
        self.out
    }
}

/// One side of one stage in one direction.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct HalfBand {
    paths: [[Allpass; 3]; 2],
}

impl HalfBand {
    fn path(&mut self, path: usize, x: f64) -> f64 {
        let stages = self.paths[path].iter_mut().zip(PATHS[path]);
        stages.fold(x, |x, (stage, c)| stage.step(x, c))
    }

    /// The two samples at twice the rate for one sample.
    pub fn up_one(&mut self, x: f32) -> [f32; 2] {
        let x = f64::from(x);
        [self.path(1, x) as f32, self.path(0, x) as f32]
    }

    /// The sample at half the rate for two.
    pub fn down_one(&mut self, pair: [f32; 2]) -> f32 {
        let first = self.path(0, f64::from(pair[0]));
        let second = self.path(1, f64::from(pair[1]));
        ((second + first) * 0.5) as f32
    }

    /// Two samples in `wide` for each one in `input`.
    pub fn up(&mut self, input: &[f32], wide: &mut [f32]) {
        for (x, pair) in input.iter().zip(wide.as_chunks_mut::<2>().0) {
            *pair = self.up_one(*x);
        }
    }

    /// One sample in `out` for each two in `wide`.
    pub fn down(&mut self, wide: &[f32], out: &mut [f32]) {
        for (pair, y) in wide.as_chunks::<2>().0.iter().zip(out) {
            *y = self.down_one(*pair);
        }
    }

    /// FL's flush after a block: an allpass output under [`TINY`] in size
    /// becomes 0. The inputs are left as they are (the kernel flushes the
    /// first twelve of a stage's 24 doubles, which are the outputs).
    pub fn flush(&mut self) {
        for stage in self.paths.as_flattened_mut() {
            if stage.out.abs() < TINY {
                stage.out = 0.0;
            }
        }
    }

    /// A flush of only what a double cannot hold in full, inputs and
    /// outputs. Love Philter and Parametric EQ 2 are built with this one.
    pub fn flush_subnormal(&mut self) {
        for stage in self.paths.as_flattened_mut() {
            for state in [&mut stage.before, &mut stage.out] {
                if state.is_subnormal() {
                    *state = 0.0;
                }
            }
        }
    }
}

/// Up to [`MAX_STAGES`] stages of [`HalfBand`] up and as many down, for a
/// stereo frame at a time: FL's `TOversampler` pair, which runs the stages
/// over a whole block one after the other. Each stage is a recursion of
/// its own, so a frame at a time comes out the same.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Oversampler {
    up: [[HalfBand; 2]; MAX_STAGES],
    down: [[HalfBand; 2]; MAX_STAGES],
}

impl Oversampler {
    /// One frame up through `stages` stages into `wide`: `1 << stages`
    /// frames in time order.
    pub fn up(&mut self, stages: usize, frame: [f32; 2], wide: &mut [[f32; 2]; WIDEST]) {
        wide[0] = frame;
        let mut scratch = [[0.0f32; 2]; WIDEST];
        for (stage, sides) in self.up[..stages].iter_mut().enumerate() {
            let n = 1 << stage;
            for (i, frame) in wide[..n].iter().enumerate() {
                let [l0, l1] = sides[0].up_one(frame[0]);
                let [r0, r1] = sides[1].up_one(frame[1]);
                scratch[2 * i] = [l0, r0];
                scratch[2 * i + 1] = [l1, r1];
            }
            wide[..2 * n].copy_from_slice(&scratch[..2 * n]);
        }
    }

    /// The `1 << stages` frames of `wide` down through `stages` stages to
    /// one. `wide` is used up on the way.
    pub fn down(&mut self, stages: usize, wide: &mut [[f32; 2]; WIDEST]) -> [f32; 2] {
        for (stage, sides) in self.down[..stages].iter_mut().enumerate() {
            let n = 1 << (stages - stage - 1);
            for i in 0..n {
                let (even, odd) = (wide[2 * i], wide[2 * i + 1]);
                wide[i] = [
                    sides[0].down_one([even[0], odd[0]]),
                    sides[1].down_one([even[1], odd[1]]),
                ];
            }
        }
        wide[0]
    }

    /// FL's flush after a block, on every stage.
    pub fn flush(&mut self) {
        for side in self.up.as_flattened_mut().iter_mut() {
            side.flush();
        }
        for side in self.down.as_flattened_mut().iter_mut() {
            side.flush();
        }
    }

    /// [`HalfBand::flush_subnormal`] on every stage.
    pub fn flush_subnormal(&mut self) {
        for side in self.up.as_flattened_mut().iter_mut() {
            side.flush_subnormal();
        }
        for side in self.down.as_flattened_mut().iter_mut() {
            side.flush_subnormal();
        }
    }

    /// Every stage back to silence, as FL's rebuild and its flush
    /// (`FUN_006db8d0`, `FUN_00830040`) leave them.
    pub fn reset(&mut self) {
        *self = Oversampler::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    fn sine(turns: f64, len: usize) -> Vec<f32> {
        (0..len)
            .map(|n| (TAU * turns * n as f64).sin() as f32)
            .collect()
    }

    /// The size of the partial at `turns` per sample in `x`.
    fn partial(x: &[f32], turns: f64) -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for (n, v) in x.iter().enumerate() {
            let phase = TAU * turns * n as f64;
            re += f64::from(*v) * phase.cos();
            im += f64::from(*v) * phase.sin();
        }
        2.0 * (re * re + im * im).sqrt() / x.len() as f64
    }

    #[test]
    fn the_paths_are_the_dll_s_doubles_to_the_bit() {
        // As `0x00733fa0` in Delay Bank's DLL and `0x0089ee70` in
        // WaveShaper's hold them, interleaved.
        let bits = [
            [
                0x3fc2_dd40_d58c_3526u64,
                0x3fde_e0c3_bd22_db5f,
                0x3fec_4193_c17c_4250,
            ],
            [
                0x3fa4_0bad_9fb6_183a,
                0x3fd3_5e90_e2f3_bc08,
                0x3fe5_9674_1f70_a58f,
            ],
        ];
        for (path, want) in PATHS.iter().zip(bits) {
            assert_eq!(path.map(f64::to_bits), want);
        }
        assert_eq!(TINY, f64::from(f32::from_bits(0x3380_0000)));
    }

    /// FNV-1a 64 over the bytes of each value's bits.
    fn fnv(bits: &[u32]) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for byte in bits.iter().flat_map(|b| b.to_le_bytes()) {
            h ^= u64::from(byte);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        h
    }

    /// The blocks `tools/plugstate/halfband_emu.py` runs, the stage up
    /// and the stage down with FL's flush after each, as interleaved
    /// bits.
    fn run_blocks(flush: bool) -> (Vec<u32>, Vec<u32>) {
        let loud: Vec<[f32; 2]> = (0..16)
            .map(|n| {
                let l = ((n * 37) % 101 - 50) as f32 / 64.0;
                let r = ((n * 53) % 97 - 48) as f32 / 32.0;
                [l, r]
            })
            .collect();
        let tiny = loud.iter().map(|f| f.map(|v| v * 2f32.powi(-28))).collect();
        let (mut up, mut down) = ([HalfBand::default(); 2], [HalfBand::default(); 2]);
        let (mut wide_bits, mut narrow_bits) = (Vec::new(), Vec::new());
        for block in [loud, tiny, vec![[0.0; 2]; 8]] {
            let mut wide = vec![[0.0f32; 2]; 2 * block.len()];
            for (frame, pair) in block.iter().zip(wide.as_chunks_mut::<2>().0) {
                let [l, r] = [0, 1].map(|side| up[side].up_one(frame[side]));
                *pair = [[l[0], r[0]], [l[1], r[1]]];
            }
            for pair in wide.as_chunks::<2>().0 {
                let out = [0, 1].map(|side| down[side].down_one([pair[0][side], pair[1][side]]));
                narrow_bits.extend(out.map(f32::to_bits));
            }
            if flush {
                for band in up.iter_mut().chain(&mut down) {
                    band.flush();
                }
            }
            wide_bits.extend(wide.as_flattened().iter().map(|v| v.to_bits()));
        }
        (wide_bits, narrow_bits)
    }

    #[test]
    fn the_dll_s_kernels_give_these_bits() {
        // `halfband_emu.py` on Delay Bank's DLL and on WaveShaper's: the
        // SSE kernels and the flush, run under unicorn, print these.
        let (wide, narrow) = run_blocks(true);
        assert_eq!(wide.len(), 160);
        assert_eq!(
            wide[..4],
            [0xbbcc_a2d6, 0xbc44_735d, 0xbd48_ea39, 0xbdc0_e0da]
        );
        assert_eq!(fnv(&wide), 0xafba_6d29_fcb5_e024);
        assert_eq!(narrow.len(), 80);
        assert_eq!(
            narrow[..4],
            [0xb9cd_927d, 0xba45_596d, 0xbc74_6f4e, 0xbce8_6959]
        );
        assert_eq!(fnv(&narrow), 0x993e_26eb_6fc9_37e0);
        // Without the flush the quiet blocks come out otherwise, so the
        // flush is part of what is checked.
        let (wide, narrow) = run_blocks(false);
        assert!(fnv(&wide) != 0xafba_6d29_fcb5_e024 || fnv(&narrow) != 0x993e_26eb_6fc9_37e0);
    }

    #[test]
    fn going_up_the_band_passes_and_its_image_is_stopped() {
        // A tone keeps its size and its image above the old band's edge
        // is 80 dB down or more, up to 18 kHz at 44.1 kHz. Down again is
        // unity, and what is over a quarter of the higher rate is stopped
        // on the way down.
        for (hz, floor) in [
            (100usize, -100.0),
            (1_000, -100.0),
            (10_000, -100.0),
            (18_000, -80.0),
        ] {
            let turns = hz as f64 / 44_100.0;
            let input = sine(turns, 2 * 44_100);
            let mut wide = vec![0.0; 4 * 44_100];
            HalfBand::default().up(&input, &mut wide);
            let settled = &wide[2 * 44_100..];
            assert!(
                (partial(settled, turns / 2.0) - 1.0).abs() < 1e-4,
                "{hz} Hz"
            );
            let image = 20.0 * partial(settled, 0.5 - turns / 2.0).log10();
            assert!(image < floor, "{hz} Hz: image at {image} dB");
            let mut back = vec![0.0; 2 * 44_100];
            HalfBand::default().down(&wide, &mut back);
            assert!(
                (partial(&back[44_100..], turns) - 1.0).abs() < 1e-4,
                "{hz} Hz"
            );
        }
        for (hz, floor) in [(26_100.0, -80.0), (30_000.0, -100.0), (40_000.0, -100.0)] {
            let turns = hz / 88_200.0;
            let wide = sine(turns, 4 * 44_100);
            let mut back = vec![0.0; 2 * 44_100];
            HalfBand::default().down(&wide, &mut back);
            let alias = 20.0 * partial(&back[44_100..], 1.0 - 2.0 * turns).log10();
            assert!(alias < floor, "{hz} Hz: alias at {alias} dB");
        }
    }

    /// How many samples `x` is behind `sin(2 pi turns n)`, from the
    /// phase of its second half.
    fn late(x: &[f32], turns: f64) -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for (n, v) in x.iter().enumerate().skip(x.len() / 2) {
            let phase = TAU * turns * n as f64;
            re += f64::from(*v) * phase.cos();
            im += f64::from(*v) * phase.sin();
        }
        // sin(w (n - d)) = sin(w n) cos(w d) - cos(w n) sin(w d).
        -re.atan2(im) / (TAU * turns)
    }

    #[test]
    fn its_delay_at_the_bottom_is_the_allpasses_and_fl_reports_none() {
        // A first-order allpass is `(1 - c) / (1 + c)` samples late at
        // DC. Going up, both paths land 1.654 samples of the lower rate
        // late (the first path's 1.154 plus the half sample it is placed
        // after); going down it is the first path's 1.154.
        let delay = |path: &[f64; 3]| path.iter().map(|c| (1.0 - c) / (1.0 + c)).sum::<f64>();
        let (first, second) = (delay(&PATHS[0]), delay(&PATHS[1]));
        assert!((second - first - 0.5).abs() < 1e-3, "{first} {second}");

        let turns = 50.0 / 44_100.0;
        let input = sine(turns, 44_100);
        let mut wide = vec![0.0; 2 * 44_100];
        let mut back = vec![0.0; 44_100];
        HalfBand::default().up(&input, &mut wide);
        HalfBand::default().down(&wide, &mut back);
        let up = late(&wide, turns / 2.0) / 2.0;
        assert!((up - second).abs() < 0.01, "{up}");
        let both = late(&back, turns);
        assert!((both - second - first).abs() < 0.01, "{both}");

        // A level that stands still goes through as it is.
        let mut wide = [0.0; 400];
        HalfBand::default().up(&[0.5; 200], &mut wide);
        assert!(wide[300..].iter().all(|v| (v - 0.5).abs() < 1e-6));
    }

    #[test]
    fn the_flush_zeroes_only_small_outputs() {
        let mut band = HalfBand::default();
        band.up_one(1e-9);
        let before = band;
        band.flush();
        assert!(band.paths.as_flattened().iter().all(|s| s.out == 0.0));
        // The inputs stay: FL flushes the first twelve doubles only.
        assert_eq!(band.paths[0][0].before, before.paths[0][0].before);
        let mut band = HalfBand::default();
        band.up_one(0.5);
        let kept = band;
        band.flush();
        assert_eq!(band, kept);
    }

    #[test]
    fn a_frame_at_a_time_is_the_block_s_cascade() {
        // Stage by stage over a block, as FL runs them, against the
        // oversampler's frame at a time, for every count of stages.
        let input = sine(0.013, 64);
        for stages in 0..=MAX_STAGES {
            let mut block: Vec<f32> = input.clone();
            let mut ups = [HalfBand::default(); MAX_STAGES];
            for up in &mut ups[..stages] {
                let mut wide = vec![0.0; block.len() * 2];
                up.up(&block, &mut wide);
                block = wide;
            }
            let shaped: Vec<f32> = block.iter().map(|v| v * v).collect();
            let mut block = shaped;
            let mut downs = [HalfBand::default(); MAX_STAGES];
            for down in &mut downs[..stages] {
                let mut narrow = vec![0.0; block.len() / 2];
                down.down(&block, &mut narrow);
                block = narrow;
            }
            let mut over = Oversampler::default();
            for (n, x) in input.iter().enumerate() {
                let mut wide = [[0.0; 2]; WIDEST];
                over.up(stages, [*x, -*x], &mut wide);
                for frame in &mut wide[..1 << stages] {
                    *frame = frame.map(|v| v * v);
                }
                assert_eq!(over.down(stages, &mut wide), [block[n]; 2], "{stages} {n}");
            }
        }
    }
}
