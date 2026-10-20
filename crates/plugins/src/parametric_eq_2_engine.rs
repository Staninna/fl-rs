//! What Fruity Parametric EQ 2 borrows from the C++ `engine_x64.dll` in
//! Frequency Splitter's folder: the engine's 2x resampler and its matched
//! second order filters. `docs/plugins/fruity-parametric-eq-2.md` maps
//! both, with their addresses.
//!
//! Every sum here is in the order the DLL does it, so that it comes out
//! the same to the last bit but where FL's libm and Rust's give a sine or
//! a logarithm one bit apart.

use dsp::biquad::Coefficients64;
use std::cmp::Ordering;
use std::f64::consts::{FRAC_2_PI, FRAC_PI_2, PI};

/// How many taps the resampler's low pass has.
pub(crate) const TAPS: usize = 176;

/// How many frames late the engine's resampler leaves the sound: its centre
/// tap, 89 samples at twice the rate, less the half sample the zero that
/// follows each one is late.
pub(crate) const RESAMPLER_LATENCY: usize = 44;

/// The engine's half-band low pass (`mydispatcher` op 10): a sinc of half
/// the band, halved, under a half sine window, rounded to floats twice as
/// the DLL rounds it.
pub(crate) fn half_band() -> [f32; TAPS] {
    let mut taps = [0.0f32; TAPS];
    for (i, tap) in taps.iter_mut().enumerate() {
        let x = (i as f64 - 89.0) * FRAC_PI_2;
        let sinc = if x == 0.0 { 1.0 } else { x.sin() / x };
        *tap = (sinc * 0.5) as f32;
    }
    for (i, tap) in taps.iter_mut().enumerate() {
        let window = (i as f64 * (PI / 176.0)).sin();
        *tap = (f64::from(*tap) * window) as f32;
    }
    taps
}

/// One side of the engine's 2x decimator (ops 10 and 12): two samples at
/// twice the rate in, one out. A ring of 256 floats that is written
/// backwards, newest first, and read against the taps in two runs either
/// side of its wrap.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Decimator {
    ring: [f32; 256],
    /// Where the next sample is written.
    at: usize,
}

impl Default for Decimator {
    fn default() -> Self {
        Decimator {
            ring: [0.0; 256],
            at: 0,
        }
    }
}

impl Decimator {
    /// Take the even and the odd sample of a pair and give the filter's
    /// output at the odd one.
    pub(crate) fn step(&mut self, taps: &[f32; TAPS], even: f32, odd: f32) -> f32 {
        const MASK: usize = 255;
        self.ring[self.at] = even;
        self.at = self.at.wrapping_sub(1) & MASK;
        self.ring[self.at] = odd;
        let start = self.at;
        let end = (start + TAPS).min(256);
        let mut out = 0.0f32;
        dot(&self.ring[start..end], &taps[..end - start], &mut out);
        if start + TAPS > 256 {
            let rest = start + TAPS - 256;
            dot(&self.ring[..rest], &taps[end - start..], &mut out);
        }
        self.at = self.at.wrapping_sub(1) & MASK;
        out
    }
}

/// The engine's dot product (`FUN_1801e0fe0`): four running sums over the
/// whole groups of four, added to `out` one after the other, then the rest
/// one by one.
fn dot(samples: &[f32], taps: &[f32], out: &mut f32) {
    let whole = samples.len() / 4 * 4;
    let mut lanes = [0.0f32; 4];
    for (s, t) in samples[..whole].chunks_exact(4).zip(taps.chunks_exact(4)) {
        for lane in 0..4 {
            lanes[lane] += s[lane] * t[lane];
        }
    }
    let mut sum = *out;
    for lane in lanes {
        sum += lane;
    }
    for (s, t) in samples[whole..].iter().zip(&taps[whole..]) {
        sum += s * t;
    }
    *out = sum;
}

/// The filters the engine's designer (`dispatch`, `FUN_1801de630`) makes,
/// by the code the plugin asks for each with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Matched {
    LowPass = 2,
    HighPass = 3,
    LowShelf = 5,
    Peak = 6,
    HighShelf = 8,
    BandPass = 9,
    Notch = 10,
}

