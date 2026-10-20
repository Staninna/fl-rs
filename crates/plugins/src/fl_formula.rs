//! The expression parser in Fruity Formula Controller's DLL: a Delphi
//! parser class (`0x6cbab0`) with a table of operators and functions
//! (`0x6cf080`) and the functions FL adds to it (`0x6d1d80`, `0x6d2160`),
//! as `docs/plugins/fruity-formula-controller.md` reads them. A formula is
//! compiled once, which allocates, and evaluated once a tick, which does
//! not.
//!
//! # Text to tokens
//!
//! Every `()` is taken out first (`0x6d04c0`), so `Rand()` is `Rand`, and
//! the text is lower-cased. The scanner (`0x6cc9c0`) skips spaces and reads
//! a number (`0x6cc720`: digits, a `.` and digits, an `e`, a sign and
//! digits), a `$` and hex digits, a name of `a` to `z`, `0` to `9` and `_`,
//! or a symbol, two characters for `<=`, `<>`, `>=` and their kind. A name
//! or a symbol the table has not got does not compile.
//!
//! Then (`0x6cd3d0`): a run of `+` and `-` at the start, after `(`, `,` or
//! a binary operator, is one unary `-` or `+` by how many `-` it has; a run
//! of `not` is one `not` or none; `^` before a whole number is the integer
//! power. Two operands side by side, `)(`, `) 1` and `1 (` do not compile.
//!
//! # Tokens to a tree
//!
//! `0x6cc050` takes the first operator or function at the outer level. A
//! function takes the bracket after it, split at its commas, as its
//! arguments; an operator takes what is before it as its first operand and
//! what follows, up to the next operator that binds no tighter, as its
//! last. Whatever it made is then the first operand of the next operator,
//! and so on to the end. So every operator is left-associative, and a
//! unary sign binds tighter than `^`: `-2^2` is 4.
//!
//! | precedence | operators |
//! |---|---|
//! | 10 | `!` (factorial), `%` (hundredths), unary `-` and `+`, `in` |
//! | 20 | `^` |
//! | 30 | `*`, `/`, `div`, `mod`, `fmod` |
//! | 40 | `+`, `-` |
//! | 50 | `>`, `>=`, `<=`, `<`, `<>`, `=` |
//! | 60 | `not` |
//! | 70 | `or`, `and`, `xor`, `shr`, `shl` |

/// The most nodes a formula has; one with more does not compile.
const MOST: usize = 4096;

/// What a formula reads beside its numbers, filled in by its host.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct Inputs {
    /// `a`, `b` and `c`.
    pub vars: [f64; 3],
    /// `SongTime`, in beats.
    pub song_time: f64,
    /// `GetTempo`, in BPM.
    pub tempo: f64,
    /// `Time`, in seconds.
    pub time: f64,
}

/// What a formula asks for that fl-rs can only stand in for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Uses {
    /// `Rand`: Delphi's `Random`, whose seed is the whole process's.
    pub rand: bool,
    /// `Time`, `Date`, `MouseX` and `MouseY`: the clock and the mouse.
    pub outside: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Func {
    Factorial,
    Percent,
    Negate,
    Plus,
    Power,
    IntPower,
    Multiply,
    Divide,
    Div,
    Mod,
    FMod,
    Add,
    Subtract,
    Greater,
    GreaterEqual,
    LessEqual,
    Less,
    NotEqual,
    Equal,
    Not,
    Or,
    And,
    Xor,
    Shr,
    Shl,
    Exp,
    If,
    Sqr,
    Sqrt,
    Abs,
    Round,
    Trunc,
    Floor,
    Ceil,
    Ln,
    Log10,
    LogN,
    Max,
    Min,
    Sin,
    Cos,
    Tan,
    ArcSin,
    ArcCos,
    ArcTan2,
    Sinh,
    Cosh,
    Tanh,
    ArcSinh,
    ArcCosh,
    ArcTanh,
    DegToRad,
    RadToDeg,
    Inter,
    InterNoSwap,
    Fold,
    Warp,
    Map,
    // FL's own (`0x6d1d80`, `0x6d2160`).
    Rand,
    SeededRand,
    Int,
    Frac,
    IfE,
    IfL,
    IfG,
    IfLE,
    IfGE,
    Case,
    Sum,
    Neg,
    Log2,
    Cotan,
    ArcTan,
    Sec,
    CoSec,
    SongTime,
    GetTempo,
    Time,
    Date,
    Tension,
    MouseX,
    MouseY,
}

/// An entry of the table: a name, what it does, how many it takes, its
/// precedence and whether it is an operator (and not a function).
struct Entry {
    name: &'static str,
    func: Func,
    arity: u8,
    precedence: u8,
    operator: bool,
}

