//! What a Patcher's state holds, as `docs/plugins/patcher.md` reads it: the
//! plugins inside it, each with its own name, state and ports, the ports
//! Patcher has towards FL, the wires between ports, and which inner
//! parameters Patcher's own parameters are.
//!
//! The state is an i32 version and then a run of chunks, each an i32 tag,
//! an i64 length and that many bytes. A tag 0 has a length but no bytes of
//! its own: it says how long the group after it is, and is read past. The
//! groups are flat: a plugin is the chunks from its `0x1388` to its
//! `0x1389`, a port towards FL those from `0x578` to `0x579`.

use crate::StateError;

/// Chunk tags.
mod tag {
    pub const GROUP: u32 = 0;
    pub const PLUGIN: u32 = 0x1388;
    pub const PLUGIN_END: u32 = 0x1389;
    pub const PLUGIN_NAME: u32 = 0x138c;
    pub const PLUGIN_STATE: u32 = 0x138d;
    pub const PLUGIN_SETTINGS: u32 = 0x138f;
    pub const PLUGIN_LABEL: u32 = 0x1392;
    pub const PORT: u32 = 0x1391;
    pub const FL_PORT: u32 = 0x578;
    pub const FL_PORT_NAME: u32 = 0x57a;
    pub const WIRE: u32 = 0x514;
    pub const EXPOSED: u32 = 0x44c;
}

/// What a port carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Carries {
    /// Stereo sound.
    Audio,
    /// Notes.
    Notes,
    /// A parameter's value, or a controller output's.
    Control,
}

/// A port of a plugin inside the Patcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Port {
    /// The number wires name it by. Every port has its own.
    pub id: i32,
    pub carries: Carries,
    /// For sound and notes, which input or output of the plugin: 0 its
    /// main one. For a control, the parameter, or the controller output
    /// when [`Port::output`].
    pub index: usize,
    /// Whether the port takes what comes in, rather than puts out.
    pub input: bool,
}

/// A plugin inside the Patcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// What the plugin is: 2 for an effect, 3 for a generator.
    pub kind: i32,
    /// The node's own number.
    pub id: i32,
    /// The fourth word of the node's header: bit 0 puts the plugin on the
    /// Patcher's surface, bit 2 keeps it off the worker threads and bit 3
    /// is the wrapper's flag at `+0xc49`. None of them touches the sound.
    pub flags: i32,
    /// Bit 0 of the third word of chunk `0x138f`: the module renders
    /// nothing (`+0x88`, `0x7fdcd0`).
    pub muted: bool,
    /// The lowest and the highest key the plugin takes notes of: the
    /// fourth and fifth word of `0x138f` (`+0x738`, `+0x73c`).
    pub keys: (i32, i32),
    /// The plugin's event 201 name.
    pub plugin: String,
    /// The name the patch shows on it.
    pub label: String,
    /// The plugin's own state, what its event 213 would be.
    pub state: Vec<u8>,
    pub ports: Vec<Port>,
}

/// A port of the Patcher itself, on its "From FL" or "To FL" side, as
/// `0x952560` reads chunk `0x578`: an i64 id, an i32 kind, an i32 of
/// flags, a byte and an i32 number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlPort {
    pub id: i32,
    /// Sound or notes.
    pub carries: Carries,
    /// Bit 0 of the flags: a port on the "From FL" side, which hands the
    /// graph what FL hands the Patcher. Without it the port is on the "To
    /// FL" side.
    pub from_fl: bool,
    /// For sound, which of FL's buffers: 0 the Patcher's own input or
    /// output, a sidechain input or a second output from 1 (`+0x78`,
    /// `+0x68` of the port). For notes, the MIDI port, -1 for FL's own.
    pub number: i32,
    pub name: String,
}

/// A wire from one port to another.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wire {
    pub to: i32,
    pub from: i32,
    /// What the wire multiplies what it carries by, for sound.
    pub gain: f32,
}

/// A parameter of the Patcher that is a port inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exposed {
    /// The port.
    pub port: i32,
    /// The Patcher's parameter.
    pub param: usize,
}

/// A Patcher's state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Patch {
    pub version: i32,
    pub nodes: Vec<Node>,
    pub fl_ports: Vec<FlPort>,
    pub wires: Vec<Wire>,
    pub exposed: Vec<Exposed>,
}

/// The highest version read.
pub const VERSION: i32 = 4;

fn short(wanted: usize, found: usize) -> StateError {
    StateError::Length {
        expected: wanted,
        found,
    }
}

/// The i32 at `at` in a chunk's bytes, 0 past its end.
fn int(data: &[u8], at: usize) -> i32 {
    data.get(at..at + 4)
        .map_or(0, |b| i32::from_le_bytes(b.try_into().unwrap()))
}

