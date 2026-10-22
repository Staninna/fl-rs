//! Finding a channel's sample on disk and decoding it.

use crate::Error;
use model::{ChannelKind, Project};
use std::collections::BTreeMap;
use std::io::{Cursor, ErrorKind};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as Symphonia;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

/// A decoded sample. A mono file has no `right`. Of a file with more than
/// two channels the first two are kept.
#[derive(Debug)]
pub(crate) struct Sample {
    pub left: Vec<f32>,
    pub right: Option<Vec<f32>>,
    pub rate: u32,
}

/// What became of one sample path.
#[derive(Clone)]
pub(crate) enum Loaded {
    Ready(Arc<Sample>),
    /// No file at any place the path could mean.
    Missing,
    /// The file is there and it is a kind of WavPack that is not decoded.
    Unsupported,
}

/// Decoded samples by the path a channel's 196 stores. [`prepare`] builds
/// one. A clone shares the decoded audio.
#[derive(Clone, Default)]
pub struct PreparedSamples {
    loaded: BTreeMap<String, Loaded>,
    /// The channels whose sample file is there and did not decode, by
    /// [`Channel::index`](model::Channel::index), each with the
    /// [`Error::Decode`] it gave. Those channels stay silent.
    pub failed: Vec<(usize, Error)>,
    /// FL's `Plugins/Fruity/Generators` directory, for the generators that
    /// read their sound from a file there and the effects that find FL's
    /// other files from it.
    plugin_dir: Option<PathBuf>,
}

/// FL's `Plugins/Fruity/Generators` directory for a sample root that is
/// FL's `Data` directory: the two are side by side in an install. `None`
/// for any other sample root.
pub(crate) fn plugin_dir(sample_root: &Path) -> Option<PathBuf> {
    let data = sample_root.file_name()?.to_str()?;
    if !data.eq_ignore_ascii_case("Data") {
        return None;
    }
    Some(sample_root.parent()?.join("Plugins/Fruity/Generators"))
}

impl PreparedSamples {
    /// Say where FL keeps its generators, or with `None` that there is no
    /// such place. [`prepare`] works it out when the sample root is FL's
    /// `Data` directory, so this is for an install laid out another way.
    pub fn with_plugin_dir(mut self, plugin_dir: Option<PathBuf>) -> Self {
        self.plugin_dir = plugin_dir;
        self
    }

    pub(crate) fn plugin_dir(&self) -> Option<&Path> {
        self.plugin_dir.as_deref()
    }

    /// Look `path` up under `root` and decode it, once. A file that is
    /// there and fails to decode is the only error.
    pub(crate) fn load(&mut self, path: &str, root: &Path) -> Result<Option<Arc<Sample>>, Error> {
        if !self.loaded.contains_key(path) {
            let loaded = match resolve(path, root) {
                None => Loaded::Missing,
                Some(file) => match decode(&file) {
                    Ok(Some(sample)) => Loaded::Ready(Arc::new(sample)),
                    Ok(None) => Loaded::Unsupported,
                    Err(e) => {
                        return Err(Error::Decode {
                            path: file,
                            reason: e.to_string(),
                        });
                    }
                },
            };
            self.loaded.insert(path.to_owned(), loaded);
        }
        Ok(self.get(path))
    }

    /// The sample loaded for `path`. `None` for a path that was not loaded
    /// or has no audio behind it.
    pub(crate) fn get(&self, path: &str) -> Option<Arc<Sample>> {
        match self.loaded.get(path)? {
            Loaded::Ready(sample) => Some(Arc::clone(sample)),
            Loaded::Missing | Loaded::Unsupported => None,
        }
    }

    /// Take over what `more` has decoded. A path both have keeps the
    /// sample it has here.
    pub fn extend(&mut self, more: PreparedSamples) {
        for (path, loaded) in more.loaded {
            self.loaded.entry(path).or_insert(loaded);
        }
        self.failed.extend(more.failed);
        self.plugin_dir = self.plugin_dir.take().or(more.plugin_dir);
    }

