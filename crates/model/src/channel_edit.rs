//! The channel rack's edits of whole channels: a channel's name, and its
//! solo.
//!
//! Like the edits in `pattern.rs`, each sets a field and sees to it that
//! the field has an event to be written in, see "Changing a project" in
//! the crate's docs. A channel index that names no channel is skipped.

use crate::codec::{Codec, Cx};
use crate::{Item, Placed, Project};
use flp::event::Id;

/// Whether the entry is the event `id`, typed or raw.
fn is(placed: &Placed, id: Id) -> bool {
    match &placed.item {
        Item::Field(field) => *field == id,
        Item::Raw(e) => e.id == id.0,
    }
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
