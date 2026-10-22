//! Render the first 10 seconds of every project in the FL Studio corpus.
//!
//! The corpus is FL Studio's own `Data/` directory, which stays out of this
//! repo. Point `FL_RS_CORPUS` at it. Without the variable the test passes
//! and says it was skipped. Run with `--nocapture` for the line per project.
//!
//! A sample that is not on disk, or that is hybrid WavPack, is counted. A sample
//! that is there and fails to decode fails the test, and so does a mixer
//! whose routes loop.
//!
//! Every project is rendered twice, with the [`Approximate`] curves and with
//! [`FlExact`], and the test ends with how the master's peaks are spread
//! for each.
//!
//! The automation line counts the clips that start in those 10 seconds
//! and drive a control the engine plays, the links from a clip to a
//! control in the whole of every project, the links from the clips in the
//! 10 seconds to a control of an effect slot the engine plays, and what
//! those clips are bound to that the engine does not play.
//!
//! The controllers line counts the 227 links whose source is an output of
//! a controller plugin the engine hosts, in the whole of every project:
//! how many name each of Fruity Peak Controller's three outputs, how many
//! the engine plays, and how many drive a control it does not play or
//! have a formula it does not read.
//!
//! The effects line counts the effect slots on the inserts that sound
//! reaches in those 10 seconds: the ones whose plugin the engine hosts and
//! the ones it passes through, next to every slot with a plugin in the
//! corpus. It ends with the five plugins that most of the passed slots
//! hold.
//!
//! The generators line counts the plugin channels that are switched on
//! and have a note in those 10 seconds: the ones whose generator the engine
//! hosts and the ones that stay silent, next to every plugin channel in
//! the corpus. It ends with the five plugins that most of the silent
//! channels hold, which are the next ones to build. The hosted generators
//! are told FL's `Plugins/Fruity/Generators` directory, which the engine
//! finds next to the corpus.
//!
//! The approximated generators line counts the hosted generators that
//! stand in for a thing their state asks for or lack a file of FL's, by
//! what the plugin says of it.
//!
//! The approximated line counts the hosted slots whose plugin stands in
//! for a thing its state asks for, by what the plugin says of it.
//!
//! The sidechain line counts the sidechain inputs the hosted plugins
//! listen to, each an insert and an insert that sends to it, and the slots
//! whose plugin is handed one.
//!
//! The latency line adds up, over the projects with sound, how many frames
//! late plugin delay compensation makes each one's output, and counts the
//! inserts it holds back.

use engine::{Approximate, Curves, FlExact, RenderOptions, Rendered};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn find(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            find(&path, out);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("flp"))
        {
            out.push(path);
        }
    }
}

/// The largest sample of a render, after a check that every sample is a
/// number.
fn peak(out: &Rendered, name: &str) -> f32 {
    assert_eq!(out.left.len(), out.right.len(), "{name}");
    assert!(out.left.len() <= 480_000, "{name}: {}", out.left.len());
    let finite = out.left.iter().chain(&out.right).all(|v| v.is_finite());
    assert!(finite, "{name}: NaN or infinity in the output");
    let samples = out.left.iter().chain(&out.right);
    samples.fold(0f32, |m, v| m.max(v.abs()))
}

/// How many projects peak where.
#[derive(Default)]
struct Peaks {
    /// Silent, up to 0.25, 0.5, 1, 2 and 4, and above 4.
    buckets: [usize; 7],
    max: f32,
}

impl Peaks {
    fn add(&mut self, peak: f32) {
        let edges = [0.0, 0.25, 0.5, 1.0, 2.0, 4.0];
        let bucket = edges.iter().position(|edge| peak <= *edge);
        self.buckets[bucket.unwrap_or(edges.len())] += 1;
        self.max = self.max.max(peak);
    }

    fn line(&self, curves: &str) -> String {
        let [silent, b25, b50, b100, b200, b400, more] = self.buckets;
        format!(
            "master peaks, {curves}: silent {silent}, to 0.25 {b25}, to 0.5 {b50}, to 1.0 {b100}, \
             to 2.0 {b200}, to 4.0 {b400}, above {more}; {} projects peak above 1.0, highest {:.3}",
            b200 + b400 + more,
            self.max,
        )
    }
}

/// The five plugins with the highest counts, each with its count.
fn most(counts: &BTreeMap<String, usize>) -> String {
    let mut most: Vec<_> = counts.iter().collect();
    most.sort_by_key(|(plugin, count)| (std::cmp::Reverse(**count), *plugin));
    let most = most.iter().take(5).map(|(p, n)| format!("{p} {n}"));
    most.collect::<Vec<_>>().join(", ")
}