/// The engine's matched second order filter. `f` is the frequency over
/// half the rate, `q` the analog prototype's Q, `db` the gain; all three
/// come in as floats, as `dispatch` takes them. The poles and the
/// magnitude are fitted to the analog prototype at DC, at Nyquist and at
/// a point between, so the top of the band does not cramp.
pub(crate) fn matched(kind: Matched, f: f32, q: f32, db: f32) -> Coefficients64 {
    let (f, q, db) = (f64::from(f), f64::from(q), f64::from(db));
    match kind {
        Matched::LowPass => low_pass(f, q),
        Matched::HighPass => high_pass(f, q),
        Matched::BandPass => band_pass(f, q),
        Matched::Notch => notch(f, q),
        Matched::Peak => peak(f, q, gain(db)),
        Matched::LowShelf => low_shelf(f, q, gain(db)),
        Matched::HighShelf => high_shelf(f, q, gain(db)),
    }
}

/// `pow(10, dB * 0.05)`, as the designer starts.
fn gain(db: f64) -> f64 {
    10f64.powf(db * 0.05)
}

/// The square root the DLL takes of what may be at or under 0: 0 then.
fn root(x: f64) -> f64 {
    if x > 0.0 { x.sqrt() } else { 0.0 }
}

/// `minsd a, b` and `maxsd a, b`: `b` unless `a` is past it.
fn min_sd(a: f64, b: f64) -> f64 {
    if a < b { a } else { b }
}

fn max_sd(a: f64, b: f64) -> f64 {
    if a > b { a } else { b }
}

/// What a gain this close to unity gets: a wire.
const FLAT: Coefficients64 = Coefficients64::IDENTITY;

fn is_flat(g: f64) -> bool {
    // `|g - 1| < 0.001`, as the DLL's branch has it: a NaN is flat too.
    !matches!(
        (g - 1.0).abs().partial_cmp(&0.001),
        Some(Ordering::Greater | Ordering::Equal)
    )
}

/// What every design but the pole-only ones ends in: `p` and `z` are the
/// squared sums that fix the poles and the zeros, `dp` and `dz` the
/// products, and the five come out over the leading coefficient. `scale`
/// is the low shelf's gain on the zeros.
fn factor(p: f64, z: f64, dp: f64, dz: f64, scale: f64) -> Coefficients64 {
    let pole = (root(p) + 1.0) * 0.5;
    let zero = (root(z) + 1.0) * 0.5;
    let norm = 1.0 / (root(pole * pole + dp) + pole);
    let a1 = 1.0 - root(p);
    let a2 = -(norm * dp);
    let b0 = root(zero * zero + dz) + zero;
    let b1 = 1.0 - root(z);
    let b2 = -(dz / b0);
    let n = norm * scale;
    Coefficients64 {
        b0: n * b0,
        b1: n * b1,
        b2: n * b2,
        a1: norm * a1,
        a2: norm * a2,
    }
}

/// `(2 / pi)^4` and `4 / pi^2` as the DLL has them.
const TWO_OVER_PI_4: f64 = 0.164_255_716_074_949_38;
const FOUR_OVER_PI_2: f64 = 0.405_284_734_569_351_1;

/// Case 2.
fn low_pass(f: f64, q: f64) -> Coefficients64 {
    let mut iq2 = 1.0 / (q * q);
    if (1.99..2.0).contains(&iq2) {
        iq2 = 1.99;
    }
    let slope = (FRAC_2_PI / f) * (FRAC_2_PI / f) * (iq2 - 2.0);
    let f2 = f * f;
    let f4 = f2 * f2;
    let t = f2 - 1.0;
    let den = iq2 * f * f + t * t;
    let nyquist = f4 / den;
    let peaked = iq2 < 2.0 && (1.0 - iq2 * 0.5) * f * f < 1.0;
    let (p, z, dp, dz);
    if peaked {
        let r = 1.0 - iq2 * 0.5;
        let s = ((root(r) * f) * FRAC_PI_2).sin();
        let c = 1.0 - s * s;
        let top = 1.0 / (iq2 - iq2 * 0.25 * iq2);
        let k = top - 1.0;
        p = (c * k * c) / (((top - nyquist) * s) * ((s * s) * s));
        let s4 = 1.0 / ((s * s) * (s * s));
        z = (p / den) * f4;
        dp = (((p - z) - slope) * 0.25) / k - s4 * 0.25;
        dz = k * 0.25 * s4 + dp * top;
    } else {
        let fc = min_sd(f, 0.5);
        let s = (fc * FRAC_PI_2).sin();
        let gap = f * f - fc * fc;
        let fit = nyquist / (nyquist - 1.0);
        let at = f4 / ((fc * f) * (fc * f) * iq2 + gap * gap);
        let at = at / (at - 1.0);
        let d = (((at - fit) * slope + 1.0 / (s * s)) - 1.0) / (fit - at * (s * s));
        let t = (d + slope) * fit;
        z = -t;
        p = (slope - t) + d;
        dp = d * -0.25;
        dz = -0.0;
    }
    factor(p, z, dp, dz, 1.0)
}