const fn op(name: &'static str, func: Func, arity: u8, precedence: u8) -> Entry {
    Entry {
        name,
        func,
        arity,
        precedence,
        operator: true,
    }
}

const fn function(name: &'static str, func: Func, arity: u8) -> Entry {
    Entry {
        name,
        func,
        arity,
        precedence: 0,
        operator: false,
    }
}

/// The table in the order `0x6cf080` and FL's two functions fill it. A
/// name is found from the front, so FL's own `Min`, `Max` and `Round` are
/// the built-in ones.
const TABLE: &[Entry] = &[
    op("!", Func::Factorial, 1, 10),
    op("%", Func::Percent, 1, 10),
    op("-@", Func::Negate, 1, 10),
    op("+@", Func::Plus, 1, 10),
    op("^", Func::Power, 2, 20),
    op("^@", Func::IntPower, 2, 20),
    op("*", Func::Multiply, 2, 30),
    op("/", Func::Divide, 2, 30),
    op("div", Func::Div, 2, 30),
    op("mod", Func::Mod, 2, 30),
    op("fmod", Func::FMod, 2, 30),
    op("+", Func::Add, 2, 40),
    op("-", Func::Subtract, 2, 40),
    op(">", Func::Greater, 2, 50),
    op(">=", Func::GreaterEqual, 2, 50),
    op("<=", Func::LessEqual, 2, 50),
    op("<", Func::Less, 2, 50),
    op("<>", Func::NotEqual, 2, 50),
    op("=", Func::Equal, 2, 50),
    op("in", Func::Equal, 2, 10),
    op("not", Func::Not, 1, 60),
    op("or", Func::Or, 2, 70),
    op("and", Func::And, 2, 70),
    op("xor", Func::Xor, 2, 70),
    op("shr", Func::Shr, 2, 70),
    op("shl", Func::Shl, 2, 70),
    function("exp", Func::Exp, 1),
    function("if", Func::If, 3),
    function("sqr", Func::Sqr, 1),
    function("sqrt", Func::Sqrt, 1),
    function("abs", Func::Abs, 1),
    function("round", Func::Round, 1),
    function("trunc", Func::Trunc, 1),
    function("floor", Func::Floor, 1),
    function("ceil", Func::Ceil, 1),
    function("ln", Func::Ln, 1),
    function("log10", Func::Log10, 1),
    function("logn", Func::LogN, 2),
    function("power", Func::Power, 2),
    function("pow", Func::Power, 2),
    function("intpower", Func::IntPower, 2),
    function("max", Func::Max, 2),
    function("min", Func::Min, 2),
    function("sin", Func::Sin, 1),
    function("cos", Func::Cos, 1),
    function("tan", Func::Tan, 1),
    function("arcsin", Func::ArcSin, 1),
    function("arccos", Func::ArcCos, 1),
    function("arctan2", Func::ArcTan2, 2),
    function("sinh", Func::Sinh, 1),
    function("cosh", Func::Cosh, 1),
    function("tanh", Func::Tanh, 1),
    function("arcsinh", Func::ArcSinh, 1),
    function("arccosh", Func::ArcCosh, 1),
    function("arctanh", Func::ArcTanh, 1),
    function("degtorad", Func::DegToRad, 1),
    function("radtodeg", Func::RadToDeg, 1),
    function("atan2", Func::ArcTan2, 2),
    function("inter", Func::Inter, 3),
    function("internoswap", Func::InterNoSwap, 3),
    function("fold", Func::Fold, 1),
    function("warp", Func::Warp, 2),
    function("map", Func::Map, 3),
    function("rand", Func::Rand, 0),
    function("seededrand", Func::SeededRand, 1),
    function("int", Func::Int, 1),
    function("frac", Func::Frac, 1),
    function("ife", Func::IfE, 2),
    function("ifl", Func::IfL, 2),
    function("ifg", Func::IfG, 2),
    function("ifle", Func::IfLE, 2),
    function("ifge", Func::IfGE, 2),
    function("case", Func::Case, 3),
    function("sum", Func::Sum, 2),
    function("neg", Func::Neg, 1),
    function("log2", Func::Log2, 1),
    function("tg", Func::Tan, 1),
    function("ctg", Func::Cotan, 1),
    function("arctg", Func::ArcTan, 1),
    function("sec", Func::Sec, 1),
    function("cosec", Func::CoSec, 1),
    function("songtime", Func::SongTime, 0),
    function("gettempo", Func::GetTempo, 0),
    function("time", Func::Time, 0),
    function("date", Func::Date, 0),
    function("tension", Func::Tension, 2),
    function("mousex", Func::MouseX, 0),
    function("mousey", Func::MouseY, 0),
];

/// A token of a formula.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Token {
    Open,
    Close,
    Comma,
    Number(f64),
    /// `a`, `b` or `c`.
    Var(usize),
    /// An entry of [`TABLE`].
    Entry(usize),
}