#[test]
fn every_project_renders_its_first_10_seconds() {
    let Some(root) = std::env::var_os("FL_RS_CORPUS").map(PathBuf::from) else {
        eprintln!("FL_RS_CORPUS unset, corpus render skipped");
        return;
    };
    assert!(
        root.is_dir(),
        "FL_RS_CORPUS={} is not a directory",
        root.display()
    );
    let mut files = Vec::new();
    find(&root, &mut files);
    files.sort();
    assert!(!files.is_empty(), "no .flp files under {}", root.display());

    let opts = RenderOptions {
        sample_rate: 48_000,
        sample_root: root.clone(),
        max_seconds: 10.0,
        plugin_dir: None,
    };
    let (mut notes, mut voices, mut audible) = (0, 0, 0);
    let (mut clips, mut stretched) = (0, 0);
    let (mut automation, mut bindings, mut playable, mut unsupported_targets) = (0, 0, 0, 0);
    let mut slot_targets = 0;
    let (mut links, mut link_targets, mut unsupported_links) = (0, 0, 0);
    // The links by the output of the Peak Controller they come from:
    // Peak + LFO, Peak and LFO.
    let mut outputs = [0; 3];
    let (mut unresolved, mut unsupported) = (BTreeSet::new(), BTreeSet::new());
    let (mut hosted, mut slots) = (0, 0);
    let mut unhosted: BTreeMap<String, usize> = BTreeMap::new();
    let (mut generators, mut plugin_channels) = (0, 0);
    let mut silent: BTreeMap<String, usize> = BTreeMap::new();
    let mut approximated: BTreeMap<&str, usize> = BTreeMap::new();
    let mut approximated_generators: BTreeMap<&str, usize> = BTreeMap::new();
    let (mut sidechain_routes, mut sidechain_slots) = (0, 0);
    let (mut latency, mut late, mut delayed) = (0, 0, 0);
    let (mut approximate, mut exact) = (Peaks::default(), Peaks::default());
    for path in &files {
        let name = path.strip_prefix(&root).unwrap().display().to_string();
        let file = flp::Project::parse(&std::fs::read(path).unwrap())
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let project = model::from_flp(&file);
        let render = |curves: &dyn Curves| {
            engine::render_with(&project, &opts, curves).unwrap_or_else(|e| panic!("{name}: {e}"))
        };
        let out = render(&Approximate);
        let peak = peak(&out, &name);
        let fl_peak = self::peak(&render(&FlExact), &name);
        approximate.add(peak);
        exact.add(fl_peak);
        eprintln!(
            "{name}: notes {} voices {} clips {} stretched {} unresolved {} unsupported {} \
             skipped {} automation {} unsupported targets {} hosted slots {} unhosted {} \
             generators {} unhosted {} approximated generators {} approximated {} latency {} \
             delayed inserts {} peak {peak:.3} with FL's curves {fl_peak:.3}",
            out.notes,
            out.voices,
            out.clips,
            out.unsupported_stretch,
            out.unresolved.len(),
            out.unsupported.len(),
            out.skipped,
            out.automation_clips,
            out.unsupported_targets,
            out.hosted_slots,
            out.unhosted_slots.values().sum::<usize>(),
            out.hosted_generators,
            out.unhosted_generators.values().sum::<usize>(),
            out.approximated_generators.values().sum::<usize>(),
            out.approximated_slots.values().sum::<usize>(),
            out.latency,
            out.delayed_inserts,
        );
        // What keeps the project from playing in full, by name.
        let gaps = [
            ("passed", format!("{:?}", out.unhosted_slots)),
            ("silent", format!("{:?}", out.unhosted_generators)),
            ("approximated", format!("{:?}", out.approximated_slots)),
            (
                "approximated generators",
                format!("{:?}", out.approximated_generators),
            ),
        ];
        for (what, list) in gaps.iter().filter(|(_, list)| list != "{}") {
            eprintln!("  {what}: {list}");
        }
        hosted += out.hosted_slots;
        sidechain_routes += out.sidechain_routes;
        sidechain_slots += out.sidechain_slots;
        for (what, count) in out.approximated_slots {
            *approximated.entry(what).or_default() += count;
        }
        for (what, count) in out.approximated_generators {
            *approximated_generators.entry(what).or_default() += count;
        }
        if peak > 0.0 {
            latency += out.latency;
            late += usize::from(out.latency > 0);
            delayed += out.delayed_inserts;
        }
        for (plugin, count) in out.unhosted_slots {
            *unhosted.entry(plugin).or_default() += count;
        }
        generators += out.hosted_generators;
        for (plugin, count) in out.unhosted_generators {
            *silent.entry(plugin).or_default() += count;
        }
        let plugin = |channel: &&model::Channel| channel.kind == Some(model::ChannelKind::Plugin);
        plugin_channels += project.channels.iter().filter(plugin).count();
        let named = |slot: &&model::Slot| slot.plugin_name.is_some();
        let inserts = project.inserts.iter();
        slots += inserts.flat_map(|i| &i.slots).filter(named).count();
        let bound = project.automation_bindings();
        bindings += bound.len();
        playable += bound.iter().filter(|b| b.min_raw.is_some()).count();
        automation += out.automation_clips;
        slot_targets += out.slot_targets;
        links += out.controller_links;
        link_targets += out.controller_targets;
        unsupported_links += out.unsupported_controller_targets;
        for link in &project.controller_links {
            let remote = link.link.filter(|_| link.kind == model::LinkKind::Remote);
            let source = remote.and_then(|ids| project.rec_target(ids.source).controller_output());
            let Some(source) = source else {
                continue;
            };
            let slots = project.insert_mix(source.insert).insert().map(|i| &i.slots);
            let slot = slots.and_then(|s| s.iter().find(|s| s.index == i32::from(source.slot)));
            let name = slot.and_then(|slot| slot.plugin_name.as_deref());
            let controller = name.is_some_and(|n| n.eq_ignore_ascii_case("Fruity Peak Controller"));
            if let Some(count) = outputs
                .get_mut(usize::from(source.output))
                .filter(|_| controller)
            {
                *count += 1;
            }
        }
        unsupported_targets += out.unsupported_targets;
        notes += out.notes;
        voices += out.voices;
        clips += out.clips;
        stretched += out.unsupported_stretch;
        audible += usize::from(peak > 0.0);
        unresolved.extend(out.unresolved);
        unsupported.extend(out.unsupported);
    }
    eprintln!(
        "corpus render: {} projects, {audible} with sound, {notes} notes, {voices} voices, \
         {clips} clips played, {stretched} of them with a stretch that is not applied, \
         {} sample paths unresolved, {} hybrid WavPack samples not played",
        files.len(),
        unresolved.len(),
        unsupported.len(),
    );
    eprintln!(
        "corpus automation: {automation} clips evaluated, {bindings} bindings resolved \
         ({playable} of them to a volume, a pan, a send level, the tempo or a channel's level), \
         {slot_targets} targets on a hosted effect slot played, {unsupported_targets} unsupported \
         targets"
    );
    let [both, peak, lfo] = outputs;
    eprintln!(
        "corpus controllers: {links} controller links resolved ({both} from Peak+LFO, {peak} \
         from Peak, {lfo} from LFO), {link_targets} applied, {unsupported_links} unsupported \
         targets"
    );
    let passed: usize = unhosted.values().sum();
    eprintln!(
        "corpus effects: {hosted} slots hosted and {passed} passed through on the inserts \
         sound reaches, of {slots} slots with a plugin; most passed: {}",
        most(&unhosted),
    );
    eprintln!(
        "corpus generators: {generators} channels hosted and {} silent of those with a note, \
         of {plugin_channels} plugin channels; most silent: {}",
        silent.values().sum::<usize>(),
        most(&silent),
    );
    let stood_in: Vec<_> = approximated_generators
        .iter()
        .map(|(what, n)| format!("{n} with {what}"))
        .collect();
    eprintln!(
        "corpus approximated generators: {} of the {generators} hosted channels stand in for \
         what their state asks or lack a file: {}",
        approximated_generators.values().sum::<usize>(),
        stood_in.join(", "),
    );
    let stood_in: Vec<_> = approximated
        .iter()
        .map(|(what, n)| format!("{n} where {what}"))
        .collect();
    eprintln!(
        "corpus approximated: {} of the {hosted} hosted slots stand in for what their state \
         asks: {}",
        approximated.values().sum::<usize>(),
        stood_in.join(", "),
    );
    eprintln!(
        "corpus sidechain: {sidechain_routes} sidechain routes found, {sidechain_slots} of the \
         {hosted} hosted slots fed"
    );
    eprintln!(
        "corpus latency: {latency} frames compensated across the {audible} projects with sound, \
         {late} of them late, {delayed} inserts delayed"
    );
    for path in &unresolved {
        eprintln!("unresolved: {path}");
    }
    for path in &unsupported {
        eprintln!("hybrid WavPack: {path}");
    }
    eprintln!("{}", approximate.line("Approximate"));
    eprintln!("{}", exact.line("FlExact"));
}