/// Case 3.
fn high_pass(f: f64, q: f64) -> Coefficients64 {
    let c = TWO_OVER_PI_4 / ((f * f) * (f * f));
    let iq2 = 1.0 / (q * q);
    let f2 = f * f;
    let shelf = |fm: f64, s2: f64| {
        let p = ((f2 - 1.0) * (f2 - 1.0) + f2 * iq2) * c;
        let mut d = ((f2 * iq2) * fm) * fm + (f2 - fm * fm) * (f2 - fm * fm);
        d *= s2 * c;
        d /= (fm * fm) * (fm * fm);
        (p, ((d - p) * 0.25) / (1.0 - s2) - 0.25 / s2)
    };
    let (p, d);
    let half = iq2 * 0.5;
    if iq2 >= 2.0 {
        let fm = min_sd(f, 0.5);
        let s = (fm * FRAC_PI_2).sin();
        (p, d) = shelf(fm, s * s);
    } else if f2 >= 1.0 - half {
        let fm = min_sd(f, 0.5);
        let s = (fm * FRAC_PI_2).sin();
        let mix = min_sd(((f2 - 1.0) + half) * q, 1.0);
        let (pp, dd) = shelf(fm, s * s);
        let low = -0.25 - pp * 0.25;
        p = pp;
        d = (dd - low) * mix + low;
    } else {
        let s = ((f / root(1.0 - half)) * FRAC_PI_2).sin();
        let s2 = s * s;
        let k = 1.0 / (1.0 / (iq2 - (iq2 * 0.25) * iq2));
        d = -0.25 / (s2 * s2) - (k * c) * 0.25;
        p = (((c + c) * s2) * k + 1.0) - (4.0 - s2 * 8.0) * d;
    }
    let pole = (root(p) + 1.0) * 0.5;
    let b0 = root(c) * 0.5;
    let norm = 1.0 / (root(pole * pole + d) + pole);
    Coefficients64 {
        b0: norm * b0,
        b1: norm * -root(c),
        b2: norm * ((c * 0.25) / b0),
        a1: norm * (1.0 - root(p)),
        a2: norm * -(norm * d),
    }
}

/// The poles the band pass and the notch share: an analog pair at `f` with
/// this Q, by impulse invariance.
fn poles(f: f64, q: f64) -> (f64, f64) {
    let e = ((f * -FRAC_PI_2) / q).exp();
    let k = 1.0 / ((q * 4.0) * q);
    let x = if q > 0.5 {
        (((1.0 - k).sqrt() * PI) * f).cos()
    } else {
        (((k - 1.0).sqrt() * PI) * f).cosh()
    };
    ((e * x) * -2.0, e * e)
}

/// Case 10: the zeros on the unit circle at `f`, the gain at DC as the
/// poles have it.
fn notch(f: f64, q: f64) -> Coefficients64 {
    let (a1, a2) = poles(f, q);
    let b1 = (f * PI).cos() * -2.0;
    let k = ((a1 + 1.0) + a2) / ((1.0 + b1) + 1.0);
    Coefficients64 {
        b0: k,
        b1: k * b1,
        b2: k,
        a1,
        a2,
    }
}

