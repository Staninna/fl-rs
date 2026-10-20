# ZGameEditor Visualizer: when its sound is its input

ZGameEditor Visualizer draws layers of ZGameEditor effects to the music.
Its `Eff_Render` passes the input on and then adds what can sound in it:
an effect that renders audio, the audio of video clips, and ZGameEditor's
own mixer. fl-rs hosts it (`crates/plugins/src/zgameeditor.rs`, a pass
through) for the states where none of those can add anything, and turns any
other state down as unhosted, so that the host passes it through and counts
it.

Read from `ZGameEditor Visualizer_x64.dll` (FL Studio 26.1.7, Delphi, image
base 0x400000) with Ghidra decompiles and capstone listings.

## The way in

`CreatePlugInstance` @ `0x9b2b70` calls the constructor `0x9215d0` with the
class reference `0x916c70`, VMT `0x916d38`: Dispatcher `0x921a90`,
ProcessParam `0x922b20`, `Eff_Render` `0x9232e0`. `TFruityPlugInfo` @
`0xa3bf28`: flags `0x400020`, 5072 parameters, 1024 controller outputs.

## The state

A little-endian u32 length and that much UTF-8 INI text (some start with a
byte order mark): `[General]`, one `[AppSetN]` a layer with `App=<category>\
<effect>` (the effect file `Effects/<category>/<effect>.zgeproj`), its
parameters, `Input` and `Enabled`, then `[Video export]` (the settings of
the video renderer, which only writes files), `[UserContent]` (`Text`,
`Html`, `Images`, `Meshes`, `VideoCues`, ...) and `[Detached]` (the window).

## `Eff_Render` @ `0x9232e0`

1. When `Source` and `Dest` differ, `ippsCopy_64s(Source, Dest, Length)`
   (`[0xa47a98]` -> `0xa54eb8`, bound by name at `0x5c8672`).
2. While the song plays (`this+0x1383`, set by dispatch 12 at `0x921a90`)
   and the scene's `+0x4779` is set, `0x9947d0` adds into `Dest` the 16-bit
   audio of every enabled layer whose effect renders audio (or zeroes a
   stretch when that audio runs short). `+0x4779` is set (`0x966bb0`) when
   such a layer is enabled, and an effect renders audio (its `+0x170`, set
   at `0x96cdfb`) only when it defines all of `AudioOutputModel`,
   `AudioOutputRenderTarget`, `AudioOutputTime` and `AudioOutputSampleRate`.
   Of the 328 effects FL ships, only `Misc\AudioGenerator` does.
3. While the song plays and the video transport's `+0x1e0` is set,
   `0x90c640` runs each of its clips (`0x8a6230`), which adds the clip's
   audio into `Dest`: only clips loaded as content have any.
4. Always, under ZGameEditor's global audio lock (`0xa45e20`): its own
   buffer at `this+0x134c` is zeroed (`0x40ced0`) and filled by its mixer
   (`0x7a1670`): 16 channels at `0xa5bc48` that an effect's `Sound`
   components start, a `Music` player (`0xa471f8`), a master volume. The
   buffer is then added to `Dest` with gains 1.0 by `[0xa479c0]` ->
   `0xa56618`, bound by CPU at `0x66ded6`/`0x66dfa8` to `0x669690` or
   `0x669700`: `Dest += buffer * gain`, four floats at a time. With no
   channel started and no music, the buffer stays 0.

## What fl-rs hosts

A state is hosted when every layer's `App` is one of the effects read to
make no sound, and its content loads no video: `Images` lists only still
images (`.png`, `.jpg`, `.jpeg`, `.bmp`), `VideoCues` is empty, and
`[UserContent]` has no other key than `Text`, `Html`, `Meshes`,
`MeshAutoScale`, `MeshWithColors`, `VideoUseSync` and `Filtering`. Then steps
2 to 4 add nothing and the output is the input, but that step 4's add of
zeros turns a `-0.0` into `+0.0`, which fl-rs leaves as it was.

The effects read to make no sound are the 31 the corpus's states run, each
`.zgeproj` checked for an `AudioOutput` producer, a `Sound`, `Music`,
`MusicControl`, `Sample` or `Channel` component and the video transport:
Background\FogMachine, \FourCornerGradient, \ItsFullOfStars, \SolidColor;
Blend\BufferBlender; Canvas effects\Digital Brain, \Electric, \FreqRing,
\ShimeringCage; Image effects\Image; Misc\Automator; Object Arrays\Rings;
Particles\ColorBlobs, \ReactiveFlow, \ReactiveMob; Peak Effects\JoyDividers,
\Linear, \Polar, \Reactive Sphere, \ReflectedPeeks, \WaveSimple;
Postprocess\Ascii, \AudioShake, \Blooming, \ColorCyclePalette, \FrameBlur,
\RGB Shift; Scenes\Lantern; Terrain\CubesAndSpheres, \GoopFlow;
Text\TextDraw. Another effect is added to `SILENT` in the module once its
file is read the same way. `Misc\AudioGenerator` makes sound, and
`Scenes\Abstract Balloon Flight` has a `MusicControl`.

All 9 corpus states are hosted: Astes - Raubana - LIFE, JuiceBass -
SeamlessR - Menagerie, Sacco - Expanding universe, Exploding Chips,
FlashyWaves and Pumping objects, Treben - Gimbal and Sinan - RawFL, Treben -
Umziky - All the same, and Video chopping (one Image layer with no image
loaded).

Left out: the display, the parameters, and the 1024 controller outputs (no
corpus project links one).