/// FL's render tests for Maximus and Soundgoodizer play a loop through one
/// instance of the plugin: both are hosted, and the output has sound and
/// no NaN.
#[test]
fn the_maximus_and_soundgoodizer_render_tests_play_through_the_plugin() {
    let Some(root) = std::env::var_os("FL_RS_CORPUS").map(PathBuf::from) else {
        eprintln!("FL_RS_CORPUS unset, the Maximus render tests skipped");
        return;
    };
    let opts = RenderOptions {
        sample_rate: 48_000,
        sample_root: root.clone(),
        max_seconds: 10.0,
        plugin_dir: None,
    };
    for plugin in ["Maximus", "Soundgoodizer"] {
        let path = root.join(format!("System/Render tests/{plugin}.flp"));
        let file = flp::Project::parse(&std::fs::read(&path).unwrap())
            .unwrap_or_else(|e| panic!("{plugin}: {e}"));
        let project = model::from_flp(&file);
        let out = engine::render(&project, &opts).unwrap_or_else(|e| panic!("{plugin}: {e}"));
        let peak = peak(&out, plugin);
        assert!(peak > 0.05 && peak <= 4.0, "{plugin}: peak {peak}");
        assert!(out.hosted_slots >= 1, "{plugin}: no slot hosted");
        let passed = out
            .unhosted_slots
            .keys()
            .any(|name| name.eq_ignore_ascii_case(plugin));
        assert!(!passed, "{plugin} passed through: {:?}", out.unhosted_slots);
        assert!(
            out.approximated_slots.is_empty(),
            "{plugin}: {:?}",
            out.approximated_slots
        );
    }
}

