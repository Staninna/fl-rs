//! The channel rack's edits of whole channels: a channel's name, its
//! colour, its solo, and a clone of it.
//!
//! Like the edits in `pattern.rs`, each sets a field and sees to it that
//! the field has an event to be written in, see "Changing a project" in
//! the crate's docs. A channel index that names no channel is skipped.

use crate::codec::{Codec, Cx};
use crate::{ChannelKind, Item, Placed, Project};
use flp::event::Id;

/// Whether the entry is the event `id`, typed or raw.
fn is(placed: &Placed, id: Id) -> bool {
    match &placed.item {
        Item::Field(field) => *field == id,
        Item::Raw(e) => e.id == id.0,
    }
}

/// The place of the channel's event `id` that its field writes: the typed
/// entry, or the last raw one, which becomes the field's so that the
/// channel does not end up with two. `None` when it has neither.
fn find_field(order: &mut [Placed], id: Id) -> Option<usize> {
    if let Some(at) = order.iter().position(|p| p.item == Item::Field(id)) {
        return Some(at);
    }
    let at = order.iter().rposition(|p| is(p, id))?;
    order[at].item = Item::Field(id);
    Some(at)
}

impl Project {
    /// The string encoding of the file, from its writer version.
    fn strings(&self) -> Cx {
        let mut cx = Cx::default();
        if let Some(version) = &self.writer_version {
            cx.set_version(version);
        }
        cx
    }

    /// Name the channel, as the rack shows it: 203 `PluginID.Name`, or a
    /// 192 `ChannelID._Name` when that is the event the channel has.
    ///
    /// A channel with a typed 203 or 192 gets the name in the one the
    /// loader keeps, the later. One whose name stayed a raw event gets it
    /// in that event, so that it does not end up with two. One with
    /// neither gets a 203 behind its 212, which is where FL writes it, or
    /// behind its 64 when it has no 212: after a 64 a 203 is the channel's.
    ///
    /// An empty name is not set, because a channel goes by its name and
    /// has no number to fall back on as an insert does. Nor is a name the
    /// file's strings cannot hold: a character above U+00FF in a file from
    /// before 11.5.5, or any name in a project without a writer version.
    pub fn set_channel_name(&mut self, channel: u16, name: &str) {
        let cx = self.strings();
        let fits = |name: &str| {
            !name.is_empty()
                && !name.contains('\0')
                && name.to_owned().encode(Id::PluginName, &cx).is_some()
        };
        let Some(channel) = self.channels.iter_mut().find(|c| c.index == channel) else {
            return;
        };
        if channel.name.as_deref() == Some(name) || !fits(name) {
            return;
        }
        channel.name = Some(name.to_owned());
        let order = &mut channel.order;
        let names = [Id::PluginName, Id::ChannelName];
        let typed = |placed: &Placed| matches!(placed.item, Item::Field(id) if names.contains(&id));
        if order.iter().any(typed) {
            return;
        }
        let raw = |placed: &Placed| match &placed.item {
            Item::Raw(e) => names.into_iter().find(|id| id.0 == e.id),
            Item::Field(_) => None,
        };
        if let Some(placed) = order.iter_mut().rev().find(|placed| raw(placed).is_some()) {
            placed.item = Item::Field(raw(placed).unwrap_or(Id::PluginName));
            return;
        }
        let behind = order
            .iter()
            .rposition(|placed| is(placed, Id::PluginWrapper));
        let behind = behind.or_else(|| order.iter().position(|placed| is(placed, Id::ChannelNew)));
        if let Some(at) = behind {
            let (pos, item) = (order[at].pos, Item::Field(Id::PluginName));
            order.insert(at + 1, Placed { pos, item });
        }
    }