    fn paths(&self, wanted: fn(&Loaded) -> bool) -> Vec<String> {
        let found = self.loaded.iter().filter(|(_, loaded)| wanted(loaded));
        found.map(|(path, _)| path.clone()).collect()
    }

    /// Sample paths with no file behind them.
    pub fn unresolved(&self) -> Vec<String> {
        self.paths(|loaded| matches!(loaded, Loaded::Missing))
    }

    /// Sample paths whose file is a kind of WavPack that is not decoded:
    /// hybrid, 8-bit, DSD or more than two channels.
    pub fn unsupported(&self) -> Vec<String> {
        self.paths(|loaded| matches!(loaded, Loaded::Unsupported))
    }
}

/// Decode the sample of every sampler and audio clip channel in the
/// project, whether the channel is played or not. `sample_root` is what
/// [`RenderOptions::sample_root`](crate::RenderOptions::sample_root) is.
///
/// A file that is there and does not decode is no error here: its channel
/// goes into [`PreparedSamples::failed`] and stays silent. Nothing this
/// returns is an `Err` today.
///
/// When `sample_root` is FL's `Data` directory, the result also knows FL's
/// `Plugins/Fruity/Generators` directory next to it, and a
/// [`Transport`](crate::Transport) or an [`Engine`](crate::Engine) made
/// with it tells its generators. [`PreparedSamples::with_plugin_dir`] names
/// another one.
///
/// This reads files, so it belongs on the caller's thread, before
/// [`Transport::open`](crate::Transport::open).
pub fn prepare(project: &Project, sample_root: &Path) -> Result<PreparedSamples, Error> {
    let mut samples = PreparedSamples::default().with_plugin_dir(plugin_dir(sample_root));
    let played = project.channels.iter().filter(|channel| {
        matches!(
            channel.kind,
            Some(ChannelKind::Sampler | ChannelKind::AudioClip)
        )
    });
    for channel in played {
        let Some(path) = channel.sample_path.as_deref().filter(|p| !p.is_empty()) else {
            continue;
        };
        if let Err(error) = samples.load(path, sample_root) {
            samples.failed.push((usize::from(channel.index), error));
        }
    }
    // The files a generator's state names, such as FPC's pads, or that it
    // finds in its own directory, such as FL Keys's wavetables.
    let plugin_dir = samples.plugin_dir().map(Path::to_path_buf);
    for channel in &project.channels {
        // A channel with no saved state plays the generator's defaults, as
        // `generator::hosted` makes it, and still takes the channel's sample.
        let Some(name) = channel.plugin_name.as_deref() else {
            continue;
        };
        let Some(mut generator) = plugins::generator_by_name(name) else {
            continue;
        };
        if let Some(dir) = &plugin_dir {
            generator.set_resource_dir(dir);
        }
        let restored = channel
            .plugin_state
            .as_deref()
            .is_none_or(|state| generator.set_state(state).is_ok());
        if restored {
            if let Some(path) = channel.sample_path.as_deref().filter(|p| !p.is_empty()) {
                generator.set_channel_sample(path);
            }
            for path in generator.sample_paths() {
                let _ = samples.load(&path, sample_root);
            }
        }
    }
    Ok(samples)
}