impl Token {
    fn operand(self) -> bool {
        matches!(self, Token::Number(_) | Token::Var(_))
    }

    fn entry(self) -> Option<&'static Entry> {
        match self {
            Token::Entry(at) => Some(&TABLE[at]),
            _ => None,
        }
    }

    /// An operator or a function: what takes arguments.
    fn takes(self) -> bool {
        self.entry().is_some_and(|entry| entry.arity > 0)
    }

    fn binary(self) -> bool {
        self.entry().is_some_and(|e| e.operator && e.arity == 2)
    }

    fn is(self, name: &str) -> bool {
        self.entry().is_some_and(|entry| entry.name == name)
    }
}

/// The entry of the table with this name.
fn lookup(name: &str) -> Option<Token> {
    let at = TABLE.iter().position(|entry| entry.name == name)?;
    Some(Token::Entry(at))
}

/// Why a formula did not compile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Invalid;

/// Split the text into tokens: `0x6ccfc0` with the scanner `0x6cc9c0`.
fn tokens(text: &str) -> Result<Vec<Token>, Invalid> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    let name = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_';
    while at < bytes.len() {
        if bytes[at] == b' ' {
            at += 1;
            continue;
        }
        let start = at;
        let c = bytes[at];
        let token = if c.is_ascii_digit() || c == b'.' {
            at = number(bytes, at);
            Token::Number(parse_number(&text[start..at])?)
        } else if c == b'$' {
            at += 1;
            while at < bytes.len() && bytes[at].is_ascii_hexdigit() {
                at += 1;
            }
            let value = i64::from_str_radix(&text[start + 1..at], 16).map_err(|_| Invalid)?;
            Token::Number(value as f64)
        } else if c.is_ascii_lowercase() || c == b'_' {
            while at < bytes.len() && name(bytes[at]) {
                at += 1;
            }
            identifier(&text[start..at])?
        } else {
            at += 1;
            let next = bytes.get(at).copied();
            let pair = match c {
                b'<' | b'>' | b'=' => matches!(next, Some(b'<' | b'=' | b'>')),
                b'!' | b':' => next == Some(b'='),
                b'&' => next == Some(b'&'),
                b'|' => next == Some(b'|'),
                _ => false,
            };
            at += usize::from(pair);
            match &text[start..at] {
                "(" => Token::Open,
                ")" => Token::Close,
                "," => Token::Comma,
                symbol => lookup(symbol).ok_or(Invalid)?,
            }
        };
        out.push(token);
        if out.len() > MOST {
            return Err(Invalid);
        }
    }
    Ok(out)
}

/// The end of a number from `at`: `0x6cc720`.
fn number(bytes: &[u8], mut at: usize) -> usize {
    let digits = |bytes: &[u8], mut at: usize| {
        while at < bytes.len() && bytes[at].is_ascii_digit() {
            at += 1;
        }
        at
    };
    at = digits(bytes, at);
    if bytes.get(at) == Some(&b'.') {
        at = digits(bytes, at + 1);
    }
    if bytes.get(at) == Some(&b'e') {
        at += 1;
        if matches!(bytes.get(at), Some(b'+' | b'-')) {
            at += 1;
        }
        at = digits(bytes, at);
    }
    at
}

/// A number's text as Delphi's `StrToFloat` with `.` takes it.
fn parse_number(text: &str) -> Result<f64, Invalid> {
    let text = text.strip_suffix('.').unwrap_or(text);
    if text.is_empty() || text.ends_with(['e', '+', '-']) {
        return Err(Invalid);
    }
    text.parse().map_err(|_| Invalid)
}

fn identifier(name: &str) -> Result<Token, Invalid> {
    match name {
        "a" => Ok(Token::Var(0)),
        "b" => Ok(Token::Var(1)),
        "c" => Ok(Token::Var(2)),
        "pi" => Ok(Token::Number(std::f64::consts::PI)),
        _ => lookup(name).ok_or(Invalid),
    }
}