/// FL's render tests for Fruity Love Philter and Fruity Convolver play
/// through one instance of the plugin: both are hosted, the output has
/// sound and no NaN, and Convolver finds its impulse among FL's files.
#[test]
fn the_love_philter_and_convolver_render_tests_play_through_the_plugin() {
    let Some(root) = std::env::var_os("FL_RS_CORPUS").map(PathBuf::from) else {
        eprintln!("FL_RS_CORPUS unset, the Love Philter and Convolver render tests skipped");
        return;
    };
    for (plugin, sample_rate) in [
        ("Fruity Love Philter", 48_000),
        ("Fruity Convolver", 44_100),
        ("Fruity Convolver", 48_000),
    ] {
        let opts = RenderOptions {
            sample_rate,
            sample_root: root.clone(),
            max_seconds: 10.0,
            plugin_dir: None,
        };
        let path = root.join(format!("System/Render tests/{plugin}.flp"));
        let file = flp::Project::parse(&std::fs::read(&path).unwrap())
            .unwrap_or_else(|e| panic!("{plugin}: {e}"));
        let project = model::from_flp(&file);
        let out = engine::render(&project, &opts).unwrap_or_else(|e| panic!("{plugin}: {e}"));
        let peak = peak(&out, plugin);
        eprintln!(
            "{plugin} at {sample_rate}: peak {peak:.3}, hosted {}, approximated {:?}",
            out.hosted_slots, out.approximated_slots
        );
        assert!(peak > 0.05 && peak <= 4.0, "{plugin}: peak {peak}");
        assert!(out.hosted_slots >= 1, "{plugin}: no slot hosted");
        let passed = out
            .unhosted_slots
            .keys()
            .any(|name| name.eq_ignore_ascii_case(plugin));
        assert!(!passed, "{plugin} passed through: {:?}", out.unhosted_slots);
        // Convolver does what FL's does at the rate of its impulse and at
        // another, which goes through FL's interpolator. Nothing is missing.
        let said: Vec<&str> = out.approximated_slots.keys().copied().collect();
        assert!(said.is_empty(), "{plugin}: {said:?}");
    }
}

