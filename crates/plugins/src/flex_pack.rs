//! FLEX's pack files: an index of named entries, the presets in them
//! masked with a 16-byte XOR key, and the FLAC samples they play, as
//! `docs/plugins/flex.md` has them.
//!
//! A pack is read where FL installs it, `Data/Patches/Packs/FLEX/Packs/`;
//! nothing of it is in this repository.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// The first word of a pack the v1 engine plays (`.flexpack`), big-endian.
/// A `.flex2pack` for the v2 engine has `0x0100_0002`.
pub(crate) const V1: u32 = 0x0000_0002;

/// What a v1 preset starts with once unmasked: `xelf00su`.
pub(crate) const MAGIC: &[u8; 8] = b"xelf00su";

/// One entry of a pack's index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) name: String,
    pub(crate) offset: u64,
    pub(crate) size: u64,
}

/// A pack's index, with the path of its file to read the entries from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pack {
    pub(crate) path: PathBuf,
    pub(crate) kind: u32,
    pub(crate) entries: Vec<Entry>,
}

fn be32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn be64(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

/// The index at the start of a pack: a word of kind, a count, and for each
/// entry a length (with the trailing NUL), the name, the offset and the
/// size, all big-endian.
pub(crate) fn parse_index(head: &[u8]) -> Option<(u32, Vec<Entry>)> {
    let kind = be32(head, 0)?;
    let count = be32(head, 4)? as usize;
    let mut at = 8;
    let mut entries = Vec::with_capacity(count.min(1 << 16));
    for _ in 0..count {
        let len = be32(head, at)? as usize;
        at += 4;
        let raw = head.get(at..at + len)?;
        at += len;
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        let name = String::from_utf8_lossy(&raw[..end]).into_owned();
        let offset = be64(head, at)?;
        let size = be64(head, at + 8)?;
        at += 16;
        entries.push(Entry { name, offset, size });
    }
    Some((kind, entries))
}

impl Pack {
    /// Read the index of the pack at `path`.
    pub(crate) fn open(path: &Path) -> Option<Pack> {
        let mut file = File::open(path).ok()?;
        let mut head = [0; 8];
        file.read_exact(&mut head).ok()?;
        let count = be32(&head, 4)? as u64;
        // An entry takes 20 bytes beside its name, and names are short.
        let mut index = vec![0; (8 + count * 2048).min(file.metadata().ok()?.len()) as usize];
        file.seek(SeekFrom::Start(0)).ok()?;
        file.read_exact(&mut index).ok()?;
        let (kind, entries) = parse_index(&index)?;
        Some(Pack {
            path: path.to_path_buf(),
            kind,
            entries,
        })
    }

    /// The bytes of the entry by this name, unmasked.
    pub(crate) fn read(&self, name: &str) -> Option<Vec<u8>> {
        let index = self.entries.iter().position(|entry| entry.name == name)?;
        let entry = &self.entries[index];
        let mut file = File::open(&self.path).ok()?;
        file.seek(SeekFrom::Start(entry.offset)).ok()?;
        let mut bytes = vec![0; usize::try_from(entry.size).ok()?];
        file.read_exact(&mut bytes).ok()?;
        unmask(&mut bytes, index);
        Some(bytes)
    }
}

/// The MD5 digest of `this_pack_is_free`, the string a free pack's
/// master key is hashed from (`FUN_180389b60`; a bought pack's is hashed
/// from its product name, and is not known here).
const FREE: [u8; 16] = [
    0x5d, 0x67, 0x1e, 0x7d, 0x34, 0x58, 0x8d, 0x11, 0x12, 0xf3, 0x8a, 0x20, 0xe6, 0xc0, 0x0a, 0x67,
];

/// The master key the engine stirs out of a 16-byte digest
/// (`FUN_180389b60`, `engine_x64.dll`).
pub(crate) fn master(h: &[u8; 16]) -> [u8; 16] {
    let b = |i: usize| h[i];
    let stir = [
        (b(9).wrapping_add(b(1)).wrapping_sub(b(5)))
            .wrapping_mul(0x0c)
            .wrapping_add(0x74),
        (b(1).wrapping_add(b(11)).wrapping_add(b(15)))
            .wrapping_mul(0x45)
            .wrapping_add(1),
        (b(4).wrapping_add(b(1)).wrapping_sub(b(5)))
            .wrapping_mul(0x7b)
            .wrapping_add(0x49),
        (b(2).wrapping_add(b(9)).wrapping_add(b(15)))
            .wrapping_mul(8)
            .wrapping_add(0x20),
        b(5).wrapping_mul(2).wrapping_add(0x4c),
        (b(8).wrapping_add(b(12)).wrapping_add(b(5)))
            .wrapping_mul(0x4d)
            .wrapping_sub(0x1d),
        (b(11).wrapping_add(b(11)).wrapping_sub(b(15)))
            .wrapping_mul(0x0d)
            .wrapping_add(0x14),
        (b(2).wrapping_add(b(3)).wrapping_sub(b(15)))
            .wrapping_mul(0x63)
            .wrapping_add(0x39),
        b(8).wrapping_mul(3).wrapping_add(100),
        b(1).wrapping_add(b(1))
            .wrapping_sub(b(15))
            .wrapping_sub(0x0d),
        (b(2).wrapping_add(b(1)).wrapping_add(b(5)))
            .wrapping_mul(5)
            .wrapping_add(0xbe),
        (b(5).wrapping_add(b(7)).wrapping_sub(b(15)))
            .wrapping_mul(7)
            .wrapping_sub(0x34),
        b(2).wrapping_add(b(8))
            .wrapping_sub(b(5))
            .wrapping_add(0x11),
        (b(1).wrapping_add(b(10)).wrapping_add(b(15)))
            .wrapping_mul(2)
            .wrapping_add(0x77),
        b(10).wrapping_mul(0x0a).wrapping_add(0x0b),
        (b(9).wrapping_add(b(11)).wrapping_sub(b(15)))
            .wrapping_mul(0x0f)
            .wrapping_add(0x98),
    ];
    let mut out = [0; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = h[i] ^ stir[i];
    }
    out
}

/// The 16-byte key of the entry at `index` in its pack's index, from the
/// master key (`FUN_1803de7a0`): every entry, presets, samples and the
/// `pcf.pcf` check alike, is XORed with it over and over.
pub(crate) fn entry_key(m: &[u8; 16], index: usize) -> [u8; 16] {
    let mut b =
        (((index as u8).wrapping_mul(3) & 0x1f).wrapping_mul(0xe2) & 0x3f).wrapping_mul(0x1f);
    b ^= match b & 0x0f {
        0 => 0x12,
        2 => 0x29,
        4 | 12 => 0x11,
        6 => 0xa9,
        8 => 0x02,
        10 => 0xbe,
        14 => 0x0b,
        _ => 0,
    };
    let w = |x: u8, k: u8| x.wrapping_mul(k);
    [
        w(w(m[9], b), 0x74) ^ m[0],
        b.wrapping_add(m[0]) ^ m[1],
        w(m[10], 0x49).wrapping_add(b) ^ m[2],
        w(w(m[1], b), 0x20) ^ m[3],
        w(m[11], 0x4c).wrapping_add(b) ^ m[4],
        w(w(m[9], b), 0xe3) ^ m[5],
        w(m[15], 0x14).wrapping_add(b) ^ m[6],
        b.wrapping_sub(w(m[7], 0x39)) ^ m[7],
        w(m[3], 0x64).wrapping_add(b) ^ m[8],
        w(w(m[6], b), 0xf3) ^ m[9],
        w(m[8], 0x42).wrapping_add(b) ^ m[10],
        b.wrapping_sub(w(m[4], 0x34)) ^ m[11],
        w(w(b, m[12]), 0x11) ^ m[12],
        w(m[2], 0x77).wrapping_add(b) ^ m[13],
        b.wrapping_sub(w(m[9], 0x0b)) ^ m[14],
        b.wrapping_sub(w(m[1], 0x68)) ^ m[15],
    ]
}

/// Unmask the entry at `index` of a free pack in place.
pub(crate) fn unmask(bytes: &mut [u8], index: usize) {
    let key = entry_key(&master(&FREE), index);
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte ^= key[i % 16];
    }
}

