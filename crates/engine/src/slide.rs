//! Slide notes, as FL's host plays them (`docs/plugins/host-voice.md`,
//! section 5).
//!
//! A slide note starts no voice. `FUN_00d2b660` takes the voices of its
//! channel and its colour, finds the highest `rec+0x58` (the key a voice's
//! `InitLevels.Pitch` is headed for, `key * 100` until a slide moves it)
//! among those that are held, and moves every voice of the group that
//! still sounds by as much as takes that one to the slide note's key. A
//! voice is held from its note on to its note off; one that was let go of
//! slides along when another is held, and with none held nothing moves.
//!
//! The engine starts no voice for a slide note, on a sampler channel or a
//! plugin channel, and plays these steps instead: a generator hears of
//! them by [`plugins::Generator::set_note_pitch`], and a sampler voice
//! changes its speed.
//!
//! `FUN_00d26460` then sets the voice up to get there in a straight line
//! over the slide note's length plus one tick: `InitLevels.Pitch` takes a
//! step of `(target - pitch) / (length + 1)` on each of that many ticks
//! (`FUN_00d26250`, from `FUN_00d2ab30`), the first on the slide note's
//! own tick, in f32. A slide that comes while another runs starts from
//! where that one has got to, for the target it was headed for moved by
//! the new slide. FL slides the pan, the volume and the two mods of the
//! voice to the slide note's the same way, which [`Slides`] leaves out.
//!
//! The note's fine pitch is not in `InitLevels.Pitch` (FL keeps it in
//! `rec+0x1e0`), and neither is a slide note's.
//!
//! An arpeggiator with slide on moves `rec+0x1e0` of the note it sounds
//! ([`crate::arp`]). That adds to what slides do, and [`Slides::glide`]
//! takes it.

use crate::clock::Clock;
use crate::sequence::Played;

/// The notes of a song as slides find them, and the slides.
#[derive(Debug, Default)]
pub(crate) struct Slides {
    /// In the order they came.
    tracks: Vec<Track>,
    /// The steps of the slides that are over, as [`Step`]s.
    steps: Vec<Step>,
    /// The notes an arpeggiator glides, by their id, and the cents it
    /// has them at from each tick on, on top of what slides do.
    glides: Vec<(usize, Vec<(u64, f32)>)>,
}

/// A step of a slide: from output sample `at` on, note `id` is `cents` off
/// the pitch it started at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Step {
    pub id: usize,
    pub at: usize,
    pub cents: f32,
}

/// A note as slides find it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Track {
    /// What the note plays on, and its colour: the voices a slide note
    /// moves are those of its own.
    owner: usize,
    color: u8,
    id: usize,
    /// The ticks of its note on and its note off, and the output sample
    /// it stops sounding on.
    on: u64,
    off: u64,
    done: usize,
    /// `key * 100`, `InitLevels.Pitch` now (`rec+0x280`) and where a slide
    /// takes it (`rec+0x58`).
    key: f32,
    init: f32,
    target: f32,
    slide: Option<Slide>,
}

/// A slide of a note: from its tick on, `steps` ticks that each add `step`
/// to its `InitLevels.Pitch`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Slide {
    tick: u64,
    step: f32,
    steps: u64,
}

impl Slides {
    /// A note that `owner` plays under `id` and stops sounding on output
    /// sample `done`. Notes come in the order the song has them.
    pub fn note(&mut self, owner: usize, id: usize, played: &Played, done: usize) {
        let key = f32::from(played.note.key) * 100.0;
        self.tracks.push(Track {
            owner,
            color: played.note.color(),
            id,
            on: played.tick,
            off: played.tick + played.length.unwrap_or(0),
            done,
            key,
            init: key,
            target: key,
            slide: None,
        });
    }

    /// A slide note on `owner`. Whether it moves a note.
    pub fn slide(&mut self, owner: usize, played: &Played, clock: &Clock) -> bool {
        let note = &played.note;
        let tick = played.tick;
        let at = clock.sample(tick);
        let color = note.color();
        let group =
            |track: &&mut Track| track.owner == owner && track.color == color && track.on <= tick;
        let held = |track: &Track| tick < track.off;
        let highest = (self.tracks.iter_mut())
            .filter(group)
            .filter(|track| held(track))
            .map(|track| track.target)
            .reduce(f32::max);
        let Some(highest) = highest else {
            return false;
        };
        let delta = f32::from(note.key) * 100.0 - highest;
        let steps = played.length.unwrap_or(0) + 1;
        let tracks = self.tracks.iter_mut().filter(group);
        for track in tracks.filter(|track| held(track) || track.done > at) {
            finish(track, tick, clock, &mut self.steps);
            track.target += delta;
            track.slide = Some(Slide {
                tick,
                step: (track.target - track.init) / steps as f32,
                steps,
            });
        }
        true
    }

    /// The note `id`, the last one handed to [`Slides::note`], is moved by
    /// an arpeggiator to `points`: from each tick on, this many cents off
    /// the pitch it started at.
    pub fn glide(&mut self, id: usize, points: &[(u64, f32)]) {
        self.glides.push((id, points.to_vec()));
    }

