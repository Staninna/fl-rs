//! Patcher: a graph of FL's own plugins inside one effect slot or one
//! channel, as `docs/plugins/patcher.md` reads it from `Patcher_x64.dll`.
//!
//! The state ([`crate::patcher_state`]) names the plugins inside, each with
//! its own state, the ports they have, the ports of the Patcher itself
//! towards FL ("From FL" and "To FL"), the wires between ports and which
//! inner parameters the Patcher's own parameters are.
//!
//! # A block
//!
//! FL's `Eff_Render` (`0x94ebd0`) copies the block into the "From FL"
//! ports, a sidechain port the buffer FL has for its number or the block
//! when FL has none (`0x94a9b0`). The graph (`0x79cd80`) then runs every
//! plugin once, in layers: a plugin is one layer past the latest of the
//! plugins wired into any of its ports (`0x79bd40`), and the plugins of a
//! layer go in the order the state has them. Before a plugin runs, each of
//! its sound inputs is the sum of what is wired into it, each wire's source
//! times the wire's gain, and silence with no wire (`0x79bf20`). An effect
//! then runs on its main input; a generator adds its voices to silence. Its
//! second outputs are what it hands its host for them, as it would hand a
//! mixer track. Last, the "To FL" port is the sum of its wires, which
//! replaces the block (`0x94a5c0`).
//!
//! The graph keeps its plugins in step (`0x79bc00`): a plugin's lateness
//! is its own and the most of what is wired into it, a wire is held back
//! by what its source is early, and the Patcher is as late as its "To FL"
//! port.
//!
//! # Controls
//!
//! A wire from a controller output to a parameter carries the output, 0 to
//! 1, to the parameter as FL carries one to a link: `min + Round(v * (max -
//! min))` in 2^30ths ([`crate::wrapper::from_vst`]). A Patcher's parameter
//! `n` is the inner parameter the state exposes as `n` (`0x954040`), in
//! that parameter's own range. [`Effect::tick`] ticks the inner controllers
//! in the graph's order and carries what moved.
//!
//! An inner plugin that is not built passes its main input to its main
//! output and puts nothing out beside it. [`Patcher::unhosted`] names it,
//! and [`Effect::fidelity`] says the Patcher is approximate.

use crate::patcher_state::{Carries, FlPort, Patch};
use crate::{Effect, Fidelity, Generator, NoteOn, SampleFile, Sidechain, StateError, wrapper};
use std::path::{Path, PathBuf};

/// The plugin's event 201 name, the same for the effect and the
/// generator.
pub const NAME: &str = "Patcher";

/// The parameters FL gives the plugin (`0x954040` takes 0 to 0xfff).
pub const PARAMS: usize = 0x1000;

/// The longest block the graph's buffers hold. A longer one is played in
/// parts.
const BLOCK: usize = 512;

/// What [`Effect::fidelity`] says.
const PASSED: &str = "Patcher passes an inner plugin that is not built through";
const UNWIRED: &str = "Patcher has a port towards FL that is not played";

/// A stereo buffer of [`BLOCK`] frames.
#[derive(Debug, Clone, Default)]
struct Stereo {
    left: Vec<f32>,
    right: Vec<f32>,
}

impl Stereo {
    fn new() -> Self {
        Stereo {
            left: vec![0.0; BLOCK],
            right: vec![0.0; BLOCK],
        }
    }

    fn clear(&mut self, n: usize) {
        self.left[..n].fill(0.0);
        self.right[..n].fill(0.0);
    }

    fn copy(&mut self, from: &Stereo, n: usize) {
        self.left[..n].copy_from_slice(&from.left[..n]);
        self.right[..n].copy_from_slice(&from.right[..n]);
    }
}

/// A plugin inside the graph.
enum Plugin {
    Effect(Box<dyn Effect>),
    Generator(Box<dyn Generator>),
    /// One that is not built, or that turned its state down.
    Missing,
}

/// A plugin in the graph with its ports.
struct Module {
    plugin: Plugin,
    muted: bool,
    keys: (i32, i32),
    /// The sound inputs: the plugin's number for each, 0 its main one, and
    /// the buffer it fills.
    inputs: Vec<(usize, usize)>,
    /// The sound outputs, as the inputs.
    outputs: Vec<(usize, usize)>,
    /// Whether FL's notes are wired into it.
    notes: bool,
    /// How late it is by itself, and with all before it.
    own: usize,
    total: usize,
    /// Whether it hears of the ticks: a controller, or a plugin that keeps
    /// in step with the song.
    ticks: bool,
}

impl Module {
    fn effect(&mut self) -> Option<&mut dyn Effect> {
        match &mut self.plugin {
            Plugin::Effect(effect) => Some(effect.as_mut()),
            _ => None,
        }
    }

    fn latency(&self) -> usize {
        match &self.plugin {
            Plugin::Effect(effect) => effect.latency(),
            Plugin::Generator(generator) => generator.latency(),
            Plugin::Missing => 0,
        }
    }

    fn max_latency(&self) -> usize {
        match &self.plugin {
            Plugin::Effect(effect) => effect.max_latency(),
            Plugin::Generator(generator) => generator.latency(),
            Plugin::Missing => 0,
        }
    }

    fn param(&self, index: usize) -> f32 {
        match &self.plugin {
            Plugin::Effect(effect) => effect.param(index),
            Plugin::Generator(generator) => generator.param(index),
            Plugin::Missing => 0.0,
        }
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match &mut self.plugin {
            Plugin::Effect(effect) => effect.set_param(index, value),
            Plugin::Generator(generator) => generator.set_param(index, value),
            Plugin::Missing => {}
        }
    }
}

/// One end of a sound wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    /// A port of the Patcher itself, by its place in [`Graph::fl_in`] or
    /// the "To FL" output.
    Fl,
    Module(usize),
}

/// A wire that carries sound, held back by `delay` frames.
struct Wire {
    from: End,
    /// The buffer it reads.
    source: usize,
    to: End,
    /// The buffer it adds into.
    target: usize,
    gain: f32,
    delay: usize,
    line: Delay,
}

/// A delay line for both sides.
#[derive(Default)]
struct Delay {
    left: Vec<f32>,
    right: Vec<f32>,
    at: usize,
}

impl Delay {
    /// Room for `frames`, empty.
    fn with_room(frames: usize) -> Self {
        Delay {
            left: vec![0.0; frames],
            right: vec![0.0; frames],
            at: 0,
        }
    }

    fn clear(&mut self) {
        self.left.fill(0.0);
        self.right.fill(0.0);
        self.at = 0;
    }
}

/// A wire from a controller output to a parameter.
struct Control {
    from: usize,
    output: usize,
    to: usize,
    param: usize,
    /// The parameter's range.
    range: (i32, i32),
}

/// A parameter of the Patcher's that is one inside it.
struct Exposed {
    param: usize,
    module: usize,
    inner: usize,
    range: (i32, i32),
}

/// A "From FL" sound port: the buffer it fills and FL's number for it, 0
/// the Patcher's input.
struct FlIn {
    buffer: usize,
    number: usize,
    /// The block FL handed for it, kept from [`Effect::set_sidechain`].
    side: Sidechain,
    handed: bool,
}