    /// Give the channel a colour, an RGBA u32 as
    /// [`Channel::color`](crate::Channel::color) holds it, in its 128
    /// `PluginID.Color`.
    ///
    /// The loader hands a 128 to the plugin the last 212 opened, and a 64
    /// leaves none open, so the channel's 128 has to come after its 212.
    /// A channel with a 128 gets the colour in it. One without gets a 128
    /// behind its 212 and the 203 and 155 that FL writes behind that,
    /// which is where a new sampler of FL's has it (`NEW_SAMPLER`).
    ///
    /// The 128's handler works out from the colour whether it is the
    /// channel's own, and after the load FL gives a channel whose colour
    /// is not its own the default of its kind. A colour of the default
    /// palette would count as not its own, so the channel's 41
    /// `PluginColorIsCustom`, which comes after the 128 and has the last
    /// word, is set, and a channel without one gets one right behind the
    /// 128, as FL writes it.
    ///
    /// A channel without a 212 has no plugin to take the colour, and a
    /// file from 3.5.3 or before has every channel in its kind's colour
    /// (`docs/event-dictionary.md`, row 128), so neither is changed.
    pub fn set_channel_color(&mut self, channel: u16, color: u32) {
        if self.version() <= 3_005_003 {
            return;
        }
        let Some(channel) = self.channels.iter_mut().find(|c| c.index == channel) else {
            return;
        };
        let order = &mut channel.order;
        let Some(wrapper) = order.iter().rposition(|p| is(p, Id::PluginWrapper)) else {
            return;
        };
        if channel.color == Some(color) && channel.color_is_custom != Some(false) {
            return;
        }
        channel.color = Some(color);
        let after = |id: Id, order: &mut Vec<Placed>, at: usize| {
            let pos = order[at].pos;
            order.insert(
                at + 1,
                Placed {
                    pos,
                    item: Item::Field(id),
                },
            );
            at + 1
        };
        let colour = match find_field(order, Id::PluginColor) {
            Some(at) => at,
            None => {
                // Behind the 212, and the 203 and 155 that follow it.
                let tail = [Id::PluginName, Id::ChannelName, Id::PluginIcon];
                let mut at = wrapper;
                while order
                    .get(at + 1)
                    .is_some_and(|p| tail.iter().any(|id| is(p, *id)))
                {
                    at += 1;
                }
                after(Id::PluginColor, order, at)
            }
        };
        channel.color_is_custom = Some(true);
        if find_field(order, Id::PluginColorIsCustom).is_none() {
            after(Id::PluginColorIsCustom, order, colour);
        }
    }

    /// Copy the channel to the end of the rack, as FL's "Clone channel"
    /// does, and give the copy's index: the one after the highest a
    /// channel has, which FL's loader makes the channels up to.
    ///
    /// The copy has every event of the channel, its plugin's state
    /// included, in their order, behind the last event that any channel
    /// has in the file, which is where [`Project::add_sampler_channel`]
    /// puts a new channel, and the header counts a channel more. Its name
    /// is the channel's, or its plugin's, with " #2" behind it, or the
    /// first number from 2 up that no other channel has with that name
    /// (inferred from FL's habit with copies, not from its code). A name of
    /// that form counts from its stem. The pattern's notes stay the
    /// channel's (inferred: FL is taken to clone the channel and not its
    /// steps).
    ///
    /// An automation clip is not cloned: what it drives is in a controller
    /// link that names its channel by index, so a copy would drive
    /// nothing. A project whose channels have no events was not read from
    /// a file and has no place for the copy's. Either gives `None`.
    pub fn clone_channel(&mut self, channel: u16) -> Option<u16> {
        let mut copy = self.channel(channel)?.clone();
        if copy.kind == Some(ChannelKind::Automation) {
            return None;
        }
        let placed = self.channels.iter().flat_map(|channel| &channel.order);
        let pos = placed.map(|placed| placed.pos).max()?;
        let last = self.channels.iter().map(|channel| channel.index).max()?;
        let index = last.checked_add(1)?;
        copy.index = index;
        for placed in &mut copy.order {
            placed.pos = pos;
        }
        let plugin = copy.plugin_name.clone().filter(|name| !name.is_empty());
        let base = copy.name.clone().or(plugin);
        self.channels.push(copy);
        self.header.channel_count = self.header.channel_count.saturating_add(1);
        if let Some(base) = base {
            let name = self.copy_name(&base);
            self.set_channel_name(index, &name);
        }
        Some(index)
    }