/// The directory FL installs FLEX's packs in, from FL's `Data` directory.
pub(crate) fn packs_dir(data: &Path) -> PathBuf {
    data.join("Patches/Packs/FLEX/Packs")
}

/// A decoded sample: its sides and its rate.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Decoded {
    pub(crate) left: Vec<f32>,
    pub(crate) right: Option<Vec<f32>>,
    pub(crate) rate: u32,
}

/// Decode a FLAC file held in memory.
pub(crate) fn decode_flac(bytes: Vec<u8>) -> Option<Decoded> {
    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::{FormatOptions, TrackType};
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;

    let stream = MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes)), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("flac");
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .ok()?;
    let track = format.default_track(TrackType::Audio)?;
    let track_id = track.id;
    let params = track.codec_params.as_ref()?.audio()?.clone();
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .ok()?;
    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut stereo = false;
    let mut rate = 0;
    let mut frames = Vec::new();
    while let Ok(Some(packet)) = format.next_packet() {
        if packet.track_id != track_id {
            continue;
        }
        let Ok(audio) = decoder.decode(&packet) else {
            continue;
        };
        let channels = audio.spec().channels().count();
        if channels == 0 {
            continue;
        }
        rate = audio.spec().rate();
        audio.copy_to_vec_interleaved(&mut frames);
        stereo = channels > 1;
        for frame in frames.chunks_exact(channels) {
            let frame: &[f32] = frame;
            left.push(frame[0]);
            if stereo {
                right.push(frame[1]);
            }
        }
    }
    (rate > 0 && !left.is_empty()).then(|| Decoded {
        left,
        right: stereo.then_some(right),
        rate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_index_is_read_as_the_packs_write_it() {
        let mut head = Vec::new();
        head.extend_from_slice(&V1.to_be_bytes());
        head.extend_from_slice(&2u32.to_be_bytes());
        for (name, offset, size) in [("a.flac\0", 40u64, 3u64), ("bc.flexpreset\0", 43, 5)] {
            head.extend_from_slice(&(name.len() as u32).to_be_bytes());
            head.extend_from_slice(name.as_bytes());
            head.extend_from_slice(&offset.to_be_bytes());
            head.extend_from_slice(&size.to_be_bytes());
        }
        let (kind, entries) = parse_index(&head).unwrap();
        assert_eq!(kind, V1);
        assert_eq!(entries[0].name, "a.flac");
        assert_eq!(entries[1].name, "bc.flexpreset");
        assert_eq!((entries[1].offset, entries[1].size), (43, 5));
        assert!(parse_index(&head[..head.len() - 1]).is_none());
    }

    #[test]
    fn the_keys_come_out_of_the_free_master_as_the_engine_stirs_them() {
        let m = master(&FREE);
        assert_eq!(
            m,
            [
                0xd1, 0x40, 0x64, 0x9d, 0xc8, 0xab, 0x94, 0x44, 0x88, 0xa9, 0x85, 0xfa, 0x0f, 0xe7,
                0x65, 0xcb
            ]
        );
        // The key of entry 1, `pcf.pcf`, and of entry 0, as the packs have
        // them.
        let one = [
            0x01, 0xb5, 0x75, 0x9d, 0x94, 0x67, 0x94, 0x44, 0xf0, 0xd9, 0xb1, 0x7e, 0xd3, 0x47,
            0x84, 0xef,
        ];
        assert_eq!(entry_key(&m, 1), one);
        assert_eq!(entry_key(&m, 33), one);
        assert_eq!(
            entry_key(&m, 0),
            [
                0xb9, 0xa3, 0x9b, 0x9d, 0x82, 0xcd, 0x7a, 0xaa, 0xee, 0x11, 0xa7, 0x88, 0xe1, 0x69,
                0xaa, 0xd9
            ]
        );
        let mut check = b"if_you_can_read_this".to_vec();
        for (i, byte) in check.iter_mut().enumerate() {
            *byte ^= one[i % 16];
        }
        unmask(&mut check, 1);
        assert_eq!(check, b"if_you_can_read_this");
    }
}
