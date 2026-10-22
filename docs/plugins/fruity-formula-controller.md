# Fruity Formula Controller: a formula of three knobs, sent as a controller

Fruity Formula Controller compiles a formula of its knobs `a`, `b` and `c`
and of the song, works it out once a tick and sends the result, kept
inside 0 to 1, as its one internal controller output. It has no sound
path. In fl-rs it is `FormulaController`
(`crates/plugins/src/formula_controller.rs`) on the parser in
`crates/plugins/src/fl_formula.rs`.

Read from `Fruity Formula Controller_x64.dll` (FL Studio 26.1.7, Delphi,
image base 0x400000) with headless Ghidra. Tags: `asm` read from the
binary, `dfm` from the editor's form, `corpus` from the shipped projects,
`inferred` where the binary does not say.

## The plugin `asm`

`CreatePlugInstance` @ `0x6d72c0` constructs `TTestPlug` (`0x6d79d0`),
VMT `0x6d7180` (instance 0x188 bytes). `TFruityPlugInfo` @ `0x72f150`:
"Fruity formula controller" / "Formula ctrl", Flags `0x10400060` =
`FPF_WantNewTick | FPF_NoProcess | FPF_Reserved1` and bit 28, 3
parameters, 1 output controller.

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x6d7310 |
| 3 | SaveRestoreState | 0x6d7680 |
| 5 | ProcessEvent | 0x6d7d70 |
| 6 | ProcessParam | 0x6d7da0 |
| 7 | Eff_Render | 0x40a1c0, the RTL's abstract-method stub |
| 14 | NewTick | 0x6d82b0 |

## State `asm` `corpus`

| bytes | what |
|---|---|
| 4 | version, 4 when saved by FL 26 |
| 12 | `a`, `b`, `c`, i32, at `0x120` |
| 1 (+4) + n | the formula: a length byte (255: an i32 follows) and 8-bit text (`0x6c56b0`), trailing characters under 0x20 dropped |
| 1 (+4) + 2n | from version 4 the description: a length and UTF-16 (`0x6c57f0`); before, an i32 length and 8-bit text |
| 1 | from version 2, a switch of the editor |

Both corpus states (Astes - Raubana - LIFE) are version 4 with an empty
formula; their descriptions are the "Randomness" and "Step synchronized
randomness" presets' text.

## Parameters `asm` `dfm`

`Wheel_a`, `Wheel_b`, `Wheel_c`: 0 to 65536, default 32768. `ProcessParam`
stores the value at `0x120 + 4n` and the double `value * (1/65536)` at
`0x12c + 8n`, which the parser knows as `A`, `B` and `C` (`0x6ce100` in the
constructor).

## The parser `asm`

The parser object (`0x6cbab0`, at `0x144`) is a Delphi expression parser.
Its table (`0x6cf080`) holds `(`, `)`, the list separator `,`, the
constant `pi` and these operators, each with its precedence; the smaller
binds tighter:

| precedence | operators | function |
|---|---|---|
| 10 | `!` (`0x6cec40`) | factorial of `Round(x)`, `n * (n - 1)!` while `n > 1.1` |
| 10 | `%` (`0x6cebc0`) | `x * 0.01` |
| 10 | unary `-` (`0x6ce580`), unary `+` | sign flip |
| 10 | `in` | as `=` |
| 20 | `^` (`0x6ce340`, Delphi `Power`); `^` before a whole number is `IntPower` (`0x6ce370`) | |
| 30 | `*`, `/`, `div` and `mod` (of `Round`ed integers), `fmod` (0 for `y = 0`) | |
| 40 | `+`, `-` | |
| 50 | `>`, `>=` (`b <= a + 1e-30`), `<=`, `<`, `<>` (`|a - b| > 1e-30`), `=` (`|a - b| < 1e-30`) | 1 or 0 |
| 60 | `not` (`Round(x)`'s low byte is 0) | |
| 70 | `or`, `and`, `xor`, `shr`, `shl` on `Round`ed integers | |

Functions (`0x6cf080` on): `exp`, `if(c, a, b)` (`Round(c)`'s low byte),
`sqr`, `sqrt`, `abs`, `round`, `trunc`, `floor`, `ceil` (32-bit), `ln`,
`log10`, `logN(base, x)`, `power`/`pow`, `intpower`, `max`, `min`, `sin`,
`cos`, `tan`, `arcsin`, `arccos`, `arctan2`/`atan2(y, x)`, `sinh`, `cosh`,
`tanh`, `arcsinh`, `arccosh`, `arctanh`, `degtorad`, `radtodeg`, `inter(x,
a, b)` and `internoswap` (whether `Round(1000 x)` is between the ends, in
either order for `inter`), `fold` (a triangle of period 2), `warp(x, d)`
(`FMod(x + d, 1)` outside 0 to 1, `FMod(x, 1)` inside) and `map(x, lo,
hi)`.

FL adds (`0x6d1d80`, `0x6d2160`): `Rand` (Delphi's `Random`), `SeededRand`
(`0x5b0460`: Numerical Recipes' `psdes` over the two words of the double,
as a single in 0 to 1), `Min`, `Max`, `Int`, `Round`, `Frac`, `IfE`, `IfL`,
`IfG`, `IfLE`, `IfGE` (1 or 0), `Case(a, b, c)` (`a = 1 ? b : c`), `Sum`,
`Neg`, `Log2`, `Tg`, `CTg`, `ArcTg`, `Sec`, `CoSec` (1 where the cosine or
the sine is 0), `SongTime` (the host's song position in ticks times `1 /
PPQ` as a single, `0x741570`), `GetTempo` (the tempo from `FPE_Tempo`),
`Time` (milliseconds over 1000), `Date`, `Tension(x, t)` (`0x6d17b0`, the
curve of the X-Y Controller's tension in singles) and `MouseX`, `MouseY`
(the cursor). The list is searched from the front, so the names FL adds
twice are the built-in ones; they work out the same.

Compiling (`0x6cb780`):

1. Every `()` is taken out (`0x6d04c0`) and the text lower-cased.
2. The scanner (`0x6cc9c0`) skips spaces and reads numbers (`0x6cc720`:
   digits, `.` digits, `e`, a sign, digits; `StrToFloat` with `.`), `$`
   and hex digits, names of `a`-`z`, `0`-`9` and `_`, and symbols, with
   `<=`, `<>`, `<<`, `>=`, `>>`, `==`, `!=`, `:=`, `&&`, `||` as pairs. A
   name or symbol not in the table raises "Invalid identifier".
3. `0x6cd3d0` checks the brackets pair up, folds a run of `+` and `-` at
   the start, after `(`, `,` or a binary operator into one unary sign by
   the number of `-`, folds a run of `not` by its parity, turns `^` before
   a constant with no `.` into the integer power, and raises on two
   operands side by side, `( )`, `)(`, `)` before an operand and an
   operand before `(`.
4. `0x6cc050` builds the tree: strip the outer brackets; find the first
   operator or function at the outer level (`0x6ca4e0`); a function takes
   its bracket, split at commas at depth 1, as its arguments; an operator
   takes the tokens before it as its first operand and those after it, up
   to the next operator whose precedence is not smaller, as its last; the
   node made is the first operand of the next operator. Every operator is
   so left-associative, and the unary sign (10) binds tighter than `^`
   (20): `-2^2` is 4.
5. `0x6cbc20` raises when a node has fewer arguments than it takes.

An empty formula compiles to nothing, and evaluating nothing (`0x6ce140`)
gives the double at `PTR_DAT_00730170`, all bits set: a NaN with its sign
bit set.

## NewTick `asm`

`0x6d82b0`, once the compile has set `0x15b`:

```
v = (single)evaluate()                          // 0x6d03f0
v = (int)bits(v) < 0 ? 0 : (int)bits(v) > 0x3f800000 ? 1 : v
r = Round(v * 2^30)
if r != sent (0x154): sent = r; OnControllerChanged(0, r)
```

The rest feeds the editor's graph. `FPD_Flush` (2) sets `sent` to -1, so
the next tick sends; the constructor leaves it 0. So the corpus's empty
formulas send 0 once after a flush and never again.

## Corpus `corpus`

Astes - Raubana - LIFE has two, both with empty formulas. The first's
output drives two parameters of the ZGameEditor Visualizer in slot 8 of
the master, which the engine does not play (ZGameEditor has no parameters
here), so its 2 links are counted unsupported.

## What is not FL's

- `Rand` starts from a seed of 0; FL's `RandSeed` is the process's. `Time`,
  `Date`, `MouseX` and `MouseY` are 0. `fidelity()` says so when a formula
  uses them.
- Floating point exceptions are taken as masked (`1/0` is infinity, `sqrt(-1)`
  NaN), as FL's host is taken to set them (`inferred`); integer division by
  zero in `div` and `mod` raises in FL, and the tick then sends nothing,
  as here.
- `SongTime` reads the tick the host hands `tick`, taken to be what FL's
  host method `+0x80` returns (`inferred`).
- Quoted strings, which the scanner reads as string constants, are not
  compiled.
- A loaded state starts flushed (`inferred`, as for the Keyboard Controller).