    /// `base` with " #n" behind it, for the first n from 2 up that no
    /// channel's name has. A base of that form already counts from its
    /// stem.
    fn copy_name(&self, base: &str) -> String {
        let numbered = base.rsplit_once(" #");
        let stem = numbered.filter(|(_, n)| n.parse::<u32>().is_ok());
        let stem = stem.map_or(base, |(stem, _)| stem);
        let taken = |name: &str| {
            let names = self.channels.iter().filter_map(|c| c.name.as_deref());
            names.into_iter().any(|own| own == name)
        };
        (2u32..)
            .map(|n| format!("{stem} #{n}"))
            .find(|name| !taken(name))
            .unwrap_or_else(|| base.to_owned())
    }

    /// Solo the channel, as a right click on its switch does in FL: switch
    /// it on and every other channel off. When it is the one channel that
    /// is on already, every channel goes back on (inferred: FL is not
    /// known to remember which were on before the solo). A channel without
    /// a 0 counts as on.
    ///
    /// Only a channel whose switch changes is set, so one that keeps its
    /// state gets no event it did not have.
    pub fn solo_channel(&mut self, channel: u16) {
        let on = |c: &crate::Channel| c.enabled != Some(false);
        let Some(soloed) = self.channels.iter().find(|c| c.index == channel) else {
            return;
        };
        let others = self.channels.iter().filter(|c| c.index != channel);
        let alone = on(soloed) && !others.clone().any(on);
        let changes: Vec<(u16, bool)> = self
            .channels
            .iter()
            .map(|c| (c.index, alone || c.index == channel, on(c)))
            .filter(|(_, want, was)| want != was)
            .map(|(index, want, _)| (index, want))
            .collect();
        for (index, enabled) in changes {
            self.set_channel_enabled(index, enabled);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{from_flp, to_flp};
    use flp::RawEvent;

    fn ev(id: Id, data: &[u8]) -> RawEvent {
        RawEvent {
            id: id.0,
            data: data.to_vec(),
        }
    }

    fn word(id: Id, value: u16) -> RawEvent {
        ev(id, &value.to_le_bytes())
    }

    /// A string as files from 11.5.5 on store it.
    fn text(id: Id, text: &str) -> RawEvent {
        let units = text.encode_utf16().chain([0]);
        ev(id, &units.flat_map(u16::to_le_bytes).collect::<Vec<u8>>())
    }

    /// A 212 for a generator, flags 0x10.
    fn wrapper() -> RawEvent {
        let mut data = vec![0; 52];
        data[16] = 0x10;
        ev(Id::PluginWrapper, &data)
    }

    /// Four channels: a sampler named in a 203 behind its 212, one with
    /// nothing but its 64, a plugin without a name, and one named in an
    /// old 192. A pattern follows them.
    fn file() -> flp::Project {
        let events = vec![
            ev(Id::FLVersion, b"20.8.4.2576\0"),
            word(Id::ChannelNew, 0),
            ev(Id::ChannelType, &[0]),
            text(Id::PluginInternalName, ""),
            wrapper(),
            text(Id::PluginName, "Kick"),
            ev(Id::IsEnabled, &[1]),
            word(Id::ChannelNew, 1),
            word(Id::ChannelNew, 2),
            ev(Id::ChannelType, &[2]),
            text(Id::PluginInternalName, "Fruity DX10"),
            wrapper(),
            ev(Id::IsEnabled, &[0]),
            word(Id::ChannelNew, 3),
            text(Id::ChannelName, "Old"),
            word(Id::PatternNew, 1),
        ];
        let header = flp::Header {
            format: 0,
            channel_count: 4,
            ppq: 96,
        };
        flp::Project { header, events }
    }

    fn project() -> Project {
        let project = from_flp(&file());
        assert_eq!(to_flp(&project), Ok(file()));
        project
    }

    fn names(project: &Project) -> Vec<Option<&str>> {
        let channels = project.channels.iter();
        channels.map(|channel| channel.name.as_deref()).collect()
    }

    #[test]
    fn a_name_goes_into_the_event_the_channel_has() {
        let mut project = project();
        project.set_channel_name(0, "Kick 2");
        project.set_channel_name(3, "New");
        let mut want = file();
        want.events[5] = text(Id::PluginName, "Kick 2");
        want.events[14] = text(Id::ChannelName, "New");
        assert_eq!(to_flp(&project), Ok(want));
    }

    #[test]
    fn a_channel_without_a_name_gets_a_203_behind_its_212_or_its_64() {
        let mut project = project();
        project.set_channel_name(1, "Hat");
        project.set_channel_name(2, "Bell");
        let mut want = file();
        want.events.insert(12, text(Id::PluginName, "Bell"));
        want.events.insert(8, text(Id::PluginName, "Hat"));
        let written = to_flp(&project).unwrap();
        assert_eq!(written, want);
        // Read back, each name is its channel's.
        let read = from_flp(&written);
        assert_eq!(
            names(&read),
            [Some("Kick"), Some("Hat"), Some("Bell"), Some("Old")]
        );
        assert_eq!(read.channels[2].plugin_name.as_deref(), Some("Fruity DX10"));

        // A name that stayed raw takes the new one: a 203 without its end.
        let mut file = file();
        file.events[5].data.truncate(4);
        let mut project = from_flp(&file);
        assert_eq!(project.channels[0].name, None);
        project.set_channel_name(0, "Kick");
        assert_eq!(to_flp(&project).unwrap().events, self::file().events);
    }

    fn colour(value: u32) -> RawEvent {
        ev(Id::PluginColor, &value.to_le_bytes())
    }

    #[test]
    fn a_colour_goes_behind_the_212_with_a_41_that_says_it_is_its_own() {
        let mut project = project();
        for channel in 0..4 {
            project.set_channel_color(channel, 0x0050_70d0);
        }
        // Channel 0 has a 203 behind its 212, and channel 2 has none.
        // Channels 1 and 3 have no 212, so no plugin to take a colour.
        let mut want = file();
        let custom = ev(Id::PluginColorIsCustom, &[1]);
        want.events
            .splice(12..12, [colour(0x0050_70d0), custom.clone()]);
        want.events.splice(6..6, [colour(0x0050_70d0), custom]);
        let written = to_flp(&project).unwrap();
        assert_eq!(written, want);
        let read = from_flp(&written);
        let colours: Vec<_> = read.channels.iter().map(|c| c.color).collect();
        assert_eq!(colours, [Some(0x0050_70d0), None, Some(0x0050_70d0), None]);
        assert_eq!(read.channels[0].color_is_custom, Some(true));
        assert_eq!(read.channels[2].plugin_name.as_deref(), Some("Fruity DX10"));

        // A second colour goes into the same events.
        let mut project = read;
        project.set_channel_color(0, 0x00d0_9050);
        want.events[6] = colour(0x00d0_9050);
        assert_eq!(to_flp(&project), Ok(want));
    }

    #[test]
    fn a_colour_takes_the_128_and_the_41_the_channel_has() {
        // A channel as FL writes one: the grey of its kind, not its own.
        let mut file = file();
        let custom = |on: u8| ev(Id::PluginColorIsCustom, &[on]);
        file.events.splice(6..6, [colour(0x006a_655c), custom(0)]);
        let mut project = from_flp(&file);
        project.set_channel_color(0, 0x0040_60d0);
        let mut want = file.clone();
        want.events[6] = colour(0x0040_60d0);
        want.events[7] = custom(1);
        assert_eq!(to_flp(&project), Ok(want));

        // Its own colour again makes no change, and a file from 3.5.3
        // takes none.
        let mut project = from_flp(&to_flp(&project).unwrap());
        let before = project.clone();
        project.set_channel_color(0, 0x0040_60d0);
        assert_eq!(project, before);
        project.writer_version = Some("3.5.3".to_owned());
        project.set_channel_color(0, 0x0050_70d0);
        assert_eq!(project.channels[0].color, Some(0x0040_60d0));
    }

    fn enabled(project: &Project) -> Vec<Option<bool>> {
        project
            .channels
            .iter()
            .map(|channel| channel.enabled)
            .collect()
    }

    #[test]
    fn a_solo_switches_the_others_off_and_a_second_one_all_back_on() {
        let mut project = project();
        assert_eq!(enabled(&project), [Some(true), None, Some(false), None]);
        // Channel 2 is off, so the solo switches it on and the others off.
        project.solo_channel(2);
        assert_eq!(
            enabled(&project),
            [Some(false), Some(false), Some(true), Some(false)]
        );
        let written = to_flp(&project).unwrap();
        assert_eq!(enabled(&from_flp(&written)), enabled(&project));
        // A soloed channel that is soloed again puts them all back on.
        project.solo_channel(2);
        assert_eq!(enabled(&project), [Some(true); 4]);

        // A channel that is on and stays on keeps its event as it was, and
        // one that is not there solos nothing.
        let mut project = self::project();
        project.solo_channel(9);
        assert_eq!(to_flp(&project), Ok(file()));
        project.solo_channel(0);
        let written = to_flp(&project).unwrap();
        assert_eq!(written.events[..8], file().events[..8]);
        let read = from_flp(&written);
        let off = Some(false);
        assert_eq!(enabled(&read), [Some(true), off, off, off]);
    }

    #[test]
    fn a_clone_goes_behind_the_last_channel_with_every_event_of_its_own() {
        let mut project = project();
        assert_eq!(project.clone_channel(0), Some(4));
        assert_eq!(project.header.channel_count, 5);
        let written = to_flp(&project).unwrap();
        // The copy's events are behind the last channel's and in front of
        // the pattern, and every event of the file is as it was.
        let mut events = written.events.clone();
        let copy: Vec<RawEvent> = events.drain(15..21).collect();
        assert_eq!(events, file().events);
        let mut want = file().events[1..7].to_vec();
        want[0] = word(Id::ChannelNew, 4);
        want[4] = text(Id::PluginName, "Kick #2");
        assert_eq!(copy, want);

        // Read back, it is the channel with another index and name.
        let read = from_flp(&written);
        assert_eq!(read.header.channel_count, 5);
        let (kick, copy) = (&read.channels[0], &read.channels[4]);
        assert_eq!((copy.index, copy.name.as_deref()), (4, Some("Kick #2")));
        assert_eq!((copy.kind, copy.enabled), (kick.kind, kick.enabled));
        assert_eq!(copy.wrapper, kick.wrapper);
        assert_eq!(read.channels[..4], from_flp(&file()).channels[..]);

        // The next copy takes the next number, and so does a copy of the
        // copy. A channel without a name is named after its plugin, behind
        // its 212, and one without either keeps none.
        assert_eq!(project.clone_channel(0), Some(5));
        assert_eq!(project.clone_channel(4), Some(6));
        assert_eq!(project.clone_channel(2), Some(7));
        assert_eq!(project.clone_channel(1), Some(8));
        let read = from_flp(&to_flp(&project).unwrap());
        assert_eq!(
            names(&read)[4..],
            [
                Some("Kick #2"),
                Some("Kick #3"),
                Some("Kick #4"),
                Some("Fruity DX10 #2"),
                None
            ]
        );
        let indices: Vec<u16> = read.channels.iter().map(|c| c.index).collect();
        assert_eq!(indices, [0, 1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(read.channels[7].plugin_name.as_deref(), Some("Fruity DX10"));
        assert_eq!(read.channels[7].enabled, Some(false));
    }

    #[test]
    fn an_automation_clip_and_a_channel_that_is_not_there_are_not_cloned() {
        let mut project = project();
        project.channels[1].kind = Some(ChannelKind::Automation);
        assert_eq!(project.clone_channel(1), None);
        assert_eq!(project.clone_channel(9), None);
        assert_eq!(project.channels.len(), 4);
        // A project built in code has no place for the copy's events.
        let mut built = Project::new(file().header);
        built.channels.push(crate::Channel::default());
        assert_eq!(built.clone_channel(0), None);
    }

    #[test]
    fn an_empty_name_and_one_the_file_cannot_hold_are_not_set() {
        let mut project = project();
        for name in ["", "Kick", "a\0b"] {
            project.set_channel_name(0, name);
        }
        project.set_channel_name(9, "Nobody");
        assert_eq!(to_flp(&project), Ok(file()));

        // Strings are 8-bit before 11.5.5.
        project.writer_version = Some("10.0.9".to_owned());
        project.set_channel_name(0, "Kick ♪");
        assert_eq!(names(&project)[0], Some("Kick"));
        project.set_channel_name(0, "Kické");
        assert_eq!(names(&project)[0], Some("Kické"));
    }
}