/// The file a sample path means, looked up under `root`.
///
/// FL writes the samples it ships as `%FLStudioFactoryData%\Data\Patches\..`
/// and those in the user's folder as `%FLStudioData%\Patches\..`. Either
/// placeholder is dropped and the rest is tried under `root`, with and
/// without a leading `Data`, so `root` can be FL's install directory or its
/// `Data` directory. A path with no placeholder is tried as it is when it is
/// absolute, and under `root` otherwise.
///
/// `%FLStudioFactoryData%` is FL's install directory, the one `Data` is in,
/// and FL ships files next to `Data` as well: one corpus project plays
/// `%FLStudioFactoryData%\Artwork\Skins\Default\RenderSnd.wv`. So when
/// `root` is a `Data` directory, a path with that placeholder is also tried
/// under the directory `root` is in.
///
/// Names match without regard to case, because the paths come from Windows:
/// some shipped projects say `data\patches\packs` for `Data/Patches/Packs`.
fn resolve(path: &str, root: &Path) -> Option<PathBuf> {
    let path = path.replace('\\', "/");
    let mut parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    let placeholder = parts
        .first()
        .copied()
        .filter(|part| part.len() > 2 && part.starts_with('%') && part.ends_with('%'));
    if placeholder.is_some() {
        parts.remove(0);
    } else if Path::new(&path).is_absolute() {
        return Path::new(&path).is_file().then(|| PathBuf::from(&path));
    }
    if let Some(file) = find(root, &parts) {
        return Some(file);
    }
    if let Some((first, rest)) = parts.split_first()
        && first.eq_ignore_ascii_case("data")
    {
        return find(root, rest);
    }
    let factory = placeholder.is_some_and(|name| name.eq_ignore_ascii_case(FACTORY));
    let data = root.file_name()?.to_str()?.eq_ignore_ascii_case("Data");
    if factory && data {
        return find(root.parent()?, &parts);
    }
    None
}

/// The placeholder FL writes for its install directory.
const FACTORY: &str = "%FLStudioFactoryData%";

/// Walk `parts` down from `root`. A part that does not exist as written is
/// looked up among its directory's entries without regard to case.
fn find(root: &Path, parts: &[&str]) -> Option<PathBuf> {
    let mut at = root.to_path_buf();
    for part in parts {
        if !matches!(
            Path::new(part).components().next(),
            Some(Component::Normal(_))
        ) {
            // `..`, `.` or a drive letter: nothing under `root`.
            return None;
        }
        let exact = at.join(part);
        at = if exact.exists() {
            exact
        } else {
            std::fs::read_dir(&at)
                .ok()?
                .filter_map(Result::ok)
                .find(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .eq_ignore_ascii_case(part)
                })?
                .path()
        };
    }
    at.is_file().then_some(at)
}

/// The Ogg stream inside a WAV file, when that is what its `data` chunk
/// holds. Most `.wav` files FL ships are Ogg Vorbis wrapped this way, under
/// format tag 0x674F or 0x6750, and symphonia's WAV reader turns both tags
/// down. Its Ogg reader takes the chunk's bytes as they are.
fn ogg_in_wav(bytes: &[u8]) -> Option<&[u8]> {
    if !(bytes.starts_with(b"RIFF") && bytes.get(8..12)? == b"WAVE") {
        return None;
    }
    let mut at = 12;
    loop {
        let id = bytes.get(at..at + 4)?;
        let size = u32::from_le_bytes(bytes.get(at + 4..at + 8)?.try_into().ok()?) as usize;
        let body = at + 8;
        if id == b"data" {
            let data = bytes.get(body..body.checked_add(size)?)?;
            return data.starts_with(b"OggS").then_some(data);
        }
        // Chunks are padded to an even size.
        at = body.checked_add(size)?.checked_add(size & 1)?;
    }
}

/// The pages of an Ogg file's first logical stream, or `None` when every
/// page belongs to it.
///
/// 357 of the 3,107 Ogg-in-WAV files FL ships carry a second stream, with
/// serial 0x10 or 0xFFFFFFFF, that starts in the middle of the audio. What
/// it holds is not known. Symphonia's reader gives up on the audio when it
/// meets that stream and returns the part before it, so the stream's pages
/// are taken out first.
fn first_ogg_stream(bytes: &[u8]) -> Option<Vec<u8>> {
    let serial = bytes.get(14..18)?;
    let mut kept = Vec::new();
    let mut dropped = false;
    let mut at = 0;
    while let Some(header) = bytes.get(at..at + 27).filter(|h| h.starts_with(b"OggS")) {
        let segments = usize::from(header[26]);
        let Some(lacing) = bytes.get(at + 27..at + 27 + segments) else {
            break;
        };
        let body: usize = lacing.iter().map(|size| usize::from(*size)).sum();
        let end = (at + 27 + segments + body).min(bytes.len());
        if &header[14..18] == serial {
            kept.extend_from_slice(&bytes[at..end]);
        } else {
            dropped = true;
        }
        at = end;
    }
    // Whatever follows the last whole page stays with the stream.
    kept.extend_from_slice(&bytes[at..]);
    dropped.then_some(kept)
}