/// FL's render tests for Transient Processor, Fruity Multiband Compressor
/// and Fruity Delay Bank play through one instance of the plugin: each is
/// hosted, and the output has sound and no NaN. The first two say they do
/// what FL's does.
#[test]
fn the_transient_multiband_and_delay_bank_render_tests_play_through_the_plugin() {
    let Some(root) = std::env::var_os("FL_RS_CORPUS").map(PathBuf::from) else {
        eprintln!("FL_RS_CORPUS unset, the Transient Processor render tests skipped");
        return;
    };
    let opts = RenderOptions {
        sample_rate: 48_000,
        sample_root: root.clone(),
        max_seconds: 10.0,
        plugin_dir: None,
    };
    for plugin in [
        "Transient Processor",
        "Fruity Multiband Compressor",
        "Fruity Delay Bank",
    ] {
        let path = root.join(format!("System/Render tests/{plugin}.flp"));
        let file = flp::Project::parse(&std::fs::read(&path).unwrap())
            .unwrap_or_else(|e| panic!("{plugin}: {e}"));
        let project = model::from_flp(&file);
        let out = engine::render(&project, &opts).unwrap_or_else(|e| panic!("{plugin}: {e}"));
        let peak = peak(&out, plugin);
        eprintln!(
            "{plugin}: peak {peak:.3}, hosted {}, approximated {:?}",
            out.hosted_slots, out.approximated_slots
        );
        assert!(peak > 0.05 && peak <= 4.0, "{plugin}: peak {peak}");
        assert!(out.hosted_slots >= 1, "{plugin}: no slot hosted");
        let passed = out
            .unhosted_slots
            .keys()
            .any(|name| name.eq_ignore_ascii_case(plugin));
        assert!(!passed, "{plugin} passed through: {:?}", out.unhosted_slots);
        // Delay Bank's filters, oversampling and grain shape are not
        // built, and a state that has one of them on says which.
        let said = out.approximated_slots.keys();
        let own = |text: &&str| plugin == "Fruity Delay Bank" && text.contains("Delay Bank");
        assert!(
            said.clone().all(own),
            "{plugin}: {:?}",
            out.approximated_slots
        );
    }
}

/// FL's render tests for Fruity Flangus, Fruity Blood Overdrive and
/// Spreader play through one instance of the plugin: each is hosted, says
/// it does what FL's does, and the output has sound and no NaN.
///
/// The render test for Fruity Chorus has its chorus inside a Fruity
/// Wrapper, which hosts it under the wrapper's name, so it is not here.
#[test]
fn the_flangus_overdrive_and_spreader_render_tests_play_through_the_plugin() {
    let Some(root) = std::env::var_os("FL_RS_CORPUS").map(PathBuf::from) else {
        eprintln!("FL_RS_CORPUS unset, the Flangus render tests skipped");
        return;
    };
    let opts = RenderOptions {
        sample_rate: 48_000,
        sample_root: root.clone(),
        max_seconds: 10.0,
        plugin_dir: None,
    };
    for plugin in ["Fruity Flangus", "Fruity Blood Overdrive", "Spreader"] {
        let path = root.join(format!("System/Render tests/{plugin}.flp"));
        let file = flp::Project::parse(&std::fs::read(&path).unwrap())
            .unwrap_or_else(|e| panic!("{plugin}: {e}"));
        let project = model::from_flp(&file);
        let out = engine::render(&project, &opts).unwrap_or_else(|e| panic!("{plugin}: {e}"));
        let peak = peak(&out, plugin);
        eprintln!(
            "{plugin}: peak {peak:.3}, hosted {}, approximated {:?}",
            out.hosted_slots, out.approximated_slots
        );
        assert!(peak > 0.05 && peak <= 4.0, "{plugin}: peak {peak}");
        assert!(out.hosted_slots >= 1, "{plugin}: no slot hosted");
        let passed = out
            .unhosted_slots
            .keys()
            .any(|name| name.eq_ignore_ascii_case(plugin));
        assert!(!passed, "{plugin} passed through: {:?}", out.unhosted_slots);
        assert!(
            out.approximated_slots.is_empty(),
            "{plugin}: {:?}",
            out.approximated_slots
        );
    }
}