/// The checks and the folding of `0x6cd3d0`.
fn fold_signs(tokens: &mut Vec<Token>) -> Result<(), Invalid> {
    let mut depth = 0i32;
    for token in tokens.iter() {
        depth += match token {
            Token::Open => 1,
            Token::Close => -1,
            _ => 0,
        };
    }
    if depth != 0 {
        return Err(Invalid);
    }
    let sign = |t: Token| t.is("+") || t.is("-");
    let mut i = 0;
    while i < tokens.len() {
        let after_operator =
            i == 0 || matches!(tokens[i - 1], Token::Open | Token::Comma) || tokens[i - 1].binary();
        if sign(tokens[i]) && after_operator {
            let run = tokens[i..].iter().take_while(|t| sign(**t)).count();
            let minus = tokens[i..i + run].iter().filter(|t| t.is("-")).count();
            let unary = if minus % 2 == 1 { "-@" } else { "+@" };
            tokens.splice(i..i + run, [lookup(unary).ok_or(Invalid)?]);
        }
        let after_operator = i == 0
            || matches!(tokens[i - 1], Token::Open)
            || tokens[i - 1].entry().is_some_and(|e| e.operator);
        if tokens[i].is("not") && after_operator {
            let run = tokens[i..].iter().take_while(|t| t.is("not")).count();
            let keep = run % 2;
            tokens.drain(i + keep..i + run);
            if keep == 0 {
                continue;
            }
        }
        let next = tokens.get(i + 1).copied();
        match (tokens[i], next) {
            (a, Some(b)) if a.operand() && b.operand() => return Err(Invalid),
            (Token::Open, Some(Token::Close)) | (Token::Close, Some(Token::Open)) => {
                return Err(Invalid);
            }
            (Token::Close, Some(b)) if b.operand() => return Err(Invalid),
            (a, Some(Token::Open)) if a.operand() => return Err(Invalid),
            (a, Some(Token::Number(n))) if a.is("^") && n.fract() == 0.0 => {
                tokens[i] = lookup("^@").ok_or(Invalid)?;
            }
            _ => {}
        }
        i += 1;
    }
    Ok(())
}

/// A node of a compiled formula.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Node {
    Number(f64),
    Var(usize),
    Call {
        func: Func,
        /// The arguments, as places in the formula's nodes.
        args: [u16; 3],
        count: u8,
    },
}

/// A compiled formula.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Formula {
    nodes: Vec<Node>,
    root: usize,
    uses: Uses,
}

/// Where the outer brackets around all of `tokens` end, as `0x6ca3e0`
/// takes them off.
fn unwrap(mut tokens: &[Token]) -> &[Token] {
    while tokens.first() == Some(&Token::Open) && tokens.last() == Some(&Token::Close) {
        let mut depth = 0;
        let mut closes_at = None;
        for (at, token) in tokens.iter().enumerate() {
            match token {
                Token::Open => depth += 1,
                Token::Close => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                closes_at = Some(at);
                break;
            }
        }
        if closes_at != Some(tokens.len() - 1) {
            break;
        }
        tokens = &tokens[1..tokens.len() - 1];
    }
    tokens
}

/// The first operator or function from `from` at the outer level:
/// `0x6ca4e0`.
fn next_taker(tokens: &[Token], from: usize) -> usize {
    let mut depth = 0;
    for (at, token) in tokens.iter().enumerate().skip(from) {
        if depth < 1 && token.takes() {
            return at;
        }
        match token {
            Token::Open => depth += 1,
            Token::Close => depth -= 1,
            _ => {}
        }
    }
    tokens.len()
}

struct Builder {
    nodes: Vec<Node>,
    uses: Uses,
}

impl Builder {
    fn push(&mut self, node: Node) -> Result<usize, Invalid> {
        if self.nodes.len() >= MOST {
            return Err(Invalid);
        }
        self.nodes.push(node);
        Ok(self.nodes.len() - 1)
    }

    /// `0x6cc050` over a run of tokens.
    fn build(&mut self, tokens: &[Token]) -> Result<usize, Invalid> {
        let tokens = unwrap(tokens);
        match tokens {
            [] => return Err(Invalid),
            [Token::Number(n)] => return self.push(Node::Number(*n)),
            [Token::Var(v)] => return self.push(Node::Var(*v)),
            [token] if token.entry().is_some_and(|e| e.arity == 0) => {
                return self.call(token.entry().ok_or(Invalid)?, &[]);
            }
            _ => {}
        }
        let mut previous: Option<usize> = None;
        let mut start = 0;
        while start < tokens.len() {
            let at = next_taker(tokens, start);
            if at == tokens.len() {
                return Err(Invalid);
            }
            let entry = tokens[at].entry().ok_or(Invalid)?;
            let mut args: Vec<usize> = previous.into_iter().collect();
            let mut end;
            if entry.operator {
                if at > start {
                    args.push(self.build(&tokens[start..at])?);
                }
                end = at + 1;
            } else {
                // The bracket after the function, split at its commas.
                if tokens.get(at + 1) != Some(&Token::Open) {
                    return Err(Invalid);
                }
                let mut depth = 0;
                let mut from = at + 2;
                end = at + 1;
                for (i, token) in tokens.iter().enumerate().skip(at + 1) {
                    match token {
                        Token::Open => depth += 1,
                        Token::Close => depth -= 1,
                        Token::Comma if depth == 1 => {
                            args.push(self.build(&tokens[from..i])?);
                            from = i + 1;
                        }
                        _ => {}
                    }
                    if depth == 0 {
                        args.push(self.build(&tokens[from..i])?);
                        end = i + 1;
                        break;
                    }
                }
            }
            // What follows, up to the next operator that binds no tighter.
            let mut stop = end;
            loop {
                stop = next_taker(tokens, stop);
                if stop >= tokens.len() {
                    break;
                }
                if tokens[stop].entry().map_or(0, |e| e.precedence) >= entry.precedence {
                    break;
                }
                stop += 1;
            }
            if stop != end {
                args.push(self.build(&tokens[end..stop])?);
            }
            previous = Some(self.call(entry, &args)?);
            start = stop;
        }
        previous.ok_or(Invalid)
    }

