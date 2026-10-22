//! The formula of a controller link, event 230: what FL runs a controller's
//! value through on its way to the control the link drives.
//!
//! `docs/plugins/fruity-peak-controller.md`, section 4, has the frame: the
//! value comes in as `Input`, from 0 to 1, and what the formula makes of it
//! is kept inside 0 to 1 and scaled onto the control's range.
//!
//! This reads the arithmetic, numbers, `Input` in any case, `+`, `-`, `*`,
//! `/`, a sign in front and brackets, and the one function FL's formula
//! compiler (`FUN_00f81f70`) adds to its parser, `Tension(x, t)`
//! (`FUN_00f81d60`, [`tension`]). That is every formula on a controller
//! link in FL's own projects: `1-Input`, `Input*1`, `Input*0.6`,
//! `1-Input*1.6` and `1-Input*0.5-0.5` on the Peak Controller links, and
//! `Tension(Input,10)` and `Tension(1-Input,10)` in `Surround mix.flp`. A
//! formula with anything else in it does not compile, and its link is one
//! the engine does not play.

/// A formula, ready to work out. [`Formula::compile`] allocates and
/// [`Formula::at`] does not.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Formula {
    /// The formula in reverse Polish order.
    steps: Vec<Step>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Step {
    Number(f64),
    Input,
    Add,
    Subtract,
    Multiply,
    Divide,
    Negate,
    /// [`tension`] of the two values on top, `x` below `t`.
    Tension,
}

/// FL's `Tension(x, t)` (`FUN_00f81d60`): `x` bent by `t`. 0 is the
/// straight line, a `t` below 0 bends it down as `((1 + |t|)^x - 1) / |t|`
/// and one above 0 up, its mirror image. Both go through 0 at 0 and
/// through 1 at 1. FL works the power out in f32, as `exp(x * ln(t + 1))`,
/// and so does this.
pub(crate) fn tension(x: f64, t: f64) -> f64 {
    let bent = |x: f64, size: f64| {
        let ln = f64::from(size as f32 + 1.0).ln();
        f64::from(((f64::from(x as f32) * ln).exp() - 1.0) as f32)
    };
    if t == 0.0 {
        x
    } else if t < 0.0 {
        bent(x, -t) / -t
    } else {
        (t - bent(1.0 - x, t)) / t
    }
}

/// The most values a formula has on its stack at once. One that needs more
/// does not compile.
const DEPTH: usize = 16;

/// The text of a formula on its way to steps.
struct Parser<'a> {
    rest: &'a [u8],
    steps: Vec<Step>,
    /// The values on the stack once the steps so far have run.
    depth: usize,
    /// The signs and brackets that are open, which a formula cannot nest
    /// without end.
    open: usize,
}

impl Parser<'_> {
    /// The next byte that is no space.
    fn peek(&mut self) -> Option<u8> {
        while let [b' ' | b'\t', rest @ ..] = self.rest {
            self.rest = rest;
        }
        self.rest.first().copied()
    }

    /// Take the next byte when it is `byte`.
    fn take(&mut self, byte: u8) -> bool {
        let found = self.peek() == Some(byte);
        if found {
            self.rest = &self.rest[1..];
        }
        found
    }

    /// Add a step that leaves one value more on the stack.
    fn push(&mut self, step: Step) -> Option<()> {
        self.depth += 1;
        self.steps.push(step);
        (self.depth <= DEPTH).then_some(())
    }

    /// Add a step that takes two values and leaves one.
    fn join(&mut self, step: Step) {
        self.depth -= 1;
        self.steps.push(step);
    }

    /// Terms with `+` and `-` between them.
    fn sum(&mut self) -> Option<()> {
        self.product()?;
        loop {
            let step = match self.peek() {
                Some(b'+') => Step::Add,
                Some(b'-') => Step::Subtract,
                _ => return Some(()),
            };
            self.rest = &self.rest[1..];
            self.product()?;
            self.join(step);
        }
    }

    /// Factors with `*` and `/` between them.
    fn product(&mut self) -> Option<()> {
        self.factor()?;
        loop {
            let step = match self.peek() {
                Some(b'*') => Step::Multiply,
                Some(b'/') => Step::Divide,
                _ => return Some(()),
            };
            self.rest = &self.rest[1..];
            self.factor()?;
            self.join(step);
        }
    }

    /// Read what is inside a sign or a pair of brackets, unless that is
    /// one level too deep.
    fn nested(&mut self, inner: fn(&mut Self) -> Option<()>) -> Option<()> {
        self.open += 1;
        (self.open <= DEPTH).then_some(())?;
        inner(self)?;
        self.open -= 1;
        Some(())
    }

    /// A number, `Input` or a sum in brackets, with any signs in front.
    fn factor(&mut self) -> Option<()> {
        if self.take(b'-') {
            self.nested(Self::factor)?;
            self.steps.push(Step::Negate);
            return Some(());
        }
        if self.take(b'+') {
            return self.nested(Self::factor);
        }
        if self.take(b'(') {
            self.nested(Self::sum)?;
            return self.take(b')').then_some(());
        }
        let word = |byte: &u8| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_');
        let len = self.rest.iter().take_while(|byte| word(byte)).count();
        let (word, rest) = self.rest.split_at(len);
        self.rest = rest;
        let word = std::str::from_utf8(word).ok()?;
        if word.eq_ignore_ascii_case("input") {
            return self.push(Step::Input);
        }
        if word.eq_ignore_ascii_case("tension") {
            return self.tension();
        }
        // Digits with at most one point among them. `parse` alone would
        // take `inf`, `nan` and `1e5`.
        let plain = word
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.');
        let number: f64 = word.parse().ok().filter(|_| plain)?;
        self.push(Step::Number(number))
    }
}