/// FL's render test for Emphasis plays a loop through one instance of the
/// limiter: it is hosted, and the output has sound, no NaN and stays under
/// the ceiling of its state, -0.08 dB. The state is in Versatile mode,
/// which follows the input once a block, and the plugin says so.
#[test]
fn the_emphasis_render_test_plays_through_the_limiter() {
    let Some(root) = std::env::var_os("FL_RS_CORPUS").map(PathBuf::from) else {
        eprintln!("FL_RS_CORPUS unset, the Emphasis render test skipped");
        return;
    };
    let opts = RenderOptions {
        sample_rate: 44_100,
        sample_root: root.clone(),
        max_seconds: 10.0,
        plugin_dir: None,
    };
    let plugin = "Emphasis";
    let path = root.join(format!("System/Render tests/{plugin}.flp"));
    let file = flp::Project::parse(&std::fs::read(&path).unwrap()).unwrap();
    let project = model::from_flp(&file);
    let out = engine::render(&project, &opts).unwrap();
    let peak = peak(&out, plugin);
    eprintln!(
        "{plugin}: peak {peak:.4}, hosted {}, approximated {:?}",
        out.hosted_slots, out.approximated_slots
    );
    assert!(peak > 0.05 && peak <= 0.992, "{plugin}: peak {peak}");
    assert!(out.hosted_slots >= 1, "{plugin}: no slot hosted");
    let passed = out
        .unhosted_slots
        .keys()
        .any(|name| name.eq_ignore_ascii_case(plugin));
    assert!(!passed, "{plugin} passed through: {:?}", out.unhosted_slots);
    let said: Vec<&str> = out.approximated_slots.keys().copied().collect();
    assert!(said.iter().all(|s| s.contains("Emphasis")), "{said:?}");
}

/// FL's render test for Sytrus plays notes through one instance of the
/// synth: the channel is hosted, the output has sound and no NaN, and the
/// plugin says its operators stand in for FL's.
#[test]
fn the_sytrus_render_test_plays_through_the_generator() {
    let Some(root) = std::env::var_os("FL_RS_CORPUS").map(PathBuf::from) else {
        eprintln!("FL_RS_CORPUS unset, the Sytrus render test skipped");
        return;
    };
    let opts = RenderOptions {
        sample_rate: 48_000,
        sample_root: root.clone(),
        max_seconds: 10.0,
        plugin_dir: None,
    };
    let path = root.join("System/Render tests/Sytrus.flp");
    let file = flp::Project::parse(&std::fs::read(&path).unwrap()).unwrap();
    let project = model::from_flp(&file);
    let out = engine::render(&project, &opts).unwrap();
    let peak = peak(&out, "Sytrus");
    eprintln!(
        "Sytrus: peak {peak:.3}, notes {}, hosted generators {}, approximated {:?}",
        out.notes, out.hosted_generators, out.approximated_generators
    );
    assert!(peak > 0.01 && peak <= 4.0, "Sytrus: peak {peak}");
    assert!(out.hosted_generators >= 1, "Sytrus: no channel hosted");
    assert!(
        !out.unhosted_generators.contains_key("Sytrus"),
        "Sytrus passed over: {:?}",
        out.unhosted_generators
    );
    assert!(
        out.approximated_generators
            .keys()
            .all(|what| what.starts_with("Sytrus")),
        "{:?}",
        out.approximated_generators
    );
}

/// FL's render test for Frequency Shifter plays a 3x Osc through the
/// shifter in its tempo range: the plugin is hosted, says it does what FL's
/// does, and the output has sound and no NaN.
#[test]
fn the_frequency_shifter_render_test_plays_through_the_plugin() {
    let Some(root) = std::env::var_os("FL_RS_CORPUS").map(PathBuf::from) else {
        eprintln!("FL_RS_CORPUS unset, the Frequency Shifter render test skipped");
        return;
    };
    let plugin = "Frequency Shifter";
    let opts = RenderOptions {
        sample_rate: 48_000,
        sample_root: root.clone(),
        max_seconds: 10.0,
        plugin_dir: None,
    };
    let path = root.join(format!("System/Render tests/{plugin}.flp"));
    let file = flp::Project::parse(&std::fs::read(&path).unwrap()).unwrap();
    let project = model::from_flp(&file);
    let out = engine::render(&project, &opts).unwrap();
    let peak = peak(&out, plugin);
    eprintln!(
        "{plugin}: peak {peak:.3}, hosted {}, approximated {:?}",
        out.hosted_slots, out.approximated_slots
    );
    assert!(peak > 0.05 && peak <= 4.0, "{plugin}: peak {peak}");
    assert!(out.hosted_slots >= 1, "{plugin}: no slot hosted");
    let passed = out
        .unhosted_slots
        .keys()
        .any(|name| name.eq_ignore_ascii_case(plugin));
    assert!(!passed, "{plugin} passed through: {:?}", out.unhosted_slots);
    assert!(
        out.approximated_slots.is_empty(),
        "{:?}",
        out.approximated_slots
    );
}
