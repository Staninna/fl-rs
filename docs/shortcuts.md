# Keys and mouse actions

Every key and mouse action the app answers, by window, as `crates/ui` has
them. Ctrl is Cmd on macOS. Where FL Studio's own key was not read from its
code, the doc comment beside the binding says it is inferred; the same goes
for the menus.

A window's keys work only while no text field has the keyboard. The piano
roll's and the playlist's keys work only while the pointer is over that
window. Where two windows share a key, the one under the pointer wins.

## Everywhere

| Key | Does |
|---|---|
| Space | Play or stop |
| Ctrl+Z | Undo |
| Ctrl+Y, Ctrl+Shift+Z | Redo |
| Ctrl+S | Save |
| F5 / F6 / F7 / F8 / F9 | Open, bring to the front or close the playlist / channel rack / piano roll / browser / mixer |

## Top bar

| Action | Does |
|---|---|
| Drag up or down on the tempo | Change it a BPM a point, a tenth with Ctrl held, from 10 to 522 |
| Double click on the tempo | Type the tempo (Enter or a click elsewhere takes it, Escape leaves it) |
| Right click on the position | Beats per bar (1 to 16) and beat value (2, 4, 8, 16) |
| Pattern selector list | Pick a pattern; New, Clone, Rename and Delete pattern |
| F4 | New empty pattern at the lowest free index |
| F2, pointer not over the rack or the mixer | Rename the selected pattern |
| Keypad minus / plus (and the main keyboard's) | Select the previous / next pattern |
| File menu | New, Open, Save, Save as, Render |

## Playlist

| Action | Does |
|---|---|
| Drag a clip | Move it; drag an edge to resize it; Alt for no snap |
| Ctrl+drag | Copy the clips where they are dropped |
| Click, Shift+click, drag on empty space | Select, add to the selection, select a range |
| Right click on a clip | Mute or unmute it |
| Right click on a track name | Mute or unmute the track |
| Delete | Delete the selected clips |
| Ctrl+A / Ctrl+D | Select every clip / none |
| Arrows | Move the selected clips a snap step along, or a track up or down |
| Ctrl+C / Ctrl+V | Copy the selected clips / paste them at the play cursor |
| Ctrl+B | Duplicate the selection right after itself |
| Ctrl+E | Split the selected clips at the play cursor |

## Channel rack

| Action | Does |
|---|---|
| Up / Down | Select the channel above or below |
| Click on a channel's switch | Turn it on or off |
| Right click on a channel's switch | Solo it; again on the soloed one turns them all on |
| Double click on a name, or F2 over the rack | Rename the channel |
| Right click on a name | Channel menu: shift steps left or right, fill each 2/4/8 steps, clear steps, colour, pitch, clone |
| Click or drag on the steps | Light or clear steps; right click clears one |
| Double click on a knob | Put it back at FL's default |
| Drop a sample from the browser | Load it into that channel, or below the last one for a new channel |

## Piano roll

| Action | Does |
|---|---|
| Draw, paint, select, slice tools | Toolbar; right click deletes a note |
| Ctrl+A / Ctrl+D | Select every note of the channel / none |
| Delete | Delete the selected notes |
| Alt+M | Mute or unmute the selected notes |
| Arrows | Move the selection a key, or a snap step along |
| Ctrl+Up / Ctrl+Down | Move the selection an octave |
| Shift+Left / Shift+Right | Shorten / lengthen the selection by a snap step |
| Ctrl+C / Ctrl+X / Ctrl+V | Copy / cut / paste the selection |
| Ctrl+B | Duplicate the selection after itself |
| Ctrl+Q / Shift+Q | Quantize / quantize the starts |
| Alt+U | Chop |
| Alt+A | Arpeggiate |
| Alt+S | Strum |
| Alt+Y | Flip the keys |
| Ctrl+L | Legato |
| Tools menu | Also Glue, Reverse, Snap to scale, and the step and delay of Chop, Arpeggiate and Strum |
| Drag along the ruler | Set the range a pattern loops; a click on it seeks in pattern mode |
| Click on a key | Play the key through the channel while it is held |

## Mixer

| Action | Does |
|---|---|
| Left / Right | Select the insert to the left or right |
| M | Turn the selected insert on or off |
| S | Solo the selected insert |
| Ctrl+L | Link the rack's selected channel to the selected insert |
| F2 over the mixer, or a double click on a name | Rename the insert |
| Click on an insert's switch | Turn it on or off |
| Ctrl+click or right click on an insert's switch | Solo it, with what sends into it and its way to the master |
| Right click on a strip | Strip menu: solo, polarity, swap, separation, colour, bypass or enable all effects |
| Double click on a fader or knob | Put it back at FL's default |

## Browser

| Key | Does |
|---|---|
| Up / Down | Move the selection |
| Left / Right | Close a folder or go up to its parent / open a folder or step into it |
| Enter, double click | Play a sample or open a project |
| Type in the search field | Show the files whose names match, as a flat list |
