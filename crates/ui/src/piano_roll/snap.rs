//! What the piano roll snaps to, and the scale whose keys it highlights.

use crate::Grid;
use crate::playlist::snap_step;

/// The piano roll's snap, as FL's menu of it lists them.
///
/// Line snaps to the lines the grid draws at the zoom, which is what FL's
/// piano roll opens on through its "Main" snap. Cell has the same step,
/// but a note that is moved keeps where it is inside its cell: the move
/// goes in whole cells, and an off-grid note stays off the grid by as
/// much. The fractions are of a step of the step sequencer, a quarter of
/// a beat, and of a beat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Snap {
    None,
    Line,
    Cell,
    /// A step divided by this.
    Step(u8),
    /// A beat divided by this.
    Beat(u8),
    Bar,
}

impl Snap {
    pub const ALL: [Snap; 14] = [
        Snap::None,
        Snap::Line,
        Snap::Cell,
        Snap::Step(6),
        Snap::Step(4),
        Snap::Step(3),
        Snap::Step(2),
        Snap::Step(1),
        Snap::Beat(6),
        Snap::Beat(4),
        Snap::Beat(3),
        Snap::Beat(2),
        Snap::Beat(1),
        Snap::Bar,
    ];

    pub fn name(self) -> String {
        match self {
            Snap::None => "(none)".to_owned(),
            Snap::Line => "Line".to_owned(),
            Snap::Cell => "Cell".to_owned(),
            Snap::Step(1) => "Step".to_owned(),
            Snap::Step(n) => format!("1/{n} step"),
            Snap::Beat(1) => "Beat".to_owned(),
            Snap::Beat(n) => format!("1/{n} beat"),
            Snap::Bar => "Bar".to_owned(),
        }
    }

    /// How many ticks the snap's step is, a tick at least. `scale` is the
    /// zoom in points per tick, which the grid's lines follow.
    pub fn ticks(self, grid: Grid, scale: f32) -> u64 {
        let ticks = match self {
            Snap::None => 1,
            Snap::Line | Snap::Cell => snap_step(grid, scale),
            Snap::Step(n) => grid.beat / 4 / u64::from(n.max(1)),
            Snap::Beat(n) => grid.beat / u64::from(n.max(1)),
            Snap::Bar => grid.bar,
        };
        ticks.max(1)
    }
}

/// A scale, as the semitones above its root that are in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Scale {
    Major,
    Minor,
    HarmonicMinor,
    MelodicMinor,
    Dorian,
    Phrygian,
    Lydian,
    Mixolydian,
    Locrian,
    MajorPentatonic,
    MinorPentatonic,
    Blues,
}

impl Scale {
    pub const ALL: [Scale; 12] = [
        Scale::Major,
        Scale::Minor,
        Scale::HarmonicMinor,
        Scale::MelodicMinor,
        Scale::Dorian,
        Scale::Phrygian,
        Scale::Lydian,
        Scale::Mixolydian,
        Scale::Locrian,
        Scale::MajorPentatonic,
        Scale::MinorPentatonic,
        Scale::Blues,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Scale::Major => "Major",
            Scale::Minor => "Minor",
            Scale::HarmonicMinor => "Harmonic minor",
            Scale::MelodicMinor => "Melodic minor",
            Scale::Dorian => "Dorian",
            Scale::Phrygian => "Phrygian",
            Scale::Lydian => "Lydian",
            Scale::Mixolydian => "Mixolydian",
            Scale::Locrian => "Locrian",
            Scale::MajorPentatonic => "Major pentatonic",
            Scale::MinorPentatonic => "Minor pentatonic",
            Scale::Blues => "Blues",
        }
    }

    fn steps(self) -> &'static [u8] {
        match self {
            Scale::Major => &[0, 2, 4, 5, 7, 9, 11],
            Scale::Minor => &[0, 2, 3, 5, 7, 8, 10],
            Scale::HarmonicMinor => &[0, 2, 3, 5, 7, 8, 11],
            Scale::MelodicMinor => &[0, 2, 3, 5, 7, 9, 11],
            Scale::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            Scale::Phrygian => &[0, 1, 3, 5, 7, 8, 10],
            Scale::Lydian => &[0, 2, 4, 6, 7, 9, 11],
            Scale::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
            Scale::Locrian => &[0, 1, 3, 5, 6, 8, 10],
            Scale::MajorPentatonic => &[0, 2, 4, 7, 9],
            Scale::MinorPentatonic => &[0, 3, 5, 7, 10],
            Scale::Blues => &[0, 3, 5, 6, 7, 10],
        }
    }

    /// The keys of the scale on `root` (0 is C) as bits: bit n for the
    /// keys n semitones above a C.
    pub fn keys(self, root: u8) -> u16 {
        let bit = |step: &u8| 1 << ((u16::from(*step) + u16::from(root)) % 12);
        self.steps().iter().map(bit).fold(0, |keys, bit| keys | bit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_snaps_are_fl_s_steps() {
        let grid = Grid { beat: 96, bar: 384 };
        let ticks = |snap: Snap| snap.ticks(grid, 64.0 / 96.0);
        assert_eq!(ticks(Snap::None), 1);
        // At 64 points a beat the lines are a step apart.
        assert_eq!((ticks(Snap::Line), ticks(Snap::Cell)), (24, 24));
        let steps = [6, 4, 3, 2, 1].map(|n| ticks(Snap::Step(n)));
        assert_eq!(steps, [4, 6, 8, 12, 24]);
        let beats = [6, 4, 3, 2, 1].map(|n| ticks(Snap::Beat(n)));
        assert_eq!(beats, [16, 24, 32, 48, 96]);
        assert_eq!(ticks(Snap::Bar), 384);
        assert_eq!(Snap::Step(6).ticks(Grid { beat: 1, bar: 4 }, 1.0), 1);
        let names: Vec<String> = Snap::ALL.iter().map(|snap| snap.name()).collect();
        assert_eq!(names[..4], ["(none)", "Line", "Cell", "1/6 step"]);
        assert_eq!(names[7..9], ["Step", "1/6 beat"]);
    }

    #[test]
    fn a_scale_s_keys_follow_its_root() {
        // C major is the white keys.
        assert_eq!(Scale::Major.keys(0), 0b1010_1011_0101);
        // A minor has the same keys.
        assert_eq!(Scale::Minor.keys(9), Scale::Major.keys(0));
        // D dorian too.
        assert_eq!(Scale::Dorian.keys(2), Scale::Major.keys(0));
        assert_eq!(Scale::MinorPentatonic.keys(9).count_ones(), 5);
    }
}