/// Case 9.
fn band_pass(f: f64, q: f64) -> Coefficients64 {
    let (a1, a2) = poles(f, q);
    let s = (f * FRAC_PI_2).sin();
    let high = ((1.0 - a1) + a2) * ((1.0 - a1) + a2);
    let low = ((a1 + 1.0) + a2) * ((a1 + 1.0) + a2);
    let cross = a2 * -4.0;
    let s2 = s * s;
    let c2 = 1.0 - s2;
    let r2 = ((c2 - s2) * 4.0) * cross + (high - low);
    let b2v = ((s2 * high + c2 * low) + ((c2 * 4.0) * s2) * cross - r2 * s2) / ((s2 * 4.0) * s2);
    let b1v = ((s2 - c2) * 4.0) * b2v + r2;
    let b1 = b1v.sqrt() * -0.5;
    let b0 = ((b1v * 0.25 + b2v).sqrt() - b1) * 0.5;
    Coefficients64 {
        b0,
        b1,
        b2: (-b0) - b1,
        a1,
        a2,
    }
}

/// Case 6.
fn peak(f: f64, q: f64, g: f64) -> Coefficients64 {
    if is_flat(g) {
        return FLAT;
    }
    let q2 = q * q;
    let g2 = g * g;
    let f2 = f * f;
    let iq2 = 1.0 / q2;
    let m = root((q2 + 0.5) + root(q2 + 0.25));
    let edge = (f2 - 1.0) * (f2 - 1.0);
    if f >= 1.0 {
        let fq = q / m;
        let t = (fq * FRAC_PI_2).sin();
        let fq2 = fq * fq;
        let t2 = t * t;
        let near = (f2 - fq2) * (f2 - fq2);
        let hn = ((f2 * g) * iq2 + edge) / ((f2 * iq2) * (1.0 / g) + edge);
        let hm = (((fq2 * f2) * g) * iq2 + near) / (((fq2 * f2) * iq2) * (1.0 / g) + near);
        let p = ((1.0 - t2) * (hm - 1.0)) / ((hn - hm) * t2);
        return factor(p, hn * p, 0.0, 0.0, 1.0);
    }
    let s = (f * FRAC_PI_2).sin();
    let s2 = s * s;
    let t = (((f * q) / m) * FRAC_PI_2).sin();
    let t2 = t * t;
    let inv = 1.0 / s2;
    let h = ((f2 * g) * iq2 + edge) / ((f2 * iq2) / g + edge);
    let k = (1.0 - g2) * inv;
    let u = ((inv * t2 - 2.0) * k + (1.0 - g) / t2) / (g - g2);
    let v = ((((inv - 2.0) * k + (1.0 - h)) / (h - g2)) - u) / (1.0 - t2);
    let w = u - v * t2;
    let x = (1.0 - g2) / (s2 * s2) + v * g2;
    let p = (w + 1.0) + v;
    let z = (((((g2 - 1.0) + g2) - 1.0) * inv + w * g2) + 1.0) + x;
    factor(p, z, v * -0.25, x * -0.25, 1.0)
}

/// What tells the two shelves apart, worked out before the part they
/// share. The high shelf is the low one with the roles of its two ends
/// swapped, and the DLL rounds each its own way on the way in.
struct Shelf {
    q: f64,
    /// The analog magnitude squared at a squared frequency `x` is
    /// `((num - x)^2 q^2 + num x) scale / ((den - x)^2 q^2 + den x)`.
    num: f64,
    den: f64,
    scale: f64,
    /// Where the low Q design matches, at most 0.2.
    low: f64,
    /// The corner of the pole pair, over the root of the gain.
    corner: f64,
    /// The scaled frequency the high Q design matches at.
    fp: f64,
    /// The prototype's two ends at the corner.
    corner_num: f64,
    corner_den: f64,
    /// `1 - g` for the high shelf, `g - 1` for the low one.
    lift: f64,
    /// The point a shelf past Nyquist matches at, at most 0.5.
    fm: f64,
    /// The gain on the zeros at the end.
    out: f64,
    g: f64,
}

impl Shelf {
    fn response(&self, num: f64, den: f64, x: f64) -> f64 {
        let q2 = self.q * self.q;
        ((((num - x) * (num - x)) * q2 + num * x) * self.scale)
            / (((den - x) * (den - x)) * q2 + den * x)
    }

    fn at(&self, x: f64) -> f64 {
        self.response(self.num, self.den, x)
    }