/// The blocks of a WavPack file, without what follows the last of them.
/// One file FL ships ends in an APE tag, and wavicle takes whatever follows
/// a block for another block. A file cut short inside a block comes back
/// whole, for wavicle to turn down.
fn wavpack_blocks(bytes: &[u8]) -> &[u8] {
    let mut at = 0;
    while let Some(header) = bytes.get(at..at + 8).filter(|h| h.starts_with(b"wvpk")) {
        let size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        match at.checked_add(8).and_then(|body| body.checked_add(size)) {
            Some(end) if end <= bytes.len() => at = end,
            _ => return bytes,
        }
    }
    &bytes[..at]
}

/// Decode a WavPack file to f32. `Ok(None)` for the kinds wavicle turns
/// down by design: 8-bit, hybrid, DSD and more than two channels.
///
/// FL ships 1,954 WavPack files. 1,930 are lossless 16-bit and decode. 24
/// are hybrid, which is WavPack's lossy mode, and do not.
fn wavpack(bytes: &[u8]) -> Result<Option<Sample>, wavicle::Error> {
    let stream = match wavicle::decode_stream(wavpack_blocks(bytes)) {
        Ok(stream) => stream,
        Err(wavicle::Error::OutOfScope(_) | wavicle::Error::NotYetImplemented(_)) => {
            return Ok(None);
        }
        Err(e) => return Err(e),
    };
    let full_scale = (1u64 << (stream.bits_per_sample - 1)) as f32;
    let to_f32 = |sample: &i32| {
        if stream.is_float {
            f32::from_bits(sample.cast_unsigned())
        } else {
            *sample as f32 / full_scale
        }
    };
    let stereo = stream.channels > 1;
    let step = if stereo { 2 } else { 1 };
    let left = stream.samples.iter().step_by(step).map(to_f32).collect();
    let right = stream.samples.iter().skip(1).step_by(step).map(to_f32);
    Ok(Some(Sample {
        left,
        right: stereo.then(|| right.collect()),
        rate: stream.sample_rate,
    }))
}

