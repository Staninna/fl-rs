//! The parts of a channel that FL plays and the engine leaves out, counted
//! over the channels that sound in a render, so that the corpus can say
//! which to build first. `docs/rec-channel.md`, "Left out", has what turns
//! each one on ([`model::Channel::time_stretch_on`] and the rest). The
//! instrument envelopes, the tracking of mod X and Y and the echo delay
//! are played ([`crate::instrument_env`], [`crate::echo`]), and so not
//! counted.

use model::{Channel, Project};
use std::collections::BTreeSet;
use std::ops::AddAssign;

/// How many channels have each part on that the engine does not play.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UnplayedParts {
    /// Sampler and audio clip channels with a note or a clip that sounds
    /// whose time stretch or formant shift (215, controls 14 and 15) is on.
    pub time_stretch: usize,
    /// TS404 channels with a note in the render. The engine plays none.
    pub ts404: usize,
}

impl UnplayedParts {
    /// The parts of the channels in `noted`, which have a note that
    /// sounds, and in `clipped`, which have an audio clip that does, and
    /// the TS404 channels in `ts404`, which have a note.
    pub(crate) fn of(
        project: &Project,
        noted: &BTreeSet<u16>,
        clipped: &BTreeSet<u16>,
        ts404: &BTreeSet<u16>,
    ) -> Self {
        let channels = |set: &BTreeSet<u16>| -> Vec<&Channel> {
            set.iter().filter_map(|&i| project.channel(i)).collect()
        };
        let (noted, clipped) = (channels(noted), channels(clipped));
        let sounding: BTreeSet<u16> = noted.iter().chain(&clipped).map(|c| c.index).collect();
        let sounding = channels(&sounding);
        let count = |of: &[&Channel], on: &dyn Fn(&Channel) -> bool| {
            of.iter().filter(|channel| on(channel)).count()
        };
        UnplayedParts {
            time_stretch: count(&sounding, &Channel::time_stretch_on),
            ts404: ts404.len(),
        }
    }

    /// The channels over all the parts: one with two parts on counts
    /// twice.
    pub fn total(&self) -> usize {
        self.time_stretch + self.ts404
    }
}

impl AddAssign for UnplayedParts {
    fn add_assign(&mut self, other: Self) {
        self.time_stretch += other.time_stretch;
        self.ts404 += other.ts404;
    }
}