/// What the plugins inside are told when they are made.
#[derive(Debug, Clone, Default)]
struct Told {
    rate: Option<f32>,
    tempo: Option<f64>,
    ppq: Option<u16>,
    dir: Option<PathBuf>,
}

/// The graph of a Patcher's state.
#[derive(Default)]
struct Graph {
    modules: Vec<Module>,
    /// The places of the modules in the order they run.
    order: Vec<usize>,
    buffers: Vec<Stereo>,
    fl_in: Vec<FlIn>,
    /// The buffer the "To FL" port sums into.
    fl_out: usize,
    wires: Vec<Wire>,
    controls: Vec<Control>,
    exposed: Vec<Exposed>,
    unhosted: Vec<String>,
    /// Something the state asks for that is not played.
    unplayed: bool,
    /// How late the "To FL" port is.
    latency: usize,
    max_latency: usize,
    /// A block for a plugin to run in.
    work: Stereo,
    /// The modules wired into each module, and the most each can be late
    /// with all before it, as [`Graph::align`] last sized them.
    from: Vec<Vec<usize>>,
    most: Vec<usize>,
}

/// The range of a parameter of a plugin, from a second one made for the
/// purpose, so that the first keeps its values.
fn probe_range(name: &str, kind: i32, index: usize) -> (i32, i32) {
    let ends = |set: &mut dyn FnMut(f32) -> f32| (set(f32::MIN) as i32, set(f32::MAX) as i32);
    if kind == 3 {
        if let Some(mut plugin) = crate::generator_by_name(name) {
            return ends(&mut |v| {
                plugin.set_param(index, v);
                plugin.param(index)
            });
        }
    } else if let Some(mut plugin) = crate::by_name(name) {
        return ends(&mut |v| {
            plugin.set_param(index, v);
            plugin.param(index)
        });
    }
    (0, 0)
}

impl Graph {
    /// The graph of a state, its plugins told what `told` has.
    fn build(patch: &Patch, told: &Told) -> Graph {
        let mut graph = Graph {
            buffers: vec![Stereo::new()],
            work: Stereo::new(),
            ..Graph::default()
        };
        // Buffer 0 is the "To FL" sum.
        graph.fl_out = 0;
        // Port id to (module, carries, index, input).
        let mut ports = std::collections::HashMap::new();
        for node in &patch.nodes {
            let at = graph.modules.len();
            let plugin = Self::plugin(node.kind, &node.plugin, &node.state, told);
            if matches!(plugin, Plugin::Missing) {
                graph.unhosted.push(node.plugin.clone());
            }
            for port in &node.ports {
                ports.insert(port.id, (at, port.carries, port.index, port.input));
            }
            let ticks = match &plugin {
                Plugin::Effect(effect) => {
                    !effect.controller_outputs().is_empty() || effect.follows_song()
                }
                _ => false,
            };
            graph.modules.push(Module {
                plugin,
                muted: node.muted,
                keys: node.keys,
                inputs: Vec::new(),
                outputs: Vec::new(),
                notes: false,
                own: 0,
                total: 0,
                ticks,
            });
            // An effect's visible sound inputs are its own whether wired
            // or not: an unwired one is silence.
            for port in &node.ports {
                if port.carries == Carries::Audio && port.input {
                    let buffer = graph.buffer();
                    graph.modules[at].inputs.push((port.index, buffer));
                }
            }
        }
        let fl: Vec<&FlPort> = patch.fl_ports.iter().collect();
        for port in &fl {
            if port.carries == Carries::Audio && port.from_fl {
                let buffer = graph.buffer();
                graph.fl_in.push(FlIn {
                    buffer,
                    number: port.number.max(0) as usize,
                    side: Sidechain::default(),
                    handed: false,
                });
            }
            // A second output of the Patcher's, or notes to or from a MIDI
            // port, is not played.
            let main_out = port.carries == Carries::Audio && !port.from_fl && port.number == 0;
            let own_notes = port.carries == Carries::Notes && port.from_fl && port.number < 0;
            let main_in = port.carries == Carries::Audio && port.from_fl;
            if !(main_out || own_notes || main_in) {
                graph.unplayed = true;
            }
        }
        let fl_port = |id: i32| fl.iter().find(|port| port.id == id).copied();
        let fl_in_at = |graph: &Graph, id: i32| {
            let audio = fl
                .iter()
                .filter(|p| p.carries == Carries::Audio && p.from_fl);
            let place = audio.clone().position(|p| p.id == id)?;
            Some(graph.fl_in[place].buffer)
        };
        for wire in &patch.wires {
            let from = ports.get(&wire.from).copied();
            let to = ports.get(&wire.to).copied();
            match (from, to) {
                // Sound from a plugin's output.
                (Some((m, Carries::Audio, index, false)), to) => {
                    let source = graph.output(m, index);
                    let target = match to {
                        Some((t, Carries::Audio, t_index, true)) => {
                            graph.input(t, t_index).map(|b| (End::Module(t), b))
                        }
                        None if fl_port(wire.to)
                            .is_some_and(|p| !p.from_fl && p.carries == Carries::Audio) =>
                        {
                            (fl_port(wire.to).unwrap().number == 0).then_some((End::Fl, 0))
                        }
                        _ => None,
                    };
                    match target {
                        Some((to, target)) => {
                            graph.wire(End::Module(m), source, to, target, wire.gain)
                        }
                        None => graph.unplayed = true,
                    }
                }
                // Sound from FL.
                (None, to) if fl_port(wire.from).is_some_and(|p| p.from_fl) => {
                    let port = fl_port(wire.from).unwrap();
                    match (port.carries, to) {
                        (Carries::Audio, Some((t, Carries::Audio, t_index, true))) => {
                            let source = fl_in_at(&graph, wire.from).unwrap();
                            match graph.input(t, t_index) {
                                Some(target) => {
                                    graph.wire(End::Fl, source, End::Module(t), target, wire.gain)
                                }
                                None => graph.unplayed = true,
                            }
                        }
                        (Carries::Audio, None)
                            if fl_port(wire.to).is_some_and(|p| !p.from_fl && p.number == 0) =>
                        {
                            let source = fl_in_at(&graph, wire.from).unwrap();
                            graph.wire(End::Fl, source, End::Fl, 0, wire.gain);
                        }
                        (Carries::Notes, Some((t, Carries::Notes, 0, true))) if port.number < 0 => {
                            graph.modules[t].notes = true;
                        }
                        _ => graph.unplayed = true,
                    }
                }
                // A controller output to a parameter.
                (
                    Some((m, Carries::Control, output, false)),
                    Some((t, Carries::Control, param, true)),
                ) => {
                    let node = &patch.nodes[t];
                    graph.controls.push(Control {
                        from: m,
                        output,
                        to: t,
                        param,
                        range: probe_range(&node.plugin, node.kind, param),
                    });
                }
                _ => graph.unplayed = true,
            }
        }
        for exposed in &patch.exposed {
            if exposed.param >= PARAMS {
                continue;
            }
            match ports.get(&exposed.port) {
                Some(&(m, Carries::Control, inner, true)) => {
                    let node = &patch.nodes[m];
                    graph.exposed.push(Exposed {
                        param: exposed.param,
                        module: m,
                        inner,
                        range: probe_range(&node.plugin, node.kind, inner),
                    });
                }
                _ => graph.unplayed = true,
            }
        }
        graph.sort();
        graph.align(true);
        graph
    }