    fn design(&self) -> Coefficients64 {
        let q = self.q;
        let q2 = q * q;
        let iq2 = 1.0 / q2;
        if q2 <= 0.6 {
            // Low Q: poles by impulse invariance, zeros fitted.
            let x = self.low;
            let s = (x * FRAC_PI_2).sin();
            let s2 = s * s;
            let hm = self.at(x * x);
            let hn = self.at(1.0);
            let w = root(self.den) * FRAC_PI_2;
            let e = ((-1.0 / q) * w).exp();
            let c = if q > 0.5 {
                (w * root(4.0 - iq2)).cos()
            } else {
                (w * root(iq2 - 4.0)).cosh()
            };
            let a1 = (e * c) * -2.0;
            let e2 = e * e;
            let high = ((1.0 - a1) + e2) * ((1.0 - a1) + e2);
            let low = ((a1 + 1.0) + e2) * ((a1 + 1.0) + e2);
            let zeros = hn * high;
            let inv = 1.0 / low;
            let mut dz = (high * hm - zeros) / (1.0 - s2);
            dz += (hm - 1.0) / (inv * s2);
            dz = dz * 0.25 + (e2 * -4.0) * hm;
            return factor(
                high * inv,
                zeros * inv,
                (e2 * -4.0) * inv,
                dz * inv,
                self.out,
            );
        }
        let x14 = iq2 - 2.0;
        let c13 = self.corner;
        let fp = self.fp;
        let s50 = (fp * FRAC_PI_2).sin();
        let s50 = s50 * s50;
        let hp = self.at(fp * fp);
        let (y, x7, z, x10);
        if fp < c13 {
            let x = fp / c13;
            let s = (x * FRAC_PI_2).sin();
            let x5 = s * s;
            let x = x * x;
            let x3 = hp - 1.0;
            let h2 = self.at(x);
            let x1 = s50 * s50;
            let x4 = h2 - hp;
            let x2 = h2 - 1.0;
            y = ((s50 * x2 - x3 * x5) * -2.0) / ((s50 * x4) * x5);
            x7 = (x3 + x3) / s50 + hp * y;
            let x0 = (x5 * s50) * (x5 * s50) * x4;
            z = (x1 * x2 - (x5 * x5) * x3) / x0;
            x10 = hp * z - x3 / x1;
        } else if fp < 1.0 {
            let c2 = c13 * c13;
            let ha = self.response(self.corner_num, self.corner_den, c2);
            let hb = self.response(self.corner_num, self.corner_den, 1.0);
            let s = (c13 * FRAC_PI_2).sin();
            let s = s * s;
            let x1 = 1.0 / s;
            let x2 = 2.0 - (x1 + x1);
            let x2 = ((x2 * (hb - 1.0)) * (ha - 1.0)) / (hb - ha);
            let x14 = x14 * FOUR_OVER_PI_2;
            let x0 = ((c13 * root(self.g)) * x2) / (self.lift * x14);
            let power = fp.powf(x0.ln() / c13.ln());
            let x3 = power * (((self.num - self.den) * x14) / (self.den * self.num));
            let hn = self.at(1.0);
            let x12 = -2.0 / s50 - x3 / (hp - 1.0);
            let x2 = (hp - 1.0) / (s50 * s50);
            x7 = x12 - x3;
            let x9 = ((((hn + x2) - 1.0) - x7) + hn * x12) / (hp - hn);
            y = x12;
            z = x9;
            x10 = hp * x9 - x2;
        } else {
            let x12 = (FOUR_OVER_PI_2 / self.den - FOUR_OVER_PI_2 / self.num) * x14;
            let hn = self.at(1.0);
            let fm = self.fm;
            let s = (fm * FRAC_PI_2).sin();
            let s3 = s * s;
            let x6 = hn / (hn - 1.0);
            let hm = self.at(fm * fm);
            let x2 = hm / (hm - 1.0);
            let x9 = (((x2 - x6) * x12 + 1.0 / s3) - 1.0) / (x6 - x2 * s3);
            x7 = -((x12 + x9) * x6) - 1.0;
            y = x12 + x7;
            z = x9;
            x10 = 0.0;
        }
        factor(
            (y + 1.0) + z,
            (x7 + 1.0) + x10,
            z * -0.25,
            x10 * -0.25,
            self.out,
        )
    }
}