/// Text up to a 0: UTF-16 from version 4, one byte a letter before
/// (`0x956d50` sets the reader's wide flag for a version above 3).
fn text(data: &[u8], wide: bool) -> String {
    if !wide {
        let bytes = data.iter().take_while(|b| **b != 0);
        return bytes.map(|&b| char::from(b)).collect();
    }
    let units = data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes(*b));
    char::decode_utf16(units.take_while(|u| *u != 0))
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

/// The plugin a node's chunk `0x138c` names. Version 3 saves the DLL's
/// path under FL's directory, which `0x94e9b0` makes the plugin's name:
/// the file's name without `.dll`.
fn plugin_name(saved: String) -> String {
    let file = saved.rsplit(['\\', '/']).next().unwrap_or(&saved);
    let lower = file.to_ascii_lowercase();
    match lower.strip_suffix(".dll") {
        Some(stem) => file[..stem.len()].to_owned(),
        None => saved,
    }
}

impl Patch {
    /// Read a state. One that ends inside a chunk is turned down, and so
    /// is a version newer than [`VERSION`].
    pub fn read(bytes: &[u8]) -> Result<Patch, StateError> {
        let version = int(bytes, 0);
        if bytes.len() < 4 {
            return Err(short(4, bytes.len()));
        }
        if version > VERSION {
            return Err(StateError::Version { found: version });
        }
        let mut patch = Patch {
            version,
            ..Patch::default()
        };
        let wide = version > 3;
        let mut node: Option<Node> = None;
        let mut at = 4;
        while at < bytes.len() {
            let head = bytes.get(at..at + 12).ok_or(short(at + 12, bytes.len()))?;
            let tag = u32::from_le_bytes(head[..4].try_into().unwrap());
            let len = u64::from_le_bytes(head[4..].try_into().unwrap());
            at += 12;
            if tag == tag::GROUP {
                continue;
            }
            let end = usize::try_from(len)
                .ok()
                .and_then(|len| at.checked_add(len))
                .filter(|end| *end <= bytes.len())
                .ok_or(short(at.saturating_add(len as usize), bytes.len()))?;
            let data = &bytes[at..end];
            at = end;
            match tag {
                tag::PLUGIN => {
                    node = Some(Node {
                        kind: int(data, 0),
                        id: int(data, 4),
                        flags: int(data, 12),
                        muted: false,
                        keys: (0, i32::MAX),
                        plugin: String::new(),
                        label: String::new(),
                        state: Vec::new(),
                        ports: Vec::new(),
                    });
                }
                tag::PLUGIN_END => patch.nodes.extend(node.take()),
                tag::PLUGIN_NAME | tag::PLUGIN_LABEL | tag::PLUGIN_STATE | tag::PORT => {
                    let Some(node) = node.as_mut() else {
                        continue;
                    };
                    match tag {
                        tag::PLUGIN_NAME => node.plugin = plugin_name(text(data, wide)),
                        tag::PLUGIN_LABEL => node.label = text(data, wide),
                        tag::PLUGIN_STATE => node.state = data.to_vec(),
                        _ => {
                            let carries = match int(data, 0) {
                                0 => Carries::Audio,
                                1 => Carries::Notes,
                                _ => Carries::Control,
                            };
                            let index = int(data, 4) as u32;
                            let flags = int(data, 16);
                            let output = carries == Carries::Control && index & 0x1000_0000 != 0;
                            node.ports.push(Port {
                                id: int(data, 8),
                                carries,
                                index: (index & 0x0fff_ffff) as usize,
                                input: if carries == Carries::Control {
                                    !output
                                } else {
                                    flags & 1 != 0
                                },
                            });
                        }
                    }
                }
                tag::FL_PORT => patch.fl_ports.push(FlPort {
                    id: int(data, 0),
                    carries: if int(data, 8) == 1 {
                        Carries::Notes
                    } else {
                        Carries::Audio
                    },
                    from_fl: int(data, 12) & 1 != 0,
                    number: int(data, 17),
                    name: String::new(),
                }),
                tag::PLUGIN_SETTINGS => {
                    if let Some(node) = node.as_mut() {
                        node.muted = int(data, 8) & 1 != 0;
                        node.keys = (int(data, 12), int(data, 16));
                    }
                }
                tag::FL_PORT_NAME => {
                    if let Some(port) = patch.fl_ports.last_mut() {
                        port.name = text(data, wide);
                    }
                }
                tag::WIRE => patch.wires.push(Wire {
                    to: int(data, 0),
                    from: int(data, 8),
                    gain: f32::from_bits(int(data, 16) as u32),
                }),
                tag::EXPOSED => patch.exposed.push(Exposed {
                    port: int(data, 0),
                    param: int(data, 8).max(0) as usize,
                }),
                _ => {}
            }
        }
        Ok(patch)
    }
}