    /// A call of `entry` with these arguments, which have to be as many as
    /// it takes (`0x6cbc20`).
    fn call(&mut self, entry: &Entry, args: &[usize]) -> Result<usize, Invalid> {
        if args.len() != usize::from(entry.arity) {
            return Err(Invalid);
        }
        match entry.func {
            Func::Rand => self.uses.rand = true,
            Func::Time | Func::Date | Func::MouseX | Func::MouseY => self.uses.outside = true,
            _ => {}
        }
        let mut places = [0u16; 3];
        for (place, arg) in places.iter_mut().zip(args) {
            *place = u16::try_from(*arg).map_err(|_| Invalid)?;
        }
        self.push(Node::Call {
            func: entry.func,
            args: places,
            count: entry.arity,
        })
    }
}

impl Formula {
    /// Compile a formula's text, as FL's `0x6cb780` does. An empty text
    /// has no formula: [`Formula::empty`].
    pub(crate) fn compile(text: &str) -> Result<Self, Invalid> {
        let mut text = text.to_ascii_lowercase();
        while let Some(at) = text.find("()") {
            text.replace_range(at..at + 2, "");
        }
        let mut list = tokens(&text)?;
        if list.is_empty() {
            return Ok(Formula::empty());
        }
        fold_signs(&mut list)?;
        let mut builder = Builder {
            nodes: Vec::new(),
            uses: Uses::default(),
        };
        let root = builder.build(&list)?;
        Ok(Formula {
            nodes: builder.nodes,
            root,
            uses: builder.uses,
        })
    }

    /// A formula of no text, which evaluates to FL's NaN constant.
    pub(crate) fn empty() -> Self {
        Formula {
            nodes: Vec::new(),
            root: 0,
            uses: Uses::default(),
        }
    }

    pub(crate) fn uses(&self) -> Uses {
        self.uses
    }

    /// The formula's value, or `None` where FL's evaluation raises
    /// (an integer division by zero). `seed` is the state of `Rand`.
    pub(crate) fn eval(&self, inputs: &Inputs, seed: &mut u32) -> Option<f64> {
        if self.nodes.is_empty() {
            // `PTR_DAT_00730170`: a NaN with every bit set.
            return Some(f64::from_bits(u64::MAX));
        }
        self.node(self.root, inputs, seed)
    }

    fn node(&self, at: usize, inputs: &Inputs, seed: &mut u32) -> Option<f64> {
        let (func, args, count) = match self.nodes[at] {
            Node::Number(n) => return Some(n),
            Node::Var(v) => return Some(inputs.vars[v]),
            Node::Call { func, args, count } => (func, args, count),
        };
        let mut x = [0.0; 3];
        for i in 0..usize::from(count) {
            x[i] = self.node(usize::from(args[i]), inputs, seed)?;
        }
        apply(func, x, inputs, seed)
    }
}

/// Delphi's `Round`: `cvtsd2si`, ties to even, and the integer
/// indefinite for what does not fit.
fn round(x: f64) -> i64 {
    let r = x.round_ties_even();
    if r.is_finite() && r.abs() < 9.223_372_036_854_776e18 {
        r as i64
    } else {
        i64::MIN
    }
}

/// Delphi's `Trunc`.
fn trunc(x: f64) -> i64 {
    let r = x.trunc();
    if r.is_finite() && r.abs() < 9.223_372_036_854_776e18 {
        r as i64
    } else {
        i64::MIN
    }
}

/// Delphi's `Int` (`0x4087a0`): the whole part, 0 under 1 and the number
/// itself from 2^52 up.
fn int(x: f64) -> f64 {
    let exponent = (x.to_bits() >> 52) & 0x7ff;
    if exponent >= 0x433 {
        x
    } else if exponent <= 0x3fe {
        0.0
    } else {
        (x as i64) as f64
    }
}

/// Delphi's `Frac` (`0x4087d0`).
fn frac(x: f64) -> f64 {
    let exponent = (x.to_bits() >> 52) & 0x7ff;
    if exponent >= 0x433 {
        0.0
    } else if exponent > 0x3fe {
        x - (x as i64) as f64
    } else {
        x
    }
}