    /// A plugin made from a node's name and state, or [`Plugin::Missing`].
    fn plugin(kind: i32, name: &str, state: &[u8], told: &Told) -> Plugin {
        if kind == 3 {
            let Some(mut generator) = crate::generator_by_name(name) else {
                return Plugin::Missing;
            };
            if let Some(rate) = told.rate {
                generator.set_sample_rate(rate);
            }
            if let Some(tempo) = told.tempo {
                generator.set_tempo(tempo);
            }
            if let Some(ppq) = told.ppq {
                generator.set_ppq(ppq);
            }
            // As the engine's channels: the directory before the state.
            if let Some(dir) = &told.dir {
                generator.set_resource_dir(dir);
            }
            if !state.is_empty() && generator.set_state(state).is_err() {
                return Plugin::Missing;
            }
            return Plugin::Generator(generator);
        }
        let Some(mut effect) = crate::by_name(name) else {
            return Plugin::Missing;
        };
        if let Some(rate) = told.rate {
            effect.set_sample_rate(rate);
        }
        if let Some(tempo) = told.tempo {
            effect.set_tempo(tempo);
        }
        if let Some(ppq) = told.ppq {
            effect.set_ppq(ppq);
        }
        if !state.is_empty() && effect.set_state(state).is_err() {
            return Plugin::Missing;
        }
        // As the engine's slots: the directory after the state.
        if let Some(dir) = &told.dir {
            effect.set_resource_dir(dir);
        }
        effect.route_outputs();
        Plugin::Effect(effect)
    }

    /// A new buffer's place.
    fn buffer(&mut self) -> usize {
        self.buffers.push(Stereo::new());
        self.buffers.len() - 1
    }

    /// The buffer of a module's sound input, when it has that input.
    fn input(&self, module: usize, index: usize) -> Option<usize> {
        let inputs = &self.modules[module].inputs;
        inputs.iter().find(|(i, _)| *i == index).map(|(_, b)| *b)
    }

    /// The buffer of a module's sound output, made the first time a wire
    /// reads it.
    fn output(&mut self, module: usize, index: usize) -> usize {
        let outputs = &self.modules[module].outputs;
        if let Some((_, buffer)) = outputs.iter().find(|(i, _)| *i == index) {
            return *buffer;
        }
        let buffer = self.buffer();
        self.modules[module].outputs.push((index, buffer));
        buffer
    }

    fn wire(&mut self, from: End, source: usize, to: End, target: usize, gain: f32) {
        self.wires.push(Wire {
            from,
            source,
            to,
            target,
            gain,
            delay: 0,
            line: Delay::default(),
        });
    }

    /// The modules every wire into `module` comes from, sound, notes and
    /// controls alike, as `0x79bd40` and `0x79bc00` walk them.
    fn sources(&self, module: usize) -> Vec<usize> {
        let sound = self.wires.iter().filter(|w| w.to == End::Module(module));
        let sound = sound.filter_map(|w| match w.from {
            End::Module(m) => Some(m),
            End::Fl => None,
        });
        let controls = self.controls.iter().filter(|c| c.to == module);
        sound.chain(controls.map(|c| c.from)).collect()
    }

    /// Put the modules in layers and in order (`0x79daa0`). A wire that
    /// would close a loop, which FL's editor does not make, is left out of
    /// the layers.
    fn sort(&mut self) {
        let n = self.modules.len();
        let sources: Vec<Vec<usize>> = (0..n).map(|m| self.sources(m)).collect();
        // 0 not visited, 1 on the way, 2 done.
        let mut state = vec![0u8; n];
        let mut layer = vec![0usize; n];
        fn visit(m: usize, sources: &[Vec<usize>], state: &mut [u8], layer: &mut [usize]) {
            state[m] = 1;
            let mut top = 0;
            for &s in &sources[m] {
                if state[s] == 0 {
                    visit(s, sources, state, layer);
                }
                if state[s] == 2 {
                    top = top.max(layer[s] + 1);
                }
            }
            layer[m] = top;
            state[m] = 2;
        }
        for m in 0..n {
            if state[m] == 0 {
                visit(m, &sources, &mut state, &mut layer);
            }
        }
        self.order = (0..n).collect();
        self.order.sort_by_key(|m| layer[*m]);
    }

    /// Work out how late every module is and hold every wire back by what
    /// its source is early (`0x79bc00`, `0x79bf20`). With `room`, size the
    /// delay lines for the most the plugins can come to, which asks for
    /// memory; without it a delay is kept inside the room there is.
    fn align(&mut self, room: bool) {
        if room {
            self.from = (0..self.modules.len()).map(|m| self.sources(m)).collect();
            self.most = vec![0; self.modules.len()];
        }
        for at in 0..self.order.len() {
            let m = self.order[at];
            let from = &self.from[m];
            let before = from.iter().map(|s| self.modules[*s].total).max();
            let own = self.modules[m].latency();
            if room {
                let most_before = from.iter().map(|s| self.most[*s]).max();
                let most = self.modules[m].max_latency().max(own);
                self.most[m] = most_before.unwrap_or(0) + most;
            }
            let module = &mut self.modules[m];
            module.own = own;
            module.total = before.unwrap_or(0) + own;
        }
        let most = &self.most;
        let into_fl = self.wires.iter().filter(|w| w.to == End::Fl);
        let total = |end: End, modules: &[Module]| match end {
            End::Module(m) => modules[m].total,
            End::Fl => 0,
        };
        self.latency = into_fl
            .clone()
            .map(|w| total(w.from, &self.modules))
            .max()
            .unwrap_or(0);
        self.max_latency = into_fl
            .filter_map(|w| match w.from {
                End::Module(m) => Some(most[m]),
                End::Fl => None,
            })
            .max()
            .unwrap_or(0);
        for at in 0..self.wires.len() {
            let wire = &self.wires[at];
            let arrives = match wire.to {
                End::Module(t) => self.modules[t].total - self.modules[t].own,
                End::Fl => self.latency,
            };
            let delay = arrives.saturating_sub(total(wire.from, &self.modules));
            let wire = &mut self.wires[at];
            if room {
                let longest = match wire.to {
                    End::Module(t) => most[t],
                    End::Fl => self.max_latency,
                };
                wire.line = Delay::with_room(longest.max(delay));
            }
            if wire.delay != delay {
                wire.line.clear();
            }
            wire.delay = delay.min(wire.line.left.len());
        }
    }

    /// Whether a module's lateness moved since [`Graph::align`].
    fn moved(&self) -> bool {
        self.modules.iter().any(|m| m.latency() != m.own)
    }

    /// Add what the wires into `target` carry to it, which starts silent.
    fn gather(&mut self, to: End, target: usize, n: usize) {
        self.buffers[target].clear(n);
        for at in 0..self.wires.len() {
            let wire = &self.wires[at];
            if wire.to != to || wire.target != target {
                continue;
            }
            let (source, gain, delay) = (wire.source, wire.gain, wire.delay);
            let (from, into) = two(&mut self.buffers, source, target);
            let line = &mut self.wires[at].line;
            if delay == 0 {
                for (o, i) in into.left[..n].iter_mut().zip(&from.left[..n]) {
                    *o += i * gain;
                }
                for (o, i) in into.right[..n].iter_mut().zip(&from.right[..n]) {
                    *o += i * gain;
                }
                continue;
            }
            for frame in 0..n {
                let (l, r) = (line.left[line.at], line.right[line.at]);
                line.left[line.at] = from.left[frame];
                line.right[line.at] = from.right[frame];
                line.at = (line.at + 1) % delay;
                into.left[frame] += l * gain;
                into.right[frame] += r * gain;
            }
        }
    }