/// `sqrt(max(1/g, g))` and its root, as both shelves start.
fn shelf_roots(g: f64) -> (f64, f64) {
    let most = max_sd(1.0 / g, g);
    if most > 0.0 {
        let r = most.sqrt();
        (r, root(r))
    } else {
        (0.0, 0.0)
    }
}

/// The pole corner of the high Q shelves, from `r`.
fn shelf_corner(q: f64, r: f64) -> f64 {
    let x14 = 1.0 / (q * q) - 2.0;
    let c = -((r * r + 1.0) / (x14 * r));
    c - root(c * c - 1.0)
}

/// Case 8.
fn high_shelf(f: f64, q: f64, g: f64) -> Coefficients64 {
    if is_flat(g) {
        return FLAT;
    }
    let (r, r4) = shelf_roots(g);
    let (low, high) = (min_sd(g, 1.0), max_sd(g, 1.0));
    let scaled = f / r4;
    let scaled2 = scaled * scaled;
    let corner = shelf_corner(q, r);
    let pole = corner / r;
    Shelf {
        q,
        num: scaled2 / low,
        den: high * scaled2,
        scale: g * g,
        low: min_sd(scaled * root(r), 0.2),
        corner,
        fp: scaled * root(r * corner),
        corner_num: pole / low,
        corner_den: pole * high,
        lift: 1.0 - g,
        fm: min_sd(scaled, 0.5),
        out: 1.0,
        g,
    }
    .design()
}

/// Case 5.
fn low_shelf(f: f64, q: f64, g: f64) -> Coefficients64 {
    if is_flat(g) {
        return FLAT;
    }
    let (r, r4) = shelf_roots(g);
    let (low, high) = (min_sd(g, 1.0), max_sd(g, 1.0));
    let scaled = r4 * f;
    let u = (scaled * scaled) / (r * r);
    let corner = shelf_corner(q, r);
    let inv_r = 1.0 / r;
    let pole = inv_r * corner;
    Shelf {
        q,
        num: high * u,
        den: u / low,
        scale: 1.0 / (g * g),
        low: min_sd(scaled / root(r), 0.2),
        corner,
        fp: scaled * root(pole),
        corner_num: high * pole,
        corner_den: pole / low,
        lift: g - 1.0,
        fm: min_sd(inv_r * scaled, 0.5),
        out: g,
        g,
    }
    .design()
}

#[cfg(test)]
mod tests {
    use super::Matched::*;
    use super::*;

    /// The size of a filter on the unit circle, `turns` of the rate.
    fn size(c: &Coefficients64, turns: f64) -> f64 {
        let w = std::f64::consts::TAU * turns;
        let at = |k: [f64; 3]| {
            let re = k[0] + k[1] * w.cos() + k[2] * (2.0 * w).cos();
            let im = k[1] * w.sin() + k[2] * (2.0 * w).sin();
            re.hypot(im)
        };
        at([c.b0, c.b1, c.b2]) / at([1.0, c.a1, c.a2])
    }