/// Delphi's `IntPower`.
fn int_power(base: f64, exponent: i64) -> f64 {
    let mut y = exponent.unsigned_abs();
    let mut lbase = base;
    let mut result = 1.0;
    while y > 0 {
        while y & 1 == 0 {
            y >>= 1;
            lbase *= lbase;
        }
        y -= 1;
        result *= lbase;
    }
    if exponent < 0 { 1.0 / result } else { result }
}

/// Delphi's `Power`.
fn power(base: f64, exponent: f64) -> f64 {
    if exponent == 0.0 {
        1.0
    } else if base == 0.0 && exponent > 0.0 {
        0.0
    } else if exponent.fract() == 0.0 && exponent.abs() <= f64::from(i32::MAX) {
        int_power(base, exponent as i64)
    } else {
        (exponent * base.ln()).exp()
    }
}

/// `SeededRand` (`0x5b0460`): Numerical Recipes' `psdes` over the
/// double's two words, as a single from 0 to 1.
fn seeded(x: f64) -> f64 {
    const C1: [u32; 4] = [0xbaa9_6887, 0x1e17_d32c, 0x03bc_dc3c, 0x0f33_d1b2];
    const C2: [u32; 4] = [0x4b0f_3b58, 0xe874_f0c3, 0x6955_c5a6, 0x55a7_ca46];
    let bits = x.to_bits();
    let (mut low, mut high) = (bits as u32, (bits >> 32) as u32);
    for i in 0..4 {
        let old = low;
        let mixed = low ^ C1[i];
        let (lo, hi) = (mixed & 0xffff, mixed >> 16);
        let square = lo.wrapping_mul(lo).wrapping_add(!hi.wrapping_mul(hi));
        low = high ^ (square.rotate_left(16) ^ C2[i]).wrapping_add(lo.wrapping_mul(hi));
        high = old;
    }
    f64::from(f32::from_bits(low & 0x7f_ffff | 0x3f80_0000) - 1.0)
}

/// FL's `Tension(x, t)` (`0x6d17b0`), in its mix of singles and doubles.
fn tension(x: f64, t: f64) -> f64 {
    if t == 0.0 {
        return x;
    }
    if t < 0.0 {
        let neg = -t;
        let d = (f64::from(neg as f32) + 1.0).ln();
        let e = (f64::from(x as f32) * d).exp();
        f64::from((e - 1.0) as f32) / neg
    } else {
        let d = (f64::from(t as f32) + 1.0).ln();
        let e = (f64::from((1.0 - x) as f32) * d).exp();
        (t - f64::from((e - 1.0) as f32)) / t
    }
}

/// `fold` (`0x6cee80`): a triangle that folds the line back on itself.
fn fold(x: f64) -> f64 {
    let d3 = x * 2.0 - 1.0;
    let mut d4 = d3.abs() * 0.25 / 1.0;
    d4 -= int(d4);
    d4 = if d4 < 0.25 {
        d4 * 4.0
    } else if d4 < 0.75 {
        1.0 - (d4 - 0.25) * 4.0
    } else {
        (d4 - 0.75) * 4.0 - 1.0
    };
    if d3 < 0.0 {
        d4 = -d4;
    }
    d4 / 2.0 + 0.5
}

/// Delphi's `FMod`, `x - Trunc(x / y) * y`.
fn fmod(x: f64, y: f64) -> f64 {
    x - trunc(x / y) as f64 * y
}

fn truth(b: bool) -> f64 {
    if b { 1.0 } else { 0.0 }
}