    /// Run one module on its inputs.
    fn run(&mut self, m: usize, n: usize) {
        for at in 0..self.modules[m].inputs.len() {
            let (_, buffer) = self.modules[m].inputs[at];
            self.gather(End::Module(m), buffer, n);
        }
        let module = &mut self.modules[m];
        let main = module.inputs.iter().find(|(i, _)| *i == 0).map(|(_, b)| *b);
        let work = &mut self.work;
        match main {
            Some(buffer) => work.copy(&self.buffers[buffer], n),
            None => work.clear(n),
        }
        for &(_, buffer) in &module.outputs {
            self.buffers[buffer].clear(n);
        }
        if module.muted {
            return;
        }
        match &mut module.plugin {
            Plugin::Effect(effect) => {
                for &(index, buffer) in module.inputs.iter().filter(|(i, _)| *i > 0) {
                    let side = &self.buffers[buffer];
                    effect.set_sidechain(index, &side.left[..n], &side.right[..n]);
                }
                effect.process(&mut work.left[..n], &mut work.right[..n]);
                let (outputs, buffers) = (&module.outputs, &mut self.buffers);
                effect.outputs(&mut |number, left, right| {
                    let Some((_, buffer)) =
                        outputs.iter().find(|(i, _)| *i == number && number > 0)
                    else {
                        return;
                    };
                    let into = &mut buffers[*buffer];
                    let n = n.min(left.len()).min(right.len());
                    for (o, i) in into.left[..n].iter_mut().zip(left) {
                        *o += i;
                    }
                    for (o, i) in into.right[..n].iter_mut().zip(right) {
                        *o += i;
                    }
                });
            }
            Plugin::Generator(generator) => {
                work.clear(n);
                generator.process(&mut work.left[..n], &mut work.right[..n]);
            }
            Plugin::Missing => {}
        }
        if let Some((_, buffer)) = module.outputs.iter().find(|(i, _)| *i == 0) {
            self.buffers[*buffer].copy(work, n);
        }
    }

    /// Play one part of a block of no more than [`BLOCK`] frames that
    /// starts `offset` frames into the block, and leave the "To FL" sum in
    /// buffer [`Graph::fl_out`].
    fn play(&mut self, left: &[f32], right: &[f32], offset: usize) {
        let n = left.len();
        for port in &mut self.fl_in {
            let into = &mut self.buffers[port.buffer];
            let side = (port.number > 0 && port.handed)
                .then(|| {
                    let l = port.side.left.get(offset..offset + n)?;
                    let r = port.side.right.get(offset..offset + n)?;
                    Some((l, r))
                })
                .flatten();
            let (l, r) = side.unwrap_or((left, right));
            into.left[..n].copy_from_slice(l);
            into.right[..n].copy_from_slice(r);
        }
        for at in 0..self.order.len() {
            let m = self.order[at];
            self.run(m, n);
        }
        self.gather(End::Fl, self.fl_out, n);
    }

    /// Run a whole block, in parts of no more than [`BLOCK`] frames. With
    /// `add`, the "To FL" sum is added to the block; without, it takes
    /// the block's place.
    fn process(&mut self, left: &mut [f32], right: &mut [f32], add: bool) {
        let len = left.len().min(right.len());
        let mut offset = 0;
        while offset < len {
            let n = (len - offset).min(BLOCK);
            let (l, r) = (
                &mut left[offset..offset + n],
                &mut right[offset..offset + n],
            );
            self.play(l, r, offset);
            let out = &self.buffers[self.fl_out];
            if add {
                for (o, i) in l.iter_mut().zip(&out.left[..n]) {
                    *o += i;
                }
                for (o, i) in r.iter_mut().zip(&out.right[..n]) {
                    *o += i;
                }
            } else {
                l.copy_from_slice(&out.left[..n]);
                r.copy_from_slice(&out.right[..n]);
            }
            offset += n;
        }
        for port in &mut self.fl_in {
            port.handed = false;
        }
    }

    /// Tick the controllers in the graph's order and carry what their
    /// outputs say to the parameters they are wired to.
    fn tick(&mut self, ppq_tick: u64, tempo: f64) {
        for at in 0..self.order.len() {
            let m = self.order[at];
            if !self.modules[m].ticks {
                continue;
            }
            let Some(effect) = self.modules[m].effect() else {
                continue;
            };
            effect.tick(ppq_tick, tempo);
            if !effect.controller_changed() {
                continue;
            }
            for c in 0..self.controls.len() {
                let control = &self.controls[c];
                if control.from != m {
                    continue;
                }
                let (to, param, range, output) =
                    (control.to, control.param, control.range, control.output);
                let outputs = match &self.modules[m].plugin {
                    Plugin::Effect(effect) => effect.controller_outputs(),
                    _ => &[],
                };
                let Some(&value) = outputs.get(output) else {
                    continue;
                };
                let raw = wrapper::from_vst(value, range);
                self.modules[to].set_param(param, raw as f32);
            }
        }
        if self.moved() {
            self.align(false);
        }
    }

    fn reset(&mut self) {
        for module in &mut self.modules {
            match &mut module.plugin {
                Plugin::Effect(effect) => effect.reset(),
                Plugin::Generator(generator) => generator.reset(),
                Plugin::Missing => {}
            }
        }
        for wire in &mut self.wires {
            wire.line.clear();
        }
    }

    fn fidelity(&self) -> Fidelity {
        if !self.unhosted.is_empty() {
            return Fidelity::Approximate(PASSED);
        }
        for module in &self.modules {
            let said = match &module.plugin {
                Plugin::Effect(effect) => effect.fidelity(),
                Plugin::Generator(generator) => generator.fidelity(),
                Plugin::Missing => Fidelity::Exact,
            };
            if said != Fidelity::Exact {
                return said;
            }
        }
        if self.unplayed {
            return Fidelity::Approximate(UNWIRED);
        }
        Fidelity::Exact
    }

    fn param(&self, index: usize) -> f32 {
        let exposed = self.exposed.iter().find(|e| e.param == index);
        exposed.map_or(0.0, |e| self.modules[e.module].param(e.inner))
    }

    fn set_param(&mut self, index: usize, value: f32) {
        for at in 0..self.exposed.len() {
            let e = &self.exposed[at];
            if e.param != index {
                continue;
            }
            let (module, inner, (min, max)) = (e.module, e.inner, e.range);
            let value = if value.is_nan() { min as f32 } else { value };
            let value = value.clamp(min as f32, max as f32);
            self.modules[module].set_param(inner, value);
        }
    }

    fn each(&mut self, mut f: impl FnMut(&mut Plugin)) {
        for module in &mut self.modules {
            f(&mut module.plugin);
        }
    }
}

/// One buffer to read and another to write.
fn two(buffers: &mut [Stereo], read: usize, write: usize) -> (&Stereo, &mut Stereo) {
    assert_ne!(read, write, "a wire reads the buffer it writes");
    if read < write {
        let (low, high) = buffers.split_at_mut(write);
        (&low[read], &mut high[0])
    } else {
        let (low, high) = buffers.split_at_mut(read);
        (&high[0], &mut low[write])
    }
}