impl Parser<'_> {
    /// The brackets of `Tension(x, t)`, after its name.
    fn tension(&mut self) -> Option<()> {
        self.take(b'(').then_some(())?;
        self.nested(Self::sum)?;
        self.take(b',').then_some(())?;
        self.nested(Self::sum)?;
        self.take(b')').then_some(())?;
        self.join(Step::Tension);
        Some(())
    }
}

impl Formula {
    /// The formula a link's 230 holds. `None` for one that has something
    /// in it this does not read.
    pub fn compile(text: &str) -> Option<Self> {
        let mut parser = Parser {
            rest: text.as_bytes(),
            steps: Vec::new(),
            depth: 0,
            open: 0,
        };
        parser.sum()?;
        // Nothing may follow the sum.
        parser.peek().is_none().then_some(Formula {
            steps: parser.steps,
        })
    }

    /// What the formula makes of `input`, kept inside 0 to 1. A formula
    /// that comes to no number, as `0 / 0` does, gives 0.
    pub fn at(&self, input: f64) -> f64 {
        let mut stack = [0.0; DEPTH];
        let mut depth = 0;
        for step in &self.steps {
            match *step {
                Step::Number(number) => {
                    stack[depth] = number;
                    depth += 1;
                }
                Step::Input => {
                    stack[depth] = input;
                    depth += 1;
                }
                Step::Negate => stack[depth - 1] = -stack[depth - 1],
                join => {
                    depth -= 1;
                    let (left, right) = (stack[depth - 1], stack[depth]);
                    stack[depth - 1] = match join {
                        Step::Add => left + right,
                        Step::Subtract => left - right,
                        Step::Multiply => left * right,
                        Step::Tension => tension(left, right),
                        _ => left / right,
                    };
                }
            }
        }
        let value = stack[0];
        if value.is_nan() {
            0.0
        } else {
            value.clamp(0.0, 1.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str, input: f64) -> f64 {
        Formula::compile(text).unwrap().at(input)
    }

    fn close(got: f64, want: f64) -> bool {
        (got - want).abs() < 1e-12
    }

    #[test]
    fn the_formulas_of_fl_s_projects_work_out() {
        assert!(close(at("1-Input", 0.25), 0.75));
        assert!(close(at("Input*1", 0.25), 0.25));
        assert!(close(at("Input*0.6", 0.5), 0.3));
        assert!(close(at("1-Input*1.6", 0.5), 0.2));
        // Half the input, upside down, under the middle.
        assert!(close(at("1-Input*0.5-0.5", 0.0), 0.5));
        assert!(close(at("1-Input*0.5-0.5", 0.5), 0.25));
        assert!(close(at("1-Input*0.5-0.5", 1.0), 0.0));
    }

    #[test]
    fn the_result_stays_inside_0_and_1() {
        assert_eq!(at("1-Input*1.6", 1.0), 0.0);
        assert_eq!(at("Input*3", 0.5), 1.0);
        assert_eq!(at("Input+1", 0.5), 1.0);
        assert_eq!(at("-Input", 0.5), 0.0);
        // Dividing by nothing is the top, the bottom, or no number at all.
        assert_eq!(at("1/Input", 0.0), 1.0);
        assert_eq!(at("-1/Input", 0.0), 0.0);
        assert_eq!(at("Input/Input", 0.0), 0.0);
    }

    #[test]
    fn products_come_before_sums_and_brackets_before_both() {
        assert!(close(at("0.1+0.2*Input", 0.5), 0.2));
        assert!(close(at("(0.1+0.2)*Input", 0.5), 0.15));
        assert!(close(at("1-0.25-0.25", 0.0), 0.5));
        assert!(close(at("1/2/2", 0.0), 0.25));
        assert!(close(at("1-(0.25-0.125)", 0.0), 0.875));
        assert!(close(at("0.5--Input", 0.25), 0.75));
        assert!(close(at("-(Input-1)", 0.25), 0.75));
        assert!(close(at("+.5", 0.0), 0.5));
        // Spaces and the case of `Input` do not count.
        assert!(close(at(" 1 - INPUT\t* 0.5 ", 0.5), 0.75));
        assert!(close(at("input", 0.3), 0.3));
    }

    #[test]
    fn tension_bends_the_input_as_fl_does() {
        // The Surround mix formulas: 0 and 1 stay where they are.
        for text in ["Tension(Input,10)", "Tension(1-Input,10)"] {
            assert!(close(at(text, 0.0) + at(text, 1.0), 1.0), "{text}");
        }
        // Up: (10 - (11^(1 - x) - 1)) / 10, so 0.5 gives 1 - (sqrt 11 - 1) / 10.
        let up = 1.0 - (11f64.sqrt() - 1.0) / 10.0;
        assert!((at("Tension(Input,10)", 0.5) - up).abs() < 1e-6);
        assert!((at("Tension(1-Input,10)", 0.5) - up).abs() < 1e-6);
        // Down is its mirror image, and 0 the line.
        let down = (11f64.sqrt() - 1.0) / 10.0;
        assert!((at("Tension(Input, -10)", 0.5) - down).abs() < 1e-6);
        assert!(close(at("tension(Input, 0)", 0.3), 0.3));
        assert!(close(at("Tension((Input), 1-1)*0.5", 0.5), 0.25));
    }

    #[test]
    fn a_formula_with_anything_else_in_it_does_not_compile() {
        for text in [
            "",
            " ",
            "1-",
            "*Input",
            "(1-Input",
            "1-Input)",
            "Input Input",
            "1 2",
            "Tension(Input)",
            "Tension(Input, 0.5",
            "Tension Input",
            "Sin(Input)",
            "Input^2",
            "Inputs",
            "1e5",
            "inf",
            "nan",
            "1.2.3",
            "0x10",
            "1,5",
            "Input*é",
        ] {
            assert_eq!(Formula::compile(text), None, "{text:?}");
        }
    }

    #[test]
    fn a_formula_too_deep_for_the_stack_does_not_compile() {
        // Sixteen values wait for the brackets to close, and a
        // seventeenth is one too many.
        let nested = |values: usize| {
            let open = "1+(".repeat(values - 1);
            format!("{open}Input{}", ")".repeat(values - 1))
        };
        assert!(close(at(&nested(8), 0.0), 1.0));
        assert!(Formula::compile(&nested(DEPTH)).is_some());
        assert_eq!(Formula::compile(&nested(DEPTH + 1)), None);
        // A long sum needs two, however long it is.
        let long = vec!["0.01"; 90].join("+");
        assert!(close(at(&long, 0.0), 0.9));
        // Brackets and signs alone cannot run away either.
        let deep = format!("{}1{}", "(".repeat(40), ")".repeat(40));
        assert_eq!(Formula::compile(&deep), None);
        assert_eq!(Formula::compile(&"-".repeat(40)), None);
        assert!(Formula::compile(&format!("{}1", "-".repeat(DEPTH))).is_some());
    }
}