    #[test]
    fn the_designs_are_the_dll_s() {
        // What `dispatch` gives for these, run under an emulator: one or
        // more for each branch of each design. Rust's libm and the DLL's
        // can be a bit apart on a sine or a logarithm.
        #[rustfmt::skip]
        let table: [(Matched, f32, f32, f32, [f64; 5]); 32] = [
            (LowPass, 0.05, 0.7, 0.0, [0.015503719834984466, 0.006564414148585, 0.0, -1.7769238190115964, 0.798991952995166]),
            (LowPass, 0.05, 2.0, 0.0, [0.016981103143924585, 0.0070485647028377734, -0.0003484560809273908, -1.900784179986459, 0.924465391752294]),
            (LowPass, 0.3, 0.3, 0.0, [0.19304706254134124, 0.07892380976761038, 0.0, -0.721252880310408, -0.006776247380640316]),
            (LowPass, 0.95, 4.0, 0.0, [1.6152463374113424, 1.4270995206472628, 0.03637469790345443, 1.5074254434581837, 0.5712951125038759]),
            (HighPass, 0.05, 0.3, 0.0, [0.7813522114136875, -1.562704422827375, 0.7813522114136875, -1.570766961990385, 0.5900460556263705]),
            (HighPass, 0.05, 2.0, 0.0, [0.959761462150266, -1.919522924300532, 0.959761462150266, -1.9007840220140062, 0.9244651875968454]),
            (HighPass, 0.9, 2.0, 0.0, [0.25881807183673466, -0.5176361436734693, 0.2588180718367347, 0.7839185422901869, 0.2851712525993384]),
            (BandPass, 0.1, 1.4, 0.0, [0.18071643070173768, -0.16207103005467438, -0.018645400647063298, -1.7113137767475952, 0.7989958857951066]),
            (BandPass, 0.02, 0.4, 0.0, [0.13198977539767076, -0.11866334712464574, -0.01332642827302502, -1.850983809756843, 0.8546360041542833]),
            (Notch, 0.1, 1.4, 0.0, [0.895748522772462, -1.7038149364974127, 0.895748522772462, -1.7113137767475952, 0.7989958857951066]),
            (Notch, 0.02, 0.4, 0.0, [0.9254160990758796, -1.847180003754319, 0.9254160990758796, -1.850983809756843, 0.8546360041542833]),
            (Peak, 0.1, 1.4, 6.0, [1.0765384719359365, -1.769634534316363, 0.7836840848224204, -1.7626356932528588, 0.8532237156948526]),
            (Peak, 1.2, 0.8, -6.0, [0.7761763760323771, 0.2689979060719533, -0.0, 0.04517428210433049, -0.0]),
            (Peak, 0.5, 3.0, 0.005, [1.0, 0.0, 0.0, 0.0, 0.0]),
            (LowShelf, 0.02, 0.5, 6.0, [1.0216386458175344, -1.896228792954306, 0.8798807026046096, -1.8970134210826042, 0.8996649799418814]),
            (LowShelf, 0.02, 1.0, 9.0, [1.0167762029415668, -1.9475878447983663, 0.9372798950344884, -1.950368905435461, 0.9526639282678574]),
            (LowShelf, 0.3, 1.0, -9.0, [0.7484435677893155, -0.8426983829959303, 0.3637198076312995, -0.5437663529406213, 0.3032218881636577]),
            (LowShelf, 0.8, 1.0, 12.0, [2.998410475560053, 1.547173849494268, 0.11282686686122431, -0.009366088718906096, 0.17950607525540863]),
            (LowShelf, 0.95, 2.0, -12.0, [0.2376784091471031, 0.13426229651522018, 0.08888610468410164, 0.7609982309810367, 0.07358634484104676]),
            (LowShelf, 0.5, 1.5, 3.0, [1.1662585574925517, 0.02762442007718222, 0.3954672769401087, -0.2743130939970919, 0.39948690658719177]),
            (LowShelf, 1.3, 1.0, 12.0, [4.0297310888682425, 2.340109672592033, 0.008290678954935191, 0.5370189330238436, 0.06509524933251853]),
            (LowShelf, 1.5, 3.0, -12.0, [0.18923836977016928, 0.10204443654910961, 0.0, 0.16733942223239154, -0.007721683685886632]),
            (LowShelf, 1.2, 0.8, 15.0, [4.925888770298935, 2.255972340968563, -0.061724806841781235, 0.22320711821678568, 0.04295206046630219]),
            (HighShelf, 0.02, 0.5, 6.0, [1.9530054003736117, -3.704915621380836, 1.757087841819039, -1.856088627109398, 0.8612662479212125]),
            (HighShelf, 0.02, 1.0, 9.0, [2.7718812882429615, -5.4061910741474435, 2.640671316749692, -1.9154538030728174, 0.921815333918028]),
            (HighShelf, 0.3, 1.0, -9.0, [0.47406832592815407, -0.2577824046346178, 0.14374789290651907, -1.1259344314829451, 0.4859682456830003]),
            (HighShelf, 0.8, 1.0, 12.0, [1.3277273868886745, -0.012435612500120857, 0.23833513222950595, 0.51599801364931, 0.037628892968749425]),
            (HighShelf, 0.95, 2.0, -12.0, [1.0568424959268907, 0.8042552698259527, 0.07776917634794919, 0.5648905889138754, 0.3739763531869173]),
            (HighShelf, 0.5, 1.5, 3.0, [1.211170143659825, -0.3322398294642288, 0.48384661404142826, 0.023686360026866356, 0.33909056821015804]),
            (HighShelf, 1.3, 1.0, 12.0, [0.9879249055929099, 0.5305343787091858, 0.06430921805137615, 0.5807111246346854, 0.0020573777187863954]),
            (HighShelf, 1.5, 3.0, -12.0, [1.326236006666589, 0.31018298051415455, 0.0, 0.6044374709004875, 0.03198151628025607]),
            (HighShelf, 1.2, 0.8, 15.0, [1.141603782409855, 0.25481409041708586, 0.04903423469062855, 0.4579828019201613, -0.012530694402592024]),
        ];
        for (kind, f, q, db, want) in table {
            let got = matched(kind, f, q, db);
            let got = [got.b0, got.b1, got.b2, got.a1, got.a2];
            for (g, w) in got.iter().zip(want) {
                let err = (g - w).abs() / w.abs().max(1.0);
                assert!(
                    err < 1e-12,
                    "{kind:?} {f} {q} {db}: {got:?} and not {want:?}"
                );
            }
        }
    }