/// The Patcher in an effect slot, with the graph its state holds. A new
/// one holds nothing and plays silence, as FL's does before its state.
#[derive(Default)]
pub struct Patcher {
    graph: Graph,
    told: Told,
    patch: Patch,
}

impl Patcher {
    /// The plugins inside that are not built, or turned their state down,
    /// by their event 201 names, one for each that the state has.
    pub fn unhosted(&self) -> &[String] {
        &self.graph.unhosted
    }

    fn rebuild(&mut self) {
        self.graph = Graph::build(&self.patch, &self.told);
    }
}

impl Effect for Patcher {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        self.patch = Patch::read(bytes)?;
        self.rebuild();
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        self.told.rate = Some(hz);
        self.graph.each(|plugin| match plugin {
            Plugin::Effect(effect) => effect.set_sample_rate(hz),
            Plugin::Generator(generator) => generator.set_sample_rate(hz),
            Plugin::Missing => {}
        });
        self.graph.align(true);
    }

    fn set_tempo(&mut self, bpm: f64) {
        self.told.tempo = Some(bpm);
        self.graph.each(|plugin| match plugin {
            Plugin::Effect(effect) => effect.set_tempo(bpm),
            Plugin::Generator(generator) => generator.set_tempo(bpm),
            Plugin::Missing => {}
        });
    }

    fn set_ppq(&mut self, ppq: u16) {
        self.told.ppq = Some(ppq);
        self.graph.each(|plugin| match plugin {
            Plugin::Effect(effect) => effect.set_ppq(ppq),
            Plugin::Generator(generator) => generator.set_ppq(ppq),
            Plugin::Missing => {}
        });
    }

    fn set_resource_dir(&mut self, dir: &Path) {
        self.told.dir = Some(dir.to_owned());
        self.graph.each(|plugin| match plugin {
            Plugin::Effect(effect) => effect.set_resource_dir(dir),
            Plugin::Generator(generator) => generator.set_resource_dir(dir),
            Plugin::Missing => {}
        });
        self.graph.align(true);
    }

    fn tick(&mut self, ppq_tick: u64, tempo: f64) {
        self.graph.tick(ppq_tick, tempo);
    }

    fn follows_song(&self) -> bool {
        self.graph.modules.iter().any(|m| m.ticks)
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.graph.process(left, right, false);
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        self.graph.param(index)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        self.graph.set_param(index, value);
    }

    fn reset(&mut self) {
        self.graph.reset();
    }

    fn fidelity(&self) -> Fidelity {
        self.graph.fidelity()
    }

    fn latency(&self) -> usize {
        self.graph.latency
    }

    fn max_latency(&self) -> usize {
        self.graph.max_latency.max(self.graph.latency)
    }

    fn sidechain_input(&self) -> usize {
        self.sidechain_inputs()[0]
    }

    fn sidechain_inputs(&self) -> [usize; 2] {
        let mut numbers = self.graph.fl_in.iter().map(|p| p.number).filter(|n| *n > 0);
        [numbers.next().unwrap_or(0), numbers.next().unwrap_or(0)]
    }

    fn set_sidechain(&mut self, index: usize, left: &[f32], right: &[f32]) {
        for port in &mut self.graph.fl_in {
            if index > 0 && port.number == index {
                port.side.keep(left, right);
                port.handed = true;
            }
        }
    }

    fn unhosted(&self) -> &[String] {
        &self.graph.unhosted
    }
}

/// The Patcher on a channel. FL's notes go to the generators its "Notes"
/// port is wired to.
#[derive(Default)]
pub struct PatcherGenerator {
    graph: Graph,
    told: Told,
    patch: Patch,
}

impl PatcherGenerator {
    /// As [`Patcher::unhosted`].
    pub fn unhosted(&self) -> &[String] {
        &self.graph.unhosted
    }

    /// Every generator FL's notes reach, with the keys it takes.
    fn played(&mut self) -> impl Iterator<Item = (&mut Box<dyn Generator>, (i32, i32))> {
        let modules = self
            .graph
            .modules
            .iter_mut()
            .filter(|m| m.notes && !m.muted);
        modules.filter_map(|m| match &mut m.plugin {
            Plugin::Generator(generator) => Some((generator, m.keys)),
            _ => None,
        })
    }
}