    /// Every step of every slide and glide, the steps of a note in the
    /// order they come. A step on or past the sample its note stops
    /// sounding on is left out. A note that slides and glides is at the
    /// sum of the two.
    pub fn steps(mut self, clock: &Clock) -> Vec<Step> {
        for track in &mut self.tracks {
            finish(track, u64::MAX, clock, &mut self.steps);
        }
        for (id, points) in std::mem::take(&mut self.glides) {
            let Some(track) = self.tracks.iter().find(|t| t.id == id) else {
                continue;
            };
            let slides: Vec<(usize, f32)> = (self.steps.iter())
                .filter(|step| step.id == id)
                .map(|step| (step.at, step.cents))
                .collect();
            self.steps.retain(|step| step.id != id);
            let glides = points
                .iter()
                .map(|&(tick, cents)| (clock.sample(tick), cents));
            // Both in the order they come: walk them together.
            let mut changes: Vec<(usize, Option<f32>, Option<f32>)> = (slides.iter())
                .map(|&(at, cents)| (at, Some(cents), None))
                .chain(glides.map(|(at, cents)| (at, None, Some(cents))))
                .collect();
            changes.sort_by_key(|change| change.0);
            let (mut slide, mut glide) = (0.0, 0.0);
            for (k, &(at, s, g)) in changes.iter().enumerate() {
                slide = s.unwrap_or(slide);
                glide = g.unwrap_or(glide);
                let last_here = changes.get(k + 1).is_none_or(|next| next.0 != at);
                if last_here && at < track.done {
                    self.steps.push(Step {
                        id,
                        at,
                        cents: slide + glide,
                    });
                }
            }
        }
        self.steps
    }
}

/// The steps of a note's slide before `tick`, where another starts or,
/// with `u64::MAX`, for good, each on the sample its tick starts on. The
/// note's `InitLevels.Pitch` is where they leave it.
fn finish(track: &mut Track, tick: u64, clock: &Clock, steps: &mut Vec<Step>) {
    let Some(slide) = track.slide.take() else {
        return;
    };
    for k in 0..slide.steps.min(tick.saturating_sub(slide.tick)) {
        track.init += slide.step;
        let at = clock.sample(slide.tick + k);
        if at < track.done {
            steps.push(Step {
                id: track.id,
                at,
                cents: track.init - track.key,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::Note;

    fn played(tick: u64, length: Option<u64>, key: i16, color: u8, slide: bool) -> Played {
        Played {
            tick,
            length,
            note: Note {
                key,
                flags: color,
                tag: if slide { 0x4008 } else { 0x4000 },
                ..Note::default()
            },
        }
    }

    fn clock() -> Clock {
        // A tick is 250 samples.
        Clock::new(120.0, 96, 48_000).unwrap()
    }

    fn cents(steps: &[Step], id: usize) -> Vec<(usize, f32)> {
        let mine = steps.iter().filter(|s| s.id == id);
        mine.map(|s| (s.at, s.cents)).collect()
    }

    #[test]
    fn a_slide_takes_the_highest_held_note_to_its_key_and_the_rest_along() {
        let clock = clock();
        let mut slides = Slides::default();
        slides.note(0, 0, &played(0, Some(96), 60, 0, false), 1_000_000);
        slides.note(0, 1, &played(0, Some(96), 64, 0, false), 1_000_000);
        // Another colour, and another owner.
        slides.note(0, 2, &played(0, Some(96), 64, 1, false), 1_000_000);
        slides.note(1, 3, &played(0, Some(96), 64, 0, false), 1_000_000);
        assert!(slides.slide(0, &played(10, Some(3), 68, 0, true), &clock));
        let steps = slides.steps(&clock);
        let want: Vec<_> = (1..=4)
            .map(|k| (2500 + 250 * (k - 1), k as f32 * 100.0))
            .collect();
        assert_eq!(cents(&steps, 0), want);
        assert_eq!(cents(&steps, 1), want);
        assert_eq!((cents(&steps, 2), cents(&steps, 3)), (vec![], vec![]));
    }

    #[test]
    fn a_slide_with_no_note_held_moves_nothing() {
        let clock = clock();
        let mut slides = Slides::default();
        // Let go of at tick 5, and sounding on to sample 10000.
        slides.note(0, 0, &played(0, Some(5), 60, 0, false), 10_000);
        assert!(!slides.slide(0, &played(10, Some(3), 68, 0, true), &clock));
        // With another note held, it slides along until it stops sounding.
        slides.note(0, 1, &played(8, Some(96), 62, 0, false), 1_000_000);
        assert!(slides.slide(0, &played(30, Some(95), 74, 0, true), &clock));
        let steps = slides.steps(&clock);
        assert_eq!(steps.iter().filter(|s| s.id == 1).count(), 96);
        let gone = cents(&steps, 0);
        assert_eq!(gone.len(), 10);
        assert!(gone.iter().all(|(at, _)| *at < 10_000));
        assert_eq!(gone[0], (7500, 12.5));
    }

    #[test]
    fn a_slide_that_comes_while_another_runs_starts_where_it_got_to() {
        let clock = clock();
        let mut slides = Slides::default();
        slides.note(0, 0, &played(0, Some(384), 60, 0, false), usize::MAX);
        // Up an octave over 96 ticks, and after 48 of them down to 62.
        assert!(slides.slide(0, &played(0, Some(95), 72, 0, true), &clock));
        assert!(slides.slide(0, &played(48, Some(1), 62, 0, true), &clock));
        let steps = cents(&slides.steps(&clock), 0);
        assert_eq!(steps.len(), 50);
        assert_eq!(steps[47], (47 * 250, 600.0));
        // From 600 cents to 200 in two ticks.
        assert_eq!(steps[48..], [(48 * 250, 400.0), (49 * 250, 200.0)]);
    }
}