/// Decode a whole file to f32. A file whose first four bytes say `wvpk` is
/// WavPack and goes to [`wavpack`], where `Ok(None)` comes from. Any other
/// file has to decode.
fn decode(path: &Path) -> Result<Option<Sample>, Box<dyn std::error::Error>> {
    let mut bytes = std::fs::read(path)?;
    if bytes.starts_with(b"wvpk") {
        return Ok(wavpack(&bytes)?);
    }
    if let Some(ogg) = ogg_in_wav(&bytes) {
        bytes = ogg.to_vec();
    }
    if bytes.starts_with(b"OggS")
        && let Some(stream) = first_ogg_stream(&bytes)
    {
        bytes = stream;
    }
    let stream = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(extension);
    }
    let mut format = symphonia::default::get_probe().probe(
        &hint,
        stream,
        FormatOptions::default(),
        MetadataOptions::default(),
    )?;
    let no_audio = Symphonia::Unsupported("no audio track");
    let Some(track) = format.default_track(TrackType::Audio) else {
        return Err(no_audio.into());
    };
    let track_id = track.id;
    let Some(params) = track.codec_params.as_ref().and_then(|p| p.audio()) else {
        return Err(no_audio.into());
    };
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())?;

    let mut sample = Sample {
        left: Vec::new(),
        right: None,
        rate: 0,
    };
    let mut frames = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            // Two shipped Ogg files stop after a whole page that is not
            // flagged as the last, and the reader runs into the end of the
            // file looking for another. Every packet is out by then.
            Err(Symphonia::IoError(e)) if e.kind() == ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        };
        if packet.track_id != track_id {
            continue;
        }
        let audio = decoder.decode(&packet)?;
        let channels = audio.spec().channels().count();
        if channels == 0 {
            continue;
        }
        sample.rate = audio.spec().rate();
        audio.copy_to_vec_interleaved(&mut frames);
        sample.left.extend(frames.iter().step_by(channels));
        if channels > 1 {
            let right = sample.right.get_or_insert_with(Vec::new);
            right.extend(frames.iter().skip(1).step_by(channels));
        }
    }
    Ok(Some(sample))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fl_s_generators_are_next_to_its_data_directory() {
        let dir = |root: &str| plugin_dir(Path::new(root));
        let generators = PathBuf::from("/fl/Plugins/Fruity/Generators");
        assert_eq!(dir("/fl/Data"), Some(generators.clone()));
        assert_eq!(dir("/fl/data/"), Some(generators.clone()));
        assert_eq!(dir("/fl/Data/Patches"), None);
        assert_eq!(dir("/fl/Samples"), None);
        assert_eq!(
            dir("Data"),
            Some(PathBuf::from("Plugins/Fruity/Generators"))
        );
        assert_eq!(dir("/"), None);

        // prepare finds it, a caller can name another, and samples that
        // are added keep the one that is there.
        let project = Project::new(flp::Header {
            format: 0,
            channel_count: 0,
            ppq: 96,
        });
        let found = prepare(&project, Path::new("/fl/Data")).unwrap();
        assert_eq!(found.plugin_dir(), Some(generators.as_path()));
        let none = prepare(&project, Path::new("/songs")).unwrap();
        assert_eq!(none.plugin_dir(), None);
        let named = none.clone().with_plugin_dir(Some("/other".into()));
        assert_eq!(named.plugin_dir(), Some(Path::new("/other")));
        let mut both = none;
        both.extend(found.clone());
        assert_eq!(both.plugin_dir(), Some(generators.as_path()));
        let mut kept = named;
        kept.extend(found);
        assert_eq!(kept.plugin_dir(), Some(Path::new("/other")));
    }

    #[test]
    fn a_generator_is_told_its_directory_before_it_names_its_files() {
        // FL Keys's Grand Piano finds its 30 files in FL Keys/Wavtables,
        // so it can only name them once it knows where that is.
        let Some(root) = std::env::var_os("FL_RS_CORPUS").map(PathBuf::from) else {
            eprintln!("FL_RS_CORPUS unset, FL Keys's files skipped");
            return;
        };
        let bytes = std::fs::read(root.join("System/Render tests/FL Keys.flp")).unwrap();
        let project = model::from_flp(&flp::Project::parse(&bytes).unwrap());
        let samples = prepare(&project, &root).unwrap();
        let piano = samples.loaded.keys().filter(|p| p.contains("Grand Piano"));
        let decoded = piano.filter(|p| samples.get(p).is_some()).count();
        assert_eq!(decoded, 30);
    }

    #[test]
    fn a_generator_channel_with_no_state_still_takes_its_sample() {
        // FL plays such a channel at the generator's defaults, so the
        // channel's sample has to be looked for all the same.
        let mut project = model::Project::new(flp::Header {
            format: 0,
            channel_count: 1,
            ppq: 96,
        });
        project.channels.push(model::Channel {
            kind: Some(model::ChannelKind::Plugin),
            plugin_name: Some(plugins::granulizer::NAME.into()),
            sample_path: Some("Grains.wav".into()),
            ..model::Channel::default()
        });
        let samples = prepare(&project, Path::new("nowhere")).unwrap();
        assert!(samples.loaded.contains_key("Grains.wav"));
    }

    /// A directory under the system's temp directory, removed on drop.
    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("fl-rs-engine-sample-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Dir(dir)
        }

        fn file(&self, path: &str) -> PathBuf {
            let file = self.0.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, b"x").unwrap();
            file
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// An Ogg page with one segment: `serial`, then `body`.
    fn page(serial: u32, body: &[u8]) -> Vec<u8> {
        let mut page = b"OggS".to_vec();
        page.extend_from_slice(&[0; 10]);
        page.extend_from_slice(&serial.to_le_bytes());
        page.extend_from_slice(&[0; 8]);
        page.extend_from_slice(&[1, body.len() as u8]);
        page.extend_from_slice(body);
        page
    }

    fn wav(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut bytes = b"RIFF\0\0\0\0WAVE".to_vec();
        for (id, body) in chunks {
            bytes.extend_from_slice(*id);
            bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
            bytes.extend_from_slice(body);
            bytes.resize(bytes.len() + body.len() % 2, 0);
        }
        bytes
    }

    #[test]
    fn a_wavpack_stream_decodes() {
        let params = |channels, bits_per_sample| wavicle::EncodeParams {
            channels,
            sample_rate: 44_100,
            bits_per_sample,
        };
        let stereo = [0, 16_384, -32_768, 32_767, 8_192, -8_192];
        let file = wavicle::encode_int(params(2, 16), &stereo).unwrap();
        let sample = wavpack(&file).unwrap().unwrap();
        assert_eq!(sample.rate, 44_100);
        assert_eq!(sample.left, [0.0, -1.0, 0.25]);
        assert_eq!(sample.right, Some(vec![0.5, 32_767.0 / 32_768.0, -0.25]));

        let mono = wavicle::encode_int(params(1, 24), &[4_194_304, -8_388_608]).unwrap();
        let sample = wavpack(&mono).unwrap().unwrap();
        assert_eq!(sample.left, [0.5, -1.0]);
        assert!(sample.right.is_none());

        let float = wavicle::encode_float(1, 48_000, &[0.125, -1.5]).unwrap();
        let sample = wavpack(&float).unwrap().unwrap();
        assert_eq!((sample.left, sample.rate), (vec![0.125, -1.5], 48_000));

        // A tag after the last block is left out.
        let tagged = [&file[..], b"APETAGEX and so on"].concat();
        assert_eq!(wavpack(&tagged).unwrap().unwrap().left, [0.0, -1.0, 0.25]);

        // 8-bit is a kind wavicle does not decode, and so is hybrid, which
        // bit 3 of the block's flags says. A cut file is an error.
        let eight = wavicle::encode_int(params(1, 8), &[1, -1]).unwrap();
        assert!(wavpack(&eight).unwrap().is_none());
        let mut hybrid = file.clone();
        hybrid[24] |= 8;
        assert!(wavpack(&hybrid).unwrap().is_none());
        assert!(wavpack(&file[..file.len() - 4]).is_err());
    }

    #[test]
    fn a_wavpack_file_loads_by_its_path() {
        let dir = Dir::new("wavpack");
        let params = wavicle::EncodeParams {
            channels: 1,
            sample_rate: 22_050,
            bits_per_sample: 16,
        };
        let file = wavicle::encode_int(params, &[16_384, -16_384]).unwrap();
        std::fs::write(dir.file("Patches/Pad.wv"), file).unwrap();
        let mut samples = PreparedSamples::default();
        let pad = samples.load("Patches/Pad.wv", &dir.0).unwrap().unwrap();
        assert_eq!((&pad.left[..], pad.rate), (&[0.5, -0.5][..], 22_050));
        assert!(samples.unsupported().is_empty());
    }

    #[test]
    fn an_ogg_stream_in_a_wav_file_is_found() {
        let ogg = page(7, b"abc");
        let file = wav(&[
            (b"fmt ", &[0x4f, 0x67, 1]),
            (b"fact", &[0; 4]),
            (b"data", &ogg),
        ]);
        assert_eq!(ogg_in_wav(&file), Some(&ogg[..]));
        let pcm = wav(&[(b"fmt ", &[1, 0]), (b"data", &[0; 64])]);
        assert_eq!(ogg_in_wav(&pcm), None);
        // A `data` chunk that says it is longer than the file. The last
        // byte of the file is the chunk's padding.
        assert_eq!(ogg_in_wav(&file[..file.len() - 2]), None);
        assert_eq!(ogg_in_wav(&ogg), None);
        assert_eq!(ogg_in_wav(b"RIFF"), None);
    }

    #[test]
    fn pages_of_a_second_ogg_stream_are_taken_out() {
        let (a, b, c) = (page(0, b"one"), page(0x10, b"other"), page(0, b"two"));
        let mixed = [a.clone(), b.clone(), c.clone(), b].concat();
        assert_eq!(
            first_ogg_stream(&mixed),
            Some([a.clone(), c.clone()].concat())
        );
        assert_eq!(first_ogg_stream(&[a.clone(), c.clone()].concat()), None);
        // A page cut short by the end of the file is kept as far as it goes.
        let cut = [a.clone(), page(0x10, b"x"), c[..c.len() - 1].to_vec()].concat();
        let kept = [a, c[..c.len() - 1].to_vec()].concat();
        assert_eq!(first_ogg_stream(&cut), Some(kept));
        assert_eq!(first_ogg_stream(b"OggS"), None);
    }

    #[test]
    fn a_factory_path_resolves_under_the_data_directory() {
        let dir = Dir::new("factory");
        let kick = dir.file("Patches/Packs/Drums/Kick.wav");
        for path in [
            r"%FLStudioFactoryData%\Data\Patches\Packs\Drums\Kick.wav",
            r"%FLStudioData%\Patches\Packs\Drums\Kick.wav",
            "Patches/Packs/Drums/Kick.wav",
        ] {
            assert_eq!(resolve(path, &dir.0), Some(kick.clone()), "{path}");
        }
        // On a file system that ignores case the path comes back as asked.
        let lower = r"%FLStudioFactoryData%\data\patches\packs\drums\kick.wav";
        assert!(resolve(lower, &dir.0).is_some_and(|file| file.is_file()));
        let absolute = kick.to_str().unwrap();
        assert_eq!(resolve(absolute, Path::new("nowhere")), Some(kick.clone()));
    }

    #[test]
    fn a_factory_path_resolves_under_the_install_directory() {
        let dir = Dir::new("install");
        let kick = dir.file("Data/Patches/Kick.wav");
        let path = r"%FLStudioFactoryData%\Data\Patches\Kick.wav";
        assert_eq!(resolve(path, &dir.0), Some(kick));
    }

    #[test]
    fn a_factory_path_outside_data_resolves_next_to_the_data_directory() {
        let dir = Dir::new("beside");
        let sound = dir.file("Artwork/Skins/Default/RenderSnd.wv");
        dir.file("Data/Patches/Kick.wav");
        let path = r"%FLStudioFactoryData%\Artwork\Skins\Default\RenderSnd.wv";
        // From the install directory, and from the `Data` directory in it.
        assert_eq!(resolve(path, &dir.0), Some(sound.clone()));
        assert_eq!(resolve(path, &dir.0.join("Data")), Some(sound.clone()));
        assert_eq!(resolve(path, &dir.0.join("data")), Some(sound));
        // The user's folder has no directory above it to look in, and nor
        // has a path without a placeholder or a root of another name.
        for (path, root) in [
            (r"%FLStudioData%\Artwork\Skins\Default\RenderSnd.wv", "Data"),
            (r"Artwork\Skins\Default\RenderSnd.wv", "Data"),
            (
                r"%FLStudioFactoryData%\Skins\Default\RenderSnd.wv",
                "Artwork/Skins",
            ),
        ] {
            assert_eq!(resolve(path, &dir.0.join(root)), None, "{path}");
        }
    }

    #[test]
    fn what_is_not_there_does_not_resolve() {
        let dir = Dir::new("missing");
        dir.file("Patches/Kick.wav");
        for path in [
            r"%FLStudioFactoryData%\Data\Patches\Snare.wav",
            r"%FLStudioFactoryData%\Data\Patches",
            r"C:\Samples\Kick.wav",
            r"Patches\..\Patches\Kick.wav",
            "",
        ] {
            assert_eq!(resolve(path, &dir.0), None, "{path}");
        }
    }
}
