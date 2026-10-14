//! What the automation clips of a project drive.
//!
//! A clip does not store its target. A 227 controller link does: its
//! source is the automation channel and its target is a REC control ID.
//! The clip's 0..=1 covers the part of the target's range that the
//! channel's 219 gives. `docs/automation-234.md` has all three.

use crate::rec::RecTarget;
use crate::{Channel, ChannelKind, LinkKind, Project};

/// One control an automation clip drives.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Binding {
    /// The automation channel, by the index its 64 has.
    pub channel: u16,
    pub target: RecTarget,
    /// The target's raw value where the clip is at 0. In between, the raw
    /// value is on the straight line from here to `max_raw`. `None` for a
    /// target without a [`RecTarget::raw_range`].
    pub min_raw: Option<f64>,
    /// The target's raw value where the clip is at 1. `None` together with
    /// `min_raw`.
    pub max_raw: Option<f64>,
}

impl Channel {
    /// The part of its target's range an automation clip covers, from 219
    /// @0 and @4: where the clip's 0 and its 1 land, as 12800ths of the
    /// range. A channel without a 219 covers all of it, 0 to 12800, which
    /// is what FL gives a new automation channel.
    ///
    /// These are the bytes [`Channel::pan`] and [`Channel::volume`] read on
    /// any other kind of channel.
    pub fn automation_range(&self) -> (i32, i32) {
        self.levels
            .map_or((0, 12_800), |l| (l.pan as i32, l.volume as i32))
    }
}

impl Project {
    /// Every control an automation clip drives, in the order of the links.
    ///
    /// A binding is a 227 whose source is `channel << 16` for a channel of
    /// the automation kind. One clip can have several, and a clip without
    /// a link has none. The target is decoded as a 225 record's ID is,
    /// with the remap for a legacy project.
    pub fn automation_bindings(&self) -> Vec<Binding> {
        let remote = |link: &&crate::ControllerLink| link.kind == LinkKind::Remote;
        let links = self.controller_links.iter().filter(remote);
        let bindings = links.filter_map(|link| {
            let link = link.link?;
            // Control 0 of the channel. Its other controls are no clip.
            let RecTarget::Channel { index, control: 0 } = self.rec_target(link.source) else {
                return None;
            };
            let channel = self.channel(index)?;
            if channel.kind != Some(ChannelKind::Automation) {
                return None;
            }
            let target = self.rec_target(link.target);
            let (min, max) = channel.automation_range();
            let raw = |part: i32| {
                let (low, high) = target.raw_range()?;
                let fraction = f64::from(part) / 12_800.0;
                Some(f64::from(low) + fraction * f64::from(high - low))
            };
            Some(Binding {
                channel: index,
                target,
                min_raw: raw(min),
                max_raw: raw(max),
            })
        });
        bindings.collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ControllerLink;
    use crate::rec::control;
    use flp::event::layout;

    fn link(kind: LinkKind, source: u32, target: u32) -> ControllerLink {
        ControllerLink {
            kind,
            link: Some(layout::ControllerLink {
                source,
                unknown_4: [0; 4],
                target,
                flags: 8,
                unknown_16: [0; 4],
            }),
            ..ControllerLink::default()
        }
    }

    /// A project of FL 20 with a sampler channel 0 and automation channels
    /// 1 and 2. Channel 2 covers 1250 to 4250 of its target's range.
    fn project(links: &[(u32, u32)]) -> Project {
        let mut project = Project::new(flp::Header {
            format: 0,
            channel_count: 3,
            ppq: 96,
        });
        project.writer_version = Some("20.8.4.2576".to_owned());
        for (index, kind) in [
            (0, ChannelKind::Sampler),
            (1, ChannelKind::Automation),
            (2, ChannelKind::Automation),
        ] {
            project.channels.push(Channel {
                index,
                kind: Some(kind),
                ..Channel::default()
            });
        }
        project.channels[2].levels = layout::Levels::from_bytes(&{
            let mut bytes = [0; 24];
            bytes[..4].copy_from_slice(&1250i32.to_le_bytes());
            bytes[4..8].copy_from_slice(&4250i32.to_le_bytes());
            bytes
        });
        let links = links.iter();
        project.controller_links = links
            .map(|(source, target)| link(LinkKind::Remote, *source, *target))
            .collect();
        project
    }

    #[test]
    fn a_remote_link_from_an_automation_channel_is_a_binding() {
        // Insert 1's volume under the ID a legacy project gives it.
        let p = project(&[(1 << 16, 0x2040_1FC0)]);
        let insert = RecTarget::Insert {
            index: 1,
            slot: None,
            control: control::VOLUME,
        };
        let bindings = p.automation_bindings();
        assert_eq!(
            bindings,
            [Binding {
                channel: 1,
                target: insert,
                min_raw: Some(0.0),
                max_raw: Some(16_000.0),
            }]
        );
    }

    #[test]
    fn the_219_of_the_clip_picks_the_part_of_the_range() {
        let p = project(&[(2 << 16, 0x4000_0005), (2 << 16, 0x2040_1FC1)]);
        let bindings = p.automation_bindings();
        // 60 to 180 BPM of FL's 10 to 522.
        assert_eq!(bindings[0].target, RecTarget::Global { control: 5 });
        assert_eq!(
            (bindings[0].min_raw, bindings[0].max_raw),
            (Some(60_000.0), Some(180_000.0))
        );
        // The same part of a pan's -6400..=6400.
        assert_eq!(
            (bindings[1].min_raw, bindings[1].max_raw),
            (Some(-5150.0), Some(-2150.0))
        );
    }

    #[test]
    fn a_target_without_a_range_is_a_binding_without_raw_values() {
        // A parameter of channel 0's plugin, and a slot's mix level.
        let p = project(&[(1 << 16, 0x8003), (1 << 16, 0x2043_1F01)]);
        let bindings = p.automation_bindings();
        assert_eq!(
            bindings[0].target,
            RecTarget::Channel {
                index: 0,
                control: 0x8003
            }
        );
        assert_eq!(
            bindings[1].target,
            RecTarget::Insert {
                index: 1,
                slot: Some(3),
                control: control::SLOT_MIX
            }
        );
        for binding in bindings {
            assert_eq!((binding.min_raw, binding.max_raw), (None, None));
        }
    }

    #[test]
    fn only_a_227_from_control_0_of_an_automation_channel_binds() {
        let volume = 0x2040_1FC0;
        // From the sampler, from control 1 of a clip, from a channel the
        // project lacks, and from a mixer ID.
        let mut p = project(&[
            (0, volume),
            ((1 << 16) + 1, volume),
            (7 << 16, volume),
            (0x2009_8001, volume),
        ]);
        assert_eq!(p.automation_bindings(), []);
        // A 226 is a MIDI link, whatever its source.
        p.controller_links = vec![link(LinkKind::Midi, 1 << 16, volume)];
        assert_eq!(p.automation_bindings(), []);
        // A link whose 20 bytes did not decode has no source.
        p.controller_links = vec![ControllerLink {
            kind: LinkKind::Remote,
            ..ControllerLink::default()
        }];
        assert_eq!(p.automation_bindings(), []);
        p.controller_links = vec![link(LinkKind::Remote, 1 << 16, volume)];
        assert_eq!(p.automation_bindings().len(), 1);
    }
}