impl Generator for PatcherGenerator {
    fn set_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        self.patch = Patch::read(bytes)?;
        self.graph = Graph::build(&self.patch, &self.told);
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: f32) {
        self.told.rate = Some(hz);
        self.graph.each(|plugin| match plugin {
            Plugin::Effect(effect) => effect.set_sample_rate(hz),
            Plugin::Generator(generator) => generator.set_sample_rate(hz),
            Plugin::Missing => {}
        });
        self.graph.align(true);
    }

    fn set_tempo(&mut self, bpm: f64) {
        self.told.tempo = Some(bpm);
        self.graph.each(|plugin| match plugin {
            Plugin::Effect(effect) => effect.set_tempo(bpm),
            Plugin::Generator(generator) => generator.set_tempo(bpm),
            Plugin::Missing => {}
        });
    }

    fn set_ppq(&mut self, ppq: u16) {
        self.told.ppq = Some(ppq);
        self.graph.each(|plugin| match plugin {
            Plugin::Effect(effect) => effect.set_ppq(ppq),
            Plugin::Generator(generator) => generator.set_ppq(ppq),
            Plugin::Missing => {}
        });
    }

    fn set_resource_dir(&mut self, dir: &Path) {
        self.told.dir = Some(dir.to_owned());
        self.graph.each(|plugin| match plugin {
            Plugin::Effect(effect) => effect.set_resource_dir(dir),
            Plugin::Generator(generator) => generator.set_resource_dir(dir),
            Plugin::Missing => {}
        });
        self.graph.align(true);
    }

    fn note_on(&mut self, note: NoteOn) {
        for (generator, (low, high)) in self.played() {
            if (low..=high).contains(&i32::from(note.key)) {
                generator.note_on(note);
            }
        }
    }

    fn note_off(&mut self, voice: u32) {
        for (generator, _) in self.played() {
            generator.note_off(voice);
        }
    }

    fn all_notes_off(&mut self) {
        for (generator, _) in self.played() {
            generator.all_notes_off();
        }
    }

    fn set_note_pitch(&mut self, voice: u32, pitch: f32) {
        for (generator, _) in self.played() {
            generator.set_note_pitch(voice, pitch);
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.graph.process(left, right, true);
    }

    fn param_count(&self) -> usize {
        PARAMS
    }

    fn param(&self, index: usize) -> f32 {
        self.graph.param(index)
    }

    fn set_param(&mut self, index: usize, value: f32) {
        self.graph.set_param(index, value);
    }

    fn reset(&mut self) {
        self.graph.reset();
    }

    fn tail(&self) -> usize {
        let tails = self.graph.modules.iter().map(|m| match &m.plugin {
            Plugin::Generator(generator) => generator.tail(),
            _ => 0,
        });
        // The effects after the voices ring on too; a second is their
        // allowance.
        let rate = self.told.rate.unwrap_or(44_100.0) as usize;
        tails.max().unwrap_or(0) + self.graph.latency + rate
    }

    fn fidelity(&self) -> Fidelity {
        self.graph.fidelity()
    }

    fn latency(&self) -> usize {
        self.graph.latency
    }

    fn sample_paths(&self) -> Vec<String> {
        let mut paths = Vec::new();
        for module in &self.graph.modules {
            if let Plugin::Generator(generator) = &module.plugin {
                for path in generator.sample_paths() {
                    if !paths.contains(&path) {
                        paths.push(path);
                    }
                }
            }
        }
        paths
    }

    fn set_sample(&mut self, path: &str, file: Option<SampleFile<'_>>) {
        self.graph.each(|plugin| {
            if let Plugin::Generator(generator) = plugin {
                generator.set_sample(path, file);
            }
        });
    }

    fn unhosted(&self) -> &[String] {
        &self.graph.unhosted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, bits, sine};

    /// A Patcher state, chunk by chunk.
    struct State(Vec<u8>);

    /// The ids `node` gives a plugin's ports: its main input, its main
    /// output, its first sidechain input, its second outputs 1 to 3 and,
    /// for a generator, its notes.
    struct Node {
        input: i32,
        output: i32,
        side: i32,
        outs: [i32; 3],
        notes: i32,
    }

    /// FL's main input and output ports, and its notes, by id.
    const FL_IN: i32 = 1000;
    const FL_OUT: i32 = 1001;
    const FL_NOTES: i32 = 1002;

    impl State {
        fn new() -> Self {
            let mut state = State(4i32.to_le_bytes().to_vec());
            state.fl(FL_IN, 0, 7, 0, "Audio input");
            state.fl(FL_OUT, 0, 6, 0, "Audio output");
            state.fl(FL_NOTES, 1, 7, -1, "Notes");
            state
        }

        fn chunk(&mut self, tag: u32, data: &[u8]) {
            self.0.extend(tag.to_le_bytes());
            self.0.extend((data.len() as u64).to_le_bytes());
            self.0.extend(data);
        }

        fn words(&mut self, tag: u32, words: &[i32]) {
            self.chunk(tag, &testing::state(words));
        }

        fn text(&mut self, tag: u32, text: &str) {
            let wide: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
            self.chunk(tag, &wide);
        }

        fn fl(&mut self, id: i32, kind: i32, flags: i32, number: i32, name: &str) {
            let mut data = testing::state(&[id, 0, kind, flags, 0, 0, 0]);
            data[16] = 2;
            data[17..21].copy_from_slice(&number.to_le_bytes());
            self.chunk(0x578, &data);
            self.text(0x57a, name);
            self.chunk(0x579, &[]);
        }

        /// A plugin of `kind` (2 an effect, 3 a generator) with its ports
        /// numbered from `id + 1`.
        fn node(&mut self, kind: i32, id: i32, plugin: &str, state: &[u8]) -> Node {
            self.node_with(kind, id, plugin, state, &[])
        }

        /// As [`State::node`], with control ports: each a parameter (an
        /// output from 0x1000_0000 on), an id and flags.
        fn node_with(
            &mut self,
            kind: i32,
            id: i32,
            plugin: &str,
            state: &[u8],
            controls: &[(i32, i32, i32)],
        ) -> Node {
            self.words(0x1388, &[kind, id, 0, 0, 1, 5, 0, 0, 0, 0]);
            self.text(0x138c, plugin);
            self.text(0x1392, plugin);
            self.chunk(0x138d, state);
            self.words(0x138f, &[0x3f80_0000, -1, 0, 0, i32::MAX, 60, 0, 0, 0, 0]);
            let port = |n: i32| id + n;
            if kind == 3 {
                self.words(0x1391, &[1, 0, port(7), 0, 3, 0, 0]);
            } else {
                self.words(0x1391, &[0, 0, port(1), 0, 3, 0, 0]);
                self.words(0x1391, &[0, 1, port(3), 0, 7, 0, 0]);
            }
            self.words(0x1391, &[0, 0, port(2), 0, 2, 0, 0]);
            for out in 1..=3 {
                self.words(0x1391, &[0, out, port(3 + out), 0, 2, 0, 0]);
            }
            for &(index, port, flags) in controls {
                self.words(0x1391, &[2, index, port, 0, flags, 0, 0]);
            }
            self.chunk(0x1389, &[]);
            Node {
                input: port(1),
                output: port(2),
                side: port(3),
                outs: [port(4), port(5), port(6)],
                notes: port(7),
            }
        }

        fn wire(&mut self, from: i32, to: i32, gain: f32) {
            let gain = gain.to_bits() as i32;
            self.words(0x514, &[to, 0, from, 0, gain, 2, 0, 0]);
            self.chunk(0x515, &[]);
        }

        fn patcher(&self) -> Patcher {
            let mut patcher = Patcher::default();
            patcher.set_sample_rate(44_100.0);
            patcher.set_state(&self.0).unwrap();
            patcher
        }
    }

    /// A tone on the left and another on the right.
    fn tone() -> (Vec<f32>, Vec<f32>) {
        let left: Vec<f32> = sine(0.01, 2000).iter().map(|v| v * 1.5).collect();
        (left, sine(0.023, 2000))
    }

    /// What an effect at its defaults makes of a block.
    fn alone(name: &str, (left, right): &(Vec<f32>, Vec<f32>)) -> (Vec<f32>, Vec<f32>) {
        let mut effect = crate::by_name(name).unwrap();
        effect.set_sample_rate(44_100.0);
        let (mut left, mut right) = (left.clone(), right.clone());
        // In the parts the Patcher plays it in.
        for (l, r) in left.chunks_mut(BLOCK).zip(right.chunks_mut(BLOCK)) {
            effect.process(l, r);
        }
        (left, right)
    }

    fn through(
        patcher: &mut Patcher,
        (left, right): &(Vec<f32>, Vec<f32>),
    ) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (left.clone(), right.clone());
        patcher.process(&mut left, &mut right);
        (left, right)
    }

    #[test]
    fn two_plugins_in_a_chain_are_the_one_after_the_other() {
        let mut state = State::new();
        let dist = state.node(2, 2000, crate::fast_dist::NAME, &[]);
        let clip = state.node(2, 2100, crate::soft_clipper::NAME, &[]);
        state.wire(FL_IN, dist.input, 1.0);
        state.wire(dist.output, clip.input, 1.0);
        state.wire(clip.output, FL_OUT, 1.0);
        let mut patcher = state.patcher();
        let input = tone();
        let first = alone(crate::fast_dist::NAME, &input);
        let both = alone(crate::soft_clipper::NAME, &first);
        let out = through(&mut patcher, &input);
        assert_eq!(bits(&out.0), bits(&both.0));
        assert_eq!(bits(&out.1), bits(&both.1));
        assert_eq!(patcher.fidelity(), Fidelity::Exact);
        assert!(patcher.unhosted().is_empty());
        assert_eq!(patcher.latency(), 0);
    }

    #[test]
    fn a_split_is_summed_at_the_wires_gains() {
        // The input to two plugins side by side, the one at half and the
        // other at a quarter into the output, and the dry input at 2.
        let mut state = State::new();
        let clip = state.node(2, 2100, crate::soft_clipper::NAME, &[]);
        let dist = state.node(2, 2000, crate::fast_dist::NAME, &[]);
        state.wire(FL_IN, dist.input, 1.0);
        state.wire(FL_IN, clip.input, 1.0);
        state.wire(dist.output, FL_OUT, 0.5);
        state.wire(clip.output, FL_OUT, 0.25);
        state.wire(FL_IN, FL_OUT, 2.0);
        let mut patcher = state.patcher();
        let input = tone();
        let a = alone(crate::fast_dist::NAME, &input);
        let b = alone(crate::soft_clipper::NAME, &input);
        let out = through(&mut patcher, &input);
        // In the order the wires are in the state.
        let sum = |x: &[f32], y: &[f32], z: &[f32]| -> Vec<f32> {
            let parts = x.iter().zip(y).zip(z);
            parts
                .map(|((x, y), z)| 0.0 + x * 0.5 + y * 0.25 + z * 2.0)
                .collect()
        };
        assert_eq!(bits(&out.0), bits(&sum(&a.0, &b.0, &input.0)));
        assert_eq!(bits(&out.1), bits(&sum(&a.1, &b.1, &input.1)));
    }

    #[test]
    fn a_plugin_that_is_not_built_passes_its_input_through() {
        let mut state = State::new();
        let missing = state.node(2, 2000, "Distructor", &[1, 2, 3]);
        state.wire(FL_IN, missing.input, 1.0);
        state.wire(missing.output, FL_OUT, 1.0);
        let mut patcher = state.patcher();
        let input = tone();
        let out = through(&mut patcher, &input);
        assert_eq!(bits(&out.0), bits(&input.0));
        assert_eq!(bits(&out.1), bits(&input.1));
        assert_eq!(patcher.unhosted(), ["Distructor"]);
        assert_eq!(Effect::unhosted(&patcher), ["Distructor"]);
        assert_eq!(patcher.fidelity(), Fidelity::Approximate(PASSED));
    }

    #[test]
    fn nothing_wired_to_fl_is_silence() {
        let mut state = State::new();
        state.node(2, 2000, crate::fast_dist::NAME, &[]);
        let mut patcher = state.patcher();
        let out = through(&mut patcher, &tone());
        assert!(out.0.iter().chain(&out.1).all(|v| *v == 0.0));
    }

    #[test]
    fn a_second_output_goes_where_its_number_says() {
        // Stereo Shaper with no left and its difference numbered 1, which
        // is output 1: the input less what the shaper makes of it.
        let shaper = testing::state(&[0, 0, 0, 12800, 0, 0, 0, 1, 0]);
        let mut state = State::new();
        let node = state.node(2, 2000, crate::stereo_shaper::NAME, &shaper);
        state.wire(FL_IN, node.input, 1.0);
        state.wire(node.output, FL_OUT, 1.0);
        state.wire(node.outs[0], FL_OUT, 1.0);
        let mut patcher = state.patcher();
        let input = tone();
        let out = through(&mut patcher, &input);
        // The output and the difference add up to the input.
        let pairs = out.0.iter().zip(&input.0).chain(out.1.iter().zip(&input.1));
        for (o, i) in pairs {
            assert!((o - i).abs() < 1e-6, "{o} {i}");
        }
        // And the left alone is the difference, which is not silent.
        assert!(out.0.iter().any(|v| v.abs() > 0.5));
    }

    #[test]
    fn a_sidechain_port_hears_its_own_wire() {
        let mut state = State::new();
        let node = state.node(2, 2000, crate::limiter::NAME, &[]);
        state.wire(FL_IN, node.input, 1.0);
        state.wire(FL_IN, node.side, 0.5);
        state.wire(node.output, FL_OUT, 1.0);
        let mut patcher = state.patcher();
        let out = through(&mut patcher, &tone());
        assert!(out.0.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn notes_reach_the_generator_fl_s_notes_are_wired_to() {
        let mut state = State::new();
        let sine = state.node(3, 2000, crate::test_sine::NAME, &[]);
        state.wire(FL_NOTES, sine.notes, 1.0);
        state.wire(sine.output, FL_OUT, 1.0);
        let mut patcher = PatcherGenerator::default();
        patcher.set_sample_rate(44_100.0);
        patcher.set_state(&state.0).unwrap();
        let mut alone = crate::generator_by_name(crate::test_sine::NAME).unwrap();
        alone.set_sample_rate(44_100.0);
        let note = NoteOn::default();
        patcher.note_on(note);
        alone.note_on(note);
        let (mut left, mut right) = (vec![0.25; 700], vec![0.0; 700]);
        let (mut l2, mut r2) = (vec![0.0; 700], vec![0.0; 700]);
        patcher.process(&mut left, &mut right);
        for (l, r) in l2.chunks_mut(BLOCK).zip(r2.chunks_mut(BLOCK)) {
            alone.process(l, r);
        }
        let added: Vec<f32> = l2.iter().map(|v| 0.25 + v).collect();
        assert_eq!(bits(&left), bits(&added));
        assert_eq!(bits(&right), bits(&r2));
        assert!(right.iter().any(|v| *v != 0.0));
        assert_eq!(Generator::fidelity(&patcher), Fidelity::Exact);
    }

    #[test]
    fn every_patcher_in_fl_s_projects_loads_and_plays() {
        let Some(root) = testing::corpus() else {
            eprintln!("FL_RS_CORPUS unset, the states in FL's projects skipped");
            return;
        };
        let mut states = 0;
        let mut unhosted = std::collections::BTreeMap::<String, usize>::new();
        for saved in testing::projects(&root) {
            if !saved.plugin.eq_ignore_ascii_case(NAME) {
                continue;
            }
            states += 1;
            let patch = Patch::read(&saved.state).unwrap();
            let generator = patch.fl_ports.iter().any(|p| p.carries == Carries::Notes);
            let input = tone();
            let (mut left, mut right) = input.clone();
            let names = if generator {
                let mut patcher = PatcherGenerator::default();
                patcher.set_sample_rate(44_100.0);
                patcher.set_tempo(120.0);
                patcher.set_state(&saved.state).unwrap();
                patcher.note_on(NoteOn::default());
                patcher.process(&mut left, &mut right);
                if saved.file.starts_with("Patcher (") {
                    // Sytrus, notes in and sound out.
                    assert_eq!(patcher.graph.modules.len(), 2);
                    assert!(patcher.graph.modules[1].notes);
                    assert!(left.iter().any(|v| *v != input.0[0]));
                }
                patcher.unhosted().to_vec()
            } else {
                let mut patcher = Patcher::default();
                patcher.set_sample_rate(44_100.0);
                patcher.set_tempo(120.0);
                patcher.set_state(&saved.state).unwrap();
                for tick in 0..4 {
                    patcher.tick(tick, 120.0);
                }
                patcher.process(&mut left, &mut right);
                assert!(patcher.latency() <= patcher.max_latency());
                if saved.file.starts_with("Patcher (") {
                    // Delay 2 between the input and the output.
                    let delay = &patcher.patch.nodes[1];
                    assert_eq!(delay.plugin, crate::delay_2::NAME);
                    let mut alone = crate::by_name(&delay.plugin).unwrap();
                    alone.set_sample_rate(44_100.0);
                    alone.set_tempo(120.0);
                    alone.set_state(&delay.state).unwrap();
                    let (mut l, mut r) = input.clone();
                    for (l, r) in l.chunks_mut(BLOCK).zip(r.chunks_mut(BLOCK)) {
                        alone.process(l, r);
                    }
                    assert_eq!(bits(&left), bits(&l));
                    assert_eq!(bits(&right), bits(&r));
                    assert_eq!(patcher.fidelity(), Fidelity::Exact);
                }
                patcher.unhosted().to_vec()
            };
            for name in names {
                *unhosted.entry(name).or_default() += 1;
            }
            let sound = left.iter().chain(&right);
            assert!(
                sound.clone().all(|v| v.is_finite() && v.abs() < 100.0),
                "{}",
                saved.file
            );
        }
        // 30 in mixer slots and 3 on channels in 12 projects, and the two
        // render tests.
        assert_eq!(states, 35);
        let names: Vec<&str> = unhosted.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            [
                "Distructor",
                "Fruity Formula Controller",
                "Fruity X-Y-Z Controller"
            ]
        );
    }

    /// Frequency Splitter's state, version 4, at its defaults but for
    /// these parameters and band targets.
    fn splitter(set: &[(usize, i32)], targets: [i32; 3]) -> Vec<u8> {
        let mut params = [19_800, 48_065, 3, 3, 0, 1, 2, 45_820, 0, 0, 0, 0, 0, 0];
        for &(index, value) in set {
            params[index] = value;
        }
        let mut words = vec![4];
        words.extend(params);
        words.extend(targets);
        words.extend([0, 0, 0, 0]);
        let mut bytes = testing::state(&words);
        bytes.push(0);
        bytes.extend(testing::state(&[0; 6]));
        assert_eq!(bytes.len(), 113);
        bytes
    }

    #[test]
    fn a_path_through_a_late_plugin_holds_the_others_back() {
        // Linear phase is late; the dry wire beside it waits for it.
        let linear = splitter(&[(4, 1)], [0; 3]);
        let mut state = State::new();
        let node = state.node(2, 2000, crate::frequency_splitter::NAME, &linear);
        state.wire(FL_IN, node.input, 1.0);
        state.wire(node.output, FL_OUT, 1.0);
        state.wire(FL_IN, FL_OUT, 1.0);
        let mut patcher = state.patcher();
        let mut alone = crate::by_name(crate::frequency_splitter::NAME).unwrap();
        alone.set_sample_rate(44_100.0);
        alone.set_state(&linear).unwrap();
        let late = alone.latency();
        assert!(late > 0);
        assert_eq!(patcher.latency(), late);
        let input: (Vec<f32>, Vec<f32>) = (sine(0.01, 9000), sine(0.02, 9000));
        let wet = {
            let (mut l, mut r) = input.clone();
            for (l, r) in l.chunks_mut(BLOCK).zip(r.chunks_mut(BLOCK)) {
                alone.process(l, r);
            }
            (l, r)
        };
        let out = through(&mut patcher, &input);
        for frame in 0..9000usize {
            let dry = |side: &[f32]| frame.checked_sub(late).map_or(0.0, |at| side[at]);
            assert_eq!(out.0[frame], 0.0 + wet.0[frame] + dry(&input.0), "{frame}");
            assert_eq!(out.1[frame], 0.0 + wet.1[frame] + dry(&input.1), "{frame}");
        }
    }

    #[test]
    fn a_splitter_s_bands_go_to_the_outputs_its_targets_name() {
        // Two bands at their main levels on the main output, and the low
        // band also to output 2 and the high band to output 1, at their
        // send levels of 0 dB: the two outputs add up to the main one.
        let bands = splitter(&[(5, 0)], [2, 0, 1]);
        let mut state = State::new();
        let node = state.node(2, 2000, crate::frequency_splitter::NAME, &bands);
        state.wire(FL_IN, node.input, 1.0);
        state.wire(node.outs[0], FL_OUT, 1.0);
        state.wire(node.outs[1], FL_OUT, -1.0);
        let mut patcher = state.patcher();
        assert_eq!(patcher.fidelity(), Fidelity::Exact);
        let mut low = state_with(&[(node.outs[1], FL_OUT)], &bands);
        let out = through(&mut patcher, &tone());
        let lows = through(&mut low, &tone());
        // High less low.
        assert!(out.0.iter().any(|v| v.abs() > 0.01));
        assert!(lows.0.iter().any(|v| v.abs() > 0.01));
        let mut main = state_with(&[(node.output, FL_OUT)], &bands);
        let mains = through(&mut main, &tone());
        let mut high = state_with(&[(node.outs[0], FL_OUT)], &bands);
        let highs = through(&mut high, &tone());
        for frame in 1000..2000 {
            let sum = highs.0[frame] + lows.0[frame];
            assert!((sum - mains.0[frame]).abs() < 1e-5, "{frame}");
            assert_eq!(out.0[frame], 0.0 + highs.0[frame] - lows.0[frame]);
        }
    }

    /// A Patcher of one Frequency Splitter in `2000` with the input wired
    /// in and these wires out.
    fn state_with(wires: &[(i32, i32)], splitter: &[u8]) -> Patcher {
        let mut state = State::new();
        let node = state.node(2, 2000, crate::frequency_splitter::NAME, splitter);
        state.wire(FL_IN, node.input, 1.0);
        for &(from, to) in wires {
            state.wire(from, to, 1.0);
        }
        state.patcher()
    }

    #[test]
    fn a_controller_and_an_exposed_parameter_move_a_plugin_s_parameter() {
        // X-Y Controller's X to Fast Dist's threshold, and the Patcher's
        // parameter 7 is Fast Dist's mix.
        let mut state = State::new();
        state.node_with(
            2,
            2000,
            crate::xy_controller::NAME,
            &[],
            &[(0x1000_0000, 1990, 10)],
        );
        let dist = state.node_with(
            2,
            2100,
            crate::fast_dist::NAME,
            &[],
            &[(1, 2200, 11), (4, 2201, 11)],
        );
        state.wire(FL_IN, dist.input, 1.0);
        state.wire(dist.output, FL_OUT, 1.0);
        state.wire(1990, 2200, 1.0);
        state.words(0x44c, &[2201, 0, 7, 0, 0, 0, 0, 0]);
        let mut patcher = state.patcher();
        let dist = 1;
        let xy = &patcher.graph.modules[0];
        let x = match &xy.plugin {
            Plugin::Effect(effect) => effect.param(0),
            _ => unreachable!(),
        };
        assert!(patcher.follows_song());
        patcher.tick(0, 120.0);
        let range = probe_range(crate::fast_dist::NAME, 2, 1);
        let outputs = match &patcher.graph.modules[0].plugin {
            Plugin::Effect(effect) => effect.controller_outputs().to_vec(),
            _ => unreachable!(),
        };
        let threshold = wrapper::from_vst(outputs[0], range) as f32;
        assert_eq!(patcher.graph.modules[dist].param(1), threshold, "x {x}");
        // The mix, in Fast Dist's own range.
        let (low, high) = probe_range(crate::fast_dist::NAME, 2, 4);
        patcher.set_param(7, f32::MAX);
        assert_eq!(patcher.param(7), high as f32);
        patcher.set_param(7, f32::MIN);
        assert_eq!(patcher.param(7), low as f32);
        assert_eq!(patcher.graph.modules[dist].param(4), low as f32);
        assert_eq!(patcher.param(6), 0.0);
    }
}