fn apply(func: Func, x: [f64; 3], inputs: &Inputs, seed: &mut u32) -> Option<f64> {
    let [a, b, c] = x;
    let epsilon = 1e-30;
    let value = match func {
        Func::Factorial => {
            let mut n = round(a) as f64;
            let mut result = 1.0;
            while n > 1.1 {
                result *= n;
                n -= 1.0;
            }
            result
        }
        Func::Percent => a * 0.01,
        Func::Negate | Func::Neg => f64::from_bits(a.to_bits() ^ (1 << 63)),
        Func::Plus => a,
        Func::Power => power(a, b),
        Func::IntPower => int_power(a, round(b) as i32 as i64),
        Func::Multiply => a * b,
        Func::Divide => a / b,
        Func::Div => round(a).checked_div(round(b))? as f64,
        Func::Mod => round(a).checked_rem(round(b))? as f64,
        Func::FMod => {
            if b == 0.0 {
                0.0
            } else {
                fmod(a, b)
            }
        }
        Func::Add | Func::Sum => a + b,
        Func::Subtract => a - b,
        Func::Greater => truth(b <= a && a != b),
        Func::GreaterEqual => truth(b <= a + epsilon),
        Func::LessEqual => truth(a <= b + epsilon),
        Func::Less => truth(a <= b && b != a),
        Func::NotEqual => truth(epsilon < (a - b).abs()),
        Func::Equal => truth((a - b).abs() < epsilon),
        Func::Not => truth(round(a) as u8 == 0),
        Func::Or => (round(a) | round(b)) as f64,
        Func::And => (round(a) & round(b)) as f64,
        Func::Xor => (round(a) ^ round(b)) as f64,
        Func::Shr => ((round(a) as u64) >> (round(b) as u8 & 63)) as i64 as f64,
        Func::Shl => (round(a) << (round(b) as u8 & 63)) as f64,
        Func::Exp => a.exp(),
        Func::If => {
            if round(a) as u8 != 0 {
                b
            } else {
                c
            }
        }
        Func::Sqr => a * a,
        Func::Sqrt => a.sqrt(),
        Func::Abs => a.abs(),
        Func::Round => round(a) as f64,
        Func::Trunc => trunc(a) as f64,
        Func::Floor => {
            let whole = trunc(a) as i32;
            f64::from(if frac(a) < 0.0 { whole - 1 } else { whole })
        }
        Func::Ceil => {
            let whole = trunc(a) as i32;
            f64::from(if frac(a) > 0.0 { whole + 1 } else { whole })
        }
        Func::Ln => a.ln(),
        Func::Log10 => a.log10(),
        Func::LogN => b.ln() / a.ln(),
        Func::Log2 => a.log2(),
        Func::Max => {
            if b <= a {
                a
            } else {
                b
            }
        }
        Func::Min => {
            if a <= b {
                a
            } else {
                b
            }
        }
        Func::Sin => a.sin(),
        Func::Cos => a.cos(),
        Func::Tan => a.tan(),
        Func::Cotan => 1.0 / a.tan(),
        Func::ArcSin => a.asin(),
        Func::ArcCos => a.acos(),
        Func::ArcTan => a.atan(),
        Func::ArcTan2 => a.atan2(b),
        Func::Sinh => a.sinh(),
        Func::Cosh => a.cosh(),
        Func::Tanh => a.tanh(),
        Func::ArcSinh => a.asinh(),
        Func::ArcCosh => a.acosh(),
        Func::ArcTanh => a.atanh(),
        Func::DegToRad => a * 0.017_453_292_519_943_295,
        Func::RadToDeg => a * 57.295_779_513_082_32,
        Func::Inter => {
            let (x, lo, hi) = (round(a * 1000.0), round(b * 1000.0), round(c * 1000.0));
            truth(if lo < hi {
                lo <= x && x <= hi
            } else {
                hi <= x && x <= lo
            })
        }
        Func::InterNoSwap => {
            let (x, lo, hi) = (round(a * 1000.0), round(b * 1000.0), round(c * 1000.0));
            truth(lo <= x && x <= hi)
        }
        Func::Fold => fold(a),
        Func::Warp => {
            let d = if !(0.0..=1.0).contains(&a) { a + b } else { a };
            fmod(d, 1.0)
        }
        Func::Map => a * (c - b) + b,
        Func::Rand => {
            // Delphi's `Random`: `RandSeed * $08088405 + 1`.
            *seed = seed.wrapping_mul(0x0808_8405).wrapping_add(1);
            f64::from(*seed) * (1.0 / 4_294_967_296.0)
        }
        Func::SeededRand => seeded(a),
        Func::Int => int(a),
        Func::Frac => frac(a),
        Func::IfE => truth(a == b),
        Func::IfL => truth(a < b),
        Func::IfG => truth(a > b),
        Func::IfLE => truth(a <= b),
        Func::IfGE => truth(a >= b),
        Func::Case => {
            if a == 1.0 {
                b
            } else {
                c
            }
        }
        Func::Sec => {
            let cos = a.cos();
            if cos == 0.0 { 1.0 } else { 1.0 / cos }
        }
        Func::CoSec => {
            let sin = a.sin();
            if sin == 0.0 { 1.0 } else { 1.0 / sin }
        }
        Func::SongTime => inputs.song_time,
        Func::GetTempo => inputs.tempo,
        Func::Time => inputs.time,
        Func::Date | Func::MouseX | Func::MouseY => 0.0,
        Func::Tension => tension(a, b),
    };
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(text: &str, vars: [f64; 3]) -> f64 {
        let formula = Formula::compile(text).unwrap_or_else(|_| panic!("{text}"));
        let inputs = Inputs {
            vars,
            ..Inputs::default()
        };
        formula.eval(&inputs, &mut 0).unwrap()
    }

    #[test]
    fn precedence_and_associativity_are_fl_s() {
        assert_eq!(eval("1+2*3", [0.0; 3]), 7.0);
        assert_eq!(eval("(1+2)*3", [0.0; 3]), 9.0);
        assert_eq!(eval("8-4-2", [0.0; 3]), 2.0);
        assert_eq!(eval("2^3^2", [0.0; 3]), 64.0);
        // The sign binds tighter than the power.
        assert_eq!(eval("-2^2", [0.0; 3]), 4.0);
        assert_eq!(eval("2*-3", [0.0; 3]), -6.0);
        assert_eq!(eval("1--1", [0.0; 3]), 2.0);
        assert_eq!(eval("1-+-1", [0.0; 3]), 2.0);
        assert_eq!(eval("1+2>2", [0.0; 3]), 1.0);
        assert_eq!(eval("1>0 and 2>1", [0.0; 3]), 1.0);
        assert_eq!(eval("not not 3", [0.0; 3]), 3.0);
        assert_eq!(eval("not 0", [0.0; 3]), 1.0);
        assert_eq!(eval("5!", [0.0; 3]), 120.0);
        assert_eq!(eval("50%", [0.0; 3]), 0.5);
        assert_eq!(eval("7 div 2", [0.0; 3]), 3.0);
        assert_eq!(eval("7 mod 4", [0.0; 3]), 3.0);
    }

    #[test]
    fn names_are_read_in_any_case_and_brackets_come_off() {
        assert_eq!(eval("A*B+C", [0.5, 0.5, 0.25]), 0.5);
        assert_eq!(eval("Sin(0)+Max(a, b)", [0.25, 0.75, 0.0]), 0.75);
        assert_eq!(eval("((a))", [0.3, 0.0, 0.0]), 0.3);
        assert_eq!(eval("if(a > 0.5, 1, 0)", [0.7, 0.0, 0.0]), 1.0);
        assert_eq!(
            eval("map(a, 0.2, 0.4)", [0.5, 0.0, 0.0]),
            0.30000000000000004
        );
        assert_eq!(eval("Tension(a, 0)", [0.3, 0.0, 0.0]), 0.3);
        assert_eq!(eval("pi", [0.0; 3]), std::f64::consts::PI);
        assert_eq!(eval("1.5e1 + .5", [0.0; 3]), 15.5);
        assert_eq!(eval("$ff", [0.0; 3]), 255.0);
    }

    #[test]
    fn what_fl_turns_down_does_not_compile() {
        for text in [
            "1 2",
            "a(",
            "(a))",
            "foo",
            "sin(1, 2)",
            "*5",
            "a)(b",
            "1 (2)",
            "a & b",
        ] {
            assert!(Formula::compile(text).is_err(), "{text}");
        }
    }

    #[test]
    fn an_empty_formula_is_fl_s_nan() {
        let formula = Formula::compile("  ").unwrap();
        let value = formula.eval(&Inputs::default(), &mut 0).unwrap();
        assert_eq!(value.to_bits(), u64::MAX);
    }

    #[test]
    fn tension_bends_as_fl_s() {
        // 1 - ((1 + t)^(1 - x) - 1) / t for t > 0 and ((1 - t)^x - 1) / -t
        // for t < 0, in singles where FL has them.
        let up = tension(0.5, 4.0);
        assert!((up - (4.0 - (5f64.sqrt() - 1.0)) / 4.0).abs() < 1e-6);
        let down = tension(0.5, -4.0);
        assert!((down - (5f64.sqrt() - 1.0) / 4.0).abs() < 1e-6);
        assert_eq!(tension(0.0, 3.0), 0.0);
        assert!((tension(1.0, 3.0) - 1.0).abs() < 1e-7);
    }

    #[test]
    fn seeded_rand_is_psdes() {
        // Numerical Recipes' check values: psdes(1, 1) leaves 0x604d1dce
        // in the left word and 0x509c0c23 in the right, which FL takes.
        let value = seeded(f64::from_bits((1u64 << 32) | 1));
        let right = 0x509c_0c23u32;
        assert_eq!(
            value,
            f64::from(f32::from_bits(right & 0x7f_ffff | 0x3f80_0000) - 1.0)
        );
        assert!((0.0..1.0).contains(&seeded(0.25)));
    }

    #[test]
    fn fold_and_warp_stay_inside_0_to_1() {
        assert_eq!(fold(0.5), 0.5);
        assert_eq!(fold(1.0), 1.0);
        assert_eq!(fold(0.0), 0.0);
        assert_eq!(fold(1.5), 0.5);
        assert_eq!(fold(2.0), 0.0);
        assert!((eval("warp(1.25, 0)", [0.0; 3]) - 0.25).abs() < 1e-12);
    }

    #[test]
    fn integer_division_by_zero_gives_no_value() {
        let formula = Formula::compile("1 div a").unwrap();
        assert_eq!(formula.eval(&Inputs::default(), &mut 0), None);
    }
}