    #[test]
    fn they_match_the_analog_filter_at_dc_and_at_nyquist() {
        for (f, q) in [
            (0.01f32, 0.5f32),
            (0.1, 0.707),
            (0.3, 2.0),
            (0.7, 5.0),
            (0.9, 0.3),
        ] {
            let (fd, qd) = (f64::from(f), f64::from(q));
            // The analog filters at Nyquist, `1 / f` of their corner.
            let w = 1.0 / fd;
            let pair = (1.0 - w * w) * (1.0 - w * w) + (w / qd) * (w / qd);
            let low = matched(LowPass, f, q, 0.0);
            assert!((size(&low, 0.0) - 1.0).abs() < 1e-9, "{f} {q}");
            assert!(
                (size(&low, 0.5) - (1.0 / pair).sqrt()).abs() < 1e-9,
                "{f} {q}"
            );
            let high = matched(HighPass, f, q, 0.0);
            assert!(size(&high, 0.0) < 1e-9, "{f} {q}");
            // and with no resonance it is the analog's at Nyquist too.
            if q <= 0.707 {
                let want = w * w / pair.sqrt();
                assert!((size(&high, 0.5) - want).abs() < 1e-6, "{f} {q}");
            }
            // The notch is 0 at its frequency and the band pass about 1.
            let notch = matched(Notch, f, q, 0.0);
            assert!(size(&notch, fd / 2.0) < 1e-9, "{f} {q}");
            let band = matched(BandPass, f, q, 0.0);
            assert!((size(&band, fd / 2.0) - 1.0).abs() < 0.05, "{f} {q}");
            for db in [-12.0f32, 6.0] {
                let g = 10f64.powf(f64::from(db) / 20.0);
                // The peak is unity at DC and the analog's at Nyquist.
                let peak = matched(Peak, f, q, db);
                let a = g.sqrt();
                let num = (1.0 - w * w) * (1.0 - w * w) + (w * a / qd) * (w * a / qd);
                let den = (1.0 - w * w) * (1.0 - w * w) + (w / (a * qd)) * (w / (a * qd));
                assert!((size(&peak, 0.0) - 1.0).abs() < 1e-9, "{f} {q} {db}");
                assert!(
                    (size(&peak, 0.5) - (num / den).sqrt()).abs() < 1e-6,
                    "{f} {q} {db}"
                );
                // The shelves have their gain at their end and none at
                // the other.
                let low = matched(LowShelf, f, q, db);
                assert!((size(&low, 0.0) - g).abs() < 1e-9 * g, "{f} {q} {db}");
                let high = matched(HighShelf, f, q, db);
                assert!((size(&high, 0.0) - 1.0).abs() < 1e-9, "{f} {q} {db}");
            }
        }
        // A gain within 0.001 of unity is a wire.
        for kind in [Peak, LowShelf, HighShelf] {
            assert_eq!(matched(kind, 0.1, 1.0, 0.008), Coefficients64::IDENTITY);
            assert_ne!(matched(kind, 0.1, 1.0, 0.009), Coefficients64::IDENTITY);
        }
    }
}
