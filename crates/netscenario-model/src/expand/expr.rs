//! The expressions of a scenario file (D-093): numbers, quantities with the units of the format, parameter names, `+ - * /`,
//! unary minus and parentheses. The arithmetic is exact (fractions of 128-bit integers), so `0.1 + 0.2` is `0.3` and a result
//! that is not a whole number of the smallest unit is an error, as the same value written out would be.

use crate::suggest::closest;
use crate::units::{self, Dimension};

/// An expression may be this long.
const MAX_EXPRESSION: usize = 256;

/// Parentheses and unary minuses may be nested this deep.
const MAX_DEPTH: usize = 32;

/// Digits after the point that a result without a unit is written with.
const MAX_FRACTION_DIGITS: usize = 12;

/// A fraction in lowest terms with a positive denominator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ratio {
    numerator: i128,
    denominator: i128,
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

impl Ratio {
    fn new(numerator: i128, denominator: i128) -> Option<Self> {
        if denominator == 0 {
            return None;
        }
        let divisor =
            i128::try_from(gcd(numerator.unsigned_abs(), denominator.unsigned_abs())).ok()?;
        let divisor = if denominator < 0 { -divisor } else { divisor };
        Some(Self {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
        })
    }

    fn from_unsigned(ratio: (u128, u128)) -> Option<Self> {
        Self::new(i128::try_from(ratio.0).ok()?, i128::try_from(ratio.1).ok()?)
    }

    fn add(self, other: Self) -> Option<Self> {
        Self::new(
            self.numerator
                .checked_mul(other.denominator)?
                .checked_add(other.numerator.checked_mul(self.denominator)?)?,
            self.denominator.checked_mul(other.denominator)?,
        )
    }

    fn sub(self, other: Self) -> Option<Self> {
        self.add(other.neg()?)
    }

    fn mul(self, other: Self) -> Option<Self> {
        Self::new(
            self.numerator.checked_mul(other.numerator)?,
            self.denominator.checked_mul(other.denominator)?,
        )
    }

    /// `None` if `other` is zero or the result does not fit.
    fn div(self, other: Self) -> Option<Self> {
        Self::new(
            self.numerator.checked_mul(other.denominator)?,
            self.denominator.checked_mul(other.numerator)?,
        )
    }

    fn neg(self) -> Option<Self> {
        Some(Self {
            numerator: self.numerator.checked_neg()?,
            denominator: self.denominator,
        })
    }

    fn is_zero(self) -> bool {
        self.numerator == 0
    }

    /// The exact text of a number that has a finite decimal expansion of at most [`MAX_FRACTION_DIGITS`] digits.
    fn decimal(self) -> Option<String> {
        let sign = if self.numerator < 0 { "-" } else { "" };
        let number = self.numerator.unsigned_abs();
        let denominator = self.denominator.unsigned_abs();
        let mut text = format!("{sign}{}", number / denominator);
        let mut rest = number % denominator;
        if rest == 0 {
            return Some(text);
        }
        text.push('.');
        for _ in 0..MAX_FRACTION_DIGITS {
            rest = rest.checked_mul(10)?;
            text.push(char::from(b'0' + u8::try_from(rest / denominator).ok()?));
            rest %= denominator;
            if rest == 0 {
                return Some(text);
            }
        }
        None
    }
}

/// What a value is, for the check that a given value has the kind of the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Number,
    Quantity(Dimension),
    Bool,
    Text,
}

impl Kind {
    pub(crate) fn what(self) -> &'static str {
        match self {
            Self::Number => "a number",
            Self::Quantity(Dimension::Duration) => "a duration",
            Self::Quantity(Dimension::Rate) => "a rate",
            Self::Quantity(Dimension::Size) => "a size",
            Self::Quantity(Dimension::Percent) => "a percentage",
            Self::Bool => "true or false",
            Self::Text => "text",
        }
    }
}

/// A value of an expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Value {
    Number(Ratio),
    /// A quantity in the smallest unit of its kind (nanoseconds, bits per second, bytes, millionths of a percent).
    Quantity(Dimension, Ratio),
    Bool(bool),
    Text(String),
}

impl Value {
    pub(crate) fn kind(&self) -> Kind {
        match self {
            Self::Number(_) => Kind::Number,
            Self::Quantity(dimension, _) => Kind::Quantity(*dimension),
            Self::Bool(_) => Kind::Bool,
            Self::Text(_) => Kind::Text,
        }
    }
}

/// A scalar as the file writes it: the text, and whether it was in quotes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Written {
    pub(crate) text: String,
    pub(crate) quoted: bool,
}

/// A value with the scalar that stands for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Binding {
    pub(crate) value: Value,
    pub(crate) written: Written,
}

impl Binding {
    /// The value of a scalar of the file. A text with a unit is a quantity whether it is quoted or not (every value with a unit
    /// is a string in this format); a number and `true` or `false` only when they are not quoted.
    pub(crate) fn from_scalar(text: &str, quoted: bool) -> Self {
        let value = if let Some(quantity) = quantity(text) {
            quantity
        } else if quoted {
            Value::Text(text.to_owned())
        } else if let Some(number) = number(text) {
            Value::Number(number)
        } else if text == "true" || text == "false" {
            Value::Bool(text == "true")
        } else {
            Value::Text(text.to_owned())
        };
        Self {
            value,
            written: Written {
                text: text.to_owned(),
                quoted,
            },
        }
    }

    fn computed(value: Value) -> Result<Self, ExprError> {
        let written = render(&value)?;
        Ok(Self { value, written })
    }
}

fn quantity(text: &str) -> Option<Value> {
    let (dimension, ratio) = units::read_quantity(text)?;
    Some(Value::Quantity(dimension, Ratio::from_unsigned(ratio)?))
}

/// A number as YAML writes it in the plain form that matters here: digits with an optional sign and fraction.
fn number(text: &str) -> Option<Ratio> {
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    let is_digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
    if whole.is_empty()
        || !is_digits(whole)
        || !is_digits(fraction)
        || (digits.contains('.') && fraction.is_empty())
        || whole.len() + fraction.len() > 30
    {
        return None;
    }
    let all: i128 = format!("{whole}{fraction}").parse().ok()?;
    let scale = 10i128.checked_pow(u32::try_from(fraction.len()).ok()?)?;
    Ratio::new(if negative { -all } else { all }, scale)
}

/// Why an expression has no value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExprError {
    pub(crate) message: String,
    pub(crate) help: Option<String>,
    /// The expression uses a parameter that has no value because of a problem that is reported already: nothing more to say.
    pub(crate) silent: bool,
}

fn fail<T>(message: impl Into<String>) -> Result<T, ExprError> {
    Err(ExprError {
        message: message.into(),
        help: None,
        silent: false,
    })
}

fn fail_with<T>(message: impl Into<String>, help: impl Into<String>) -> Result<T, ExprError> {
    Err(ExprError {
        message: message.into(),
        help: Some(help.into()),
        silent: false,
    })
}

const TOO_LARGE: &str = "the result is too large";

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(Value, String),
    Name(String),
    Plus,
    Minus,
    Star,
    Slash,
    Open,
    Close,
}

const UNITS_HELP: &str = "the units are ns, us, ms, s, m, h; bit, kbit, mbit, gbit, tbit; b, kb, mb, gb, kib, mib, gib; and %";

fn tokens(source: &str) -> Result<Vec<Token>, ExprError> {
    let characters: Vec<char> = source.chars().collect();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(&c) = characters.get(at) {
        match c {
            ' ' | '\t' => at += 1,
            '+' | '-' | '*' | '/' | '(' | ')' => {
                out.push(match c {
                    '+' => Token::Plus,
                    '-' => Token::Minus,
                    '*' => Token::Star,
                    '/' => Token::Slash,
                    '(' => Token::Open,
                    _ => Token::Close,
                });
                at += 1;
            }
            '0'..='9' => {
                let start = at;
                while characters.get(at).is_some_and(char::is_ascii_digit) {
                    at += 1;
                }
                if characters.get(at) == Some(&'.') {
                    at += 1;
                    let fraction = at;
                    while characters.get(at).is_some_and(char::is_ascii_digit) {
                        at += 1;
                    }
                    if at == fraction {
                        return fail("a number needs digits after the point");
                    }
                }
                let number_end = at;
                while characters
                    .get(at)
                    .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '%')
                {
                    at += 1;
                }
                let text: String = characters[start..at].iter().collect();
                if number_end == at {
                    match number(&text) {
                        Some(number) => out.push(Token::Number(Value::Number(number), text)),
                        None => return fail(format!("the number {text} is too long")),
                    }
                } else {
                    let Some(value) = quantity(&text) else {
                        let unit: String = characters[number_end..at].iter().collect();
                        return fail_with(
                            format!("'{unit}' is not a unit of the format"),
                            UNITS_HELP,
                        );
                    };
                    out.push(Token::Number(value, text));
                }
            }
            'a'..='z' => {
                let start = at;
                while characters
                    .get(at)
                    .is_some_and(|c| matches!(c, 'a'..='z' | '0'..='9' | '_'))
                {
                    at += 1;
                }
                out.push(Token::Name(characters[start..at].iter().collect()));
            }
            other => {
                return fail_with(
                    format!("unexpected '{other}' in the expression"),
                    "an expression has numbers with units, parameter names (lower case letters, digits and _), + - * / and parentheses",
                );
            }
        }
    }
    Ok(out)
}

/// What a name stands for in an expression.
pub(crate) trait Scope {
    fn lookup(&self, name: &str) -> Option<&Binding>;
    fn names(&self) -> Vec<&str>;
    /// Whether `name` is a parameter that has no value because its binding failed.
    fn is_unbound(&self, name: &str) -> bool {
        let _ = name;
        false
    }
}

struct Parser<'a> {
    tokens: Vec<Token>,
    at: usize,
    scope: &'a dyn Scope,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn expression(&mut self, depth: usize) -> Result<Value, ExprError> {
        let mut left = self.term(depth)?;
        while let Some(operator @ (Token::Plus | Token::Minus)) = self.peek().cloned() {
            self.at += 1;
            let right = self.term(depth)?;
            left = add_or_subtract(&left, &right, operator == Token::Minus)?;
        }
        Ok(left)
    }

    fn term(&mut self, depth: usize) -> Result<Value, ExprError> {
        let mut left = self.unary(depth)?;
        while let Some(operator @ (Token::Star | Token::Slash)) = self.peek().cloned() {
            self.at += 1;
            let right = self.unary(depth)?;
            left = if operator == Token::Star {
                multiply(&left, &right)?
            } else {
                divide(&left, &right)?
            };
        }
        Ok(left)
    }

    fn unary(&mut self, depth: usize) -> Result<Value, ExprError> {
        if depth > MAX_DEPTH {
            return fail(format!(
                "the expression is nested more than {MAX_DEPTH} levels"
            ));
        }
        if self.peek() == Some(&Token::Minus) {
            self.at += 1;
            return match self.unary(depth + 1)? {
                Value::Number(x) => Ok(Value::Number(x.neg().ok_or_else(too_large)?)),
                Value::Quantity(d, x) => Ok(Value::Quantity(d, x.neg().ok_or_else(too_large)?)),
                other => fail(format!("cannot negate {}", describe(&other))),
            };
        }
        self.primary(depth)
    }

    fn primary(&mut self, depth: usize) -> Result<Value, ExprError> {
        let Some(token) = self.peek().cloned() else {
            return fail_with(
                "the expression ends where a value is expected",
                "write a number, a parameter or (...)",
            );
        };
        self.at += 1;
        match token {
            Token::Number(value, _) => Ok(value),
            Token::Name(name) => match self.scope.lookup(&name) {
                Some(binding) => Ok(binding.value.clone()),
                None => unknown_parameter(&name, self.scope),
            },
            Token::Open => {
                let value = self.expression(depth + 1)?;
                if self.peek() == Some(&Token::Close) {
                    self.at += 1;
                    Ok(value)
                } else {
                    fail("a '(' is not closed")
                }
            }
            Token::Close => fail("a ')' without a '('"),
            Token::Plus | Token::Minus | Token::Star | Token::Slash => fail_with(
                "an operator where a value is expected",
                "write a number, a parameter or (...) on each side of + - * /",
            ),
        }
    }
}

fn too_large() -> ExprError {
    ExprError {
        message: TOO_LARGE.to_owned(),
        help: None,
        silent: false,
    }
}

/// The error for a name that is not a parameter of the scope.
pub(crate) fn unknown_parameter<T>(name: &str, scope: &dyn Scope) -> Result<T, ExprError> {
    if scope.is_unbound(name) {
        return Err(ExprError {
            message: String::new(),
            help: None,
            silent: true,
        });
    }
    let names = scope.names();
    let help = match closest(name, &names) {
        Some(near) => format!("did you mean `{near}`?"),
        None if names.is_empty() => {
            "no parameter is declared here (`params:` declares them)".to_owned()
        }
        None => format!("the parameters here are {}", names.join(", ")),
    };
    fail_with(format!("`{name}` is not a parameter"), help)
}

fn describe(value: &Value) -> &'static str {
    value.kind().what()
}

fn add_or_subtract(left: &Value, right: &Value, subtract: bool) -> Result<Value, ExprError> {
    let operation = |a: Ratio, b: Ratio| if subtract { a.sub(b) } else { a.add(b) };
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => {
            Ok(Value::Number(operation(*a, *b).ok_or_else(too_large)?))
        }
        (Value::Quantity(d, a), Value::Quantity(e, b)) if d == e => Ok(Value::Quantity(
            *d,
            operation(*a, *b).ok_or_else(too_large)?,
        )),
        _ => fail_with(
            format!(
                "cannot {} {} {} {}",
                if subtract { "subtract" } else { "add" },
                describe(left),
                if subtract { "from" } else { "and" },
                describe(right)
            ),
            "both sides of + and - need the same kind of value",
        ),
    }
}

fn multiply(left: &Value, right: &Value) -> Result<Value, ExprError> {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => Ok(Value::Number(a.mul(*b).ok_or_else(too_large)?)),
        (Value::Quantity(d, a), Value::Number(b)) | (Value::Number(b), Value::Quantity(d, a)) => {
            Ok(Value::Quantity(*d, a.mul(*b).ok_or_else(too_large)?))
        }
        _ => fail_with(
            format!("cannot multiply {} by {}", describe(left), describe(right)),
            "a quantity is multiplied by a number; two quantities have no unit in the format",
        ),
    }
}

fn divide(left: &Value, right: &Value) -> Result<Value, ExprError> {
    let zero = |b: &Ratio| b.is_zero();
    match (left, right) {
        (Value::Number(_) | Value::Quantity(..), Value::Number(b)) if zero(b) => {
            fail("division by zero")
        }
        (Value::Quantity(_, _), Value::Quantity(_, b)) if zero(b) => fail("division by zero"),
        (Value::Number(a), Value::Number(b)) => Ok(Value::Number(a.div(*b).ok_or_else(too_large)?)),
        (Value::Quantity(d, a), Value::Number(b)) => {
            Ok(Value::Quantity(*d, a.div(*b).ok_or_else(too_large)?))
        }
        (Value::Quantity(d, a), Value::Quantity(e, b)) if d == e => {
            Ok(Value::Number(a.div(*b).ok_or_else(too_large)?))
        }
        _ => fail_with(
            format!("cannot divide {} by {}", describe(left), describe(right)),
            "a quantity is divided by a number or by a quantity of the same kind",
        ),
    }
}

/// The scalar that stands for `value`, in the form the format reads.
pub(crate) fn render(value: &Value) -> Result<Written, ExprError> {
    let plain = |text: String| Written {
        text,
        quoted: false,
    };
    match value {
        Value::Number(number) => number.decimal().map(plain).map_or_else(
            || {
                fail_with(
                    format!(
                        "the result {}/{} has no decimal form with at most {MAX_FRACTION_DIGITS} digits after the point",
                        number.numerator, number.denominator
                    ),
                    "divide by a number that leaves a finite decimal, or change the values",
                )
            },
            Ok,
        ),
        Value::Quantity(dimension, amount) => {
            if amount.denominator != 1 {
                return fail_with(
                    format!(
                        "the result is not a whole number of {} (it is {}/{})",
                        dimension.smallest(),
                        amount.numerator,
                        amount.denominator
                    ),
                    "the format has whole numbers of the smallest unit only; change the values so that the result is exact",
                );
            }
            if amount.numerator < 0 {
                return fail(format!("the result is a negative {}", dimension.what()));
            }
            let amount = u64::try_from(amount.numerator).map_err(|_| too_large())?;
            Ok(plain(units::write_quantity(*dimension, amount)))
        }
        Value::Bool(flag) => Ok(plain(flag.to_string())),
        Value::Text(text) => Ok(plain(text.clone())),
    }
}

/// Evaluates `{source}` of the scalar (without the braces) in `scope`.
pub(crate) fn evaluate(source: &str, scope: &dyn Scope) -> Result<Binding, ExprError> {
    if source.chars().count() > MAX_EXPRESSION {
        return fail(format!(
            "the expression is longer than {MAX_EXPRESSION} characters"
        ));
    }
    let tokens = tokens(source)?;
    if tokens.is_empty() {
        return fail_with(
            "the expression is empty",
            "write a parameter or a calculation between the braces",
        );
    }
    // A parameter alone is the scalar as it was written: the text and the quotes stay
    if let [Token::Name(name)] = tokens.as_slice() {
        return match scope.lookup(name) {
            Some(binding) => Ok(binding.clone()),
            None => unknown_parameter(name, scope),
        };
    }
    if let [Token::Number(value, text)] = tokens.as_slice() {
        return Ok(Binding {
            value: value.clone(),
            written: Written {
                text: text.clone(),
                quoted: false,
            },
        });
    }
    let mut parser = Parser {
        tokens,
        at: 0,
        scope,
    };
    let value = parser.expression(0)?;
    if let Some(extra) = parser.peek() {
        return fail_with(
            format!(
                "unexpected {} after a complete expression",
                match extra {
                    Token::Close => "')'",
                    Token::Number(..) | Token::Name(_) => "value",
                    _ => "operator",
                }
            ),
            "two values need an operator between them",
        );
    }
    Binding::computed(value)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    struct Env(BTreeMap<String, Binding>);

    impl Scope for Env {
        fn lookup(&self, name: &str) -> Option<&Binding> {
            self.0.get(name)
        }

        fn names(&self) -> Vec<&str> {
            self.0.keys().map(String::as_str).collect()
        }
    }

    fn env() -> Env {
        let mut map = BTreeMap::new();
        for (name, text) in [
            ("rtt", "40ms"),
            ("rate", "10mbit"),
            ("flows", "4"),
            ("loss", "2%"),
            ("size", "1500b"),
            ("label", "access"),
            ("on", "true"),
        ] {
            map.insert(name.to_owned(), Binding::from_scalar(text, false));
        }
        Env(map)
    }

    fn eval(source: &str) -> String {
        match evaluate(source, &env()) {
            Ok(binding) => binding.written.text,
            Err(error) => format!("error: {}", error.message),
        }
    }

    #[test]
    fn a_calculation_gives_the_value_in_the_largest_exact_unit() {
        for (source, expected) in [
            ("rtt / 2", "20ms"),
            ("rtt * 3", "120ms"),
            ("rtt + 10ms", "50ms"),
            ("rtt - 40ms", "0ns"),
            ("2 * rate", "20mbit"),
            ("rate / 4", "2500kbit"),
            ("size * flows", "6kb"),
            ("loss * 2", "4%"),
            ("loss / 4", "0.5%"),
            ("(rtt + rtt) / 4", "20ms"),
            ("1.5s + 500ms", "2s"),
            ("rtt / 20ms", "2"),
            ("0.1 + 0.2", "0.3"),
            ("flows * 2", "8"),
            ("flows / 8", "0.5"),
            ("-flows + 10", "6"),
            ("1m / 4", "15s"),
            ("1kib * 2", "2kib"),
        ] {
            assert_eq!(eval(source), expected, "{source}");
        }
    }

    #[test]
    fn the_precedence_is_that_of_arithmetic() {
        assert_eq!(eval("1 + 2 * 3"), "7");
        assert_eq!(eval("(1 + 2) * 3"), "9");
        assert_eq!(eval("10 - 2 - 3"), "5");
        assert_eq!(eval("100 / 10 / 2"), "5");
        assert_eq!(eval("- -3"), "3");
    }

    #[test]
    fn a_parameter_alone_is_the_scalar_as_it_was_written() {
        let mut scope = env();
        scope
            .0
            .insert("quoted".into(), Binding::from_scalar("1000kbit", true));
        let alone = evaluate("quoted", &scope).unwrap();
        assert_eq!(alone.written.text, "1000kbit");
        assert!(alone.written.quoted);
        assert_eq!(eval("label"), "access");
        assert_eq!(eval("on"), "true");
    }

    #[test]
    fn a_result_that_is_not_whole_is_an_error() {
        assert!(
            eval("rtt / 3").contains("not a whole number of nanoseconds"),
            "{}",
            eval("rtt / 3")
        );
        assert!(
            eval("flows / 3").contains("no decimal form"),
            "{}",
            eval("flows / 3")
        );
        assert!(
            eval("rtt - 50ms").contains("negative duration"),
            "{}",
            eval("rtt - 50ms")
        );
        assert!(eval("rate / 3").contains("not a whole number of bits per second"));
    }

    #[test]
    fn kinds_do_not_mix() {
        for (source, part) in [
            ("rtt + rate", "cannot add a duration and a rate"),
            ("rtt + 1", "cannot add a duration and a number"),
            ("rtt * rate", "cannot multiply a duration by a rate"),
            ("2 / rtt", "cannot divide a number by a duration"),
            ("rtt / rate", "cannot divide a duration by a rate"),
            ("label + 1", "cannot add text and a number"),
            ("-label", "cannot negate text"),
            ("rtt / 0", "division by zero"),
            ("rtt / (1 - 1)", "division by zero"),
            ("rtt / 0ms", "division by zero"),
        ] {
            assert!(eval(source).contains(part), "{source}: {}", eval(source));
        }
    }

    #[test]
    fn mistakes_in_the_text_are_named() {
        for (source, part) in [
            ("", "expression is empty"),
            ("rtt +", "ends where a value is expected"),
            ("(rtt", "not closed"),
            ("rtt)", "unexpected ')'"),
            ("rtt rtt", "unexpected value"),
            ("rtt $ 2", "unexpected '$'"),
            ("10parsecs", "'parsecs' is not a unit"),
            ("10 parsecs", "unexpected value"),
            ("1.", "needs digits after the point"),
            ("RTT", "unexpected 'R'"),
            ("rt", "`rt` is not a parameter"),
            ("* 2", "an operator where a value is expected"),
        ] {
            assert!(eval(source).contains(part), "{source:?}: {}", eval(source));
        }
        let error = evaluate("rt", &env()).unwrap_err();
        assert_eq!(error.help.as_deref(), Some("did you mean `rtt`?"));
        let error = evaluate("zzz", &env()).unwrap_err();
        assert!(error.help.unwrap().starts_with("the parameters here are"));
    }

    #[test]
    fn size_and_nesting_are_bounded() {
        assert!(eval(&"1+".repeat(200)).contains("longer than 256"));
        let deep = format!("{}1{}", "(".repeat(40), ")".repeat(40));
        assert!(
            eval(&deep).contains("nested more than 32"),
            "{}",
            eval(&deep)
        );
        // The product does not fit and is reported, not wrapped
        let huge = "999999999999h * 999999999999h";
        assert!(eval(huge).contains("cannot multiply"), "{}", eval(huge));
        assert!(
            eval("999999999999999999 * 999999999999999999 * 999999999999999999")
                .contains("too large")
        );
    }

    #[test]
    fn the_kinds_of_values_follow_the_text() {
        assert_eq!(
            Binding::from_scalar("10mbit", false).value.kind(),
            Kind::Quantity(Dimension::Rate)
        );
        assert_eq!(
            Binding::from_scalar("10mbit", true).value.kind(),
            Kind::Quantity(Dimension::Rate)
        );
        assert_eq!(Binding::from_scalar("7", false).value.kind(), Kind::Number);
        assert_eq!(Binding::from_scalar("7", true).value.kind(), Kind::Text);
        assert_eq!(
            Binding::from_scalar("-7.5", false).value.kind(),
            Kind::Number
        );
        assert_eq!(Binding::from_scalar("true", false).value.kind(), Kind::Bool);
        assert_eq!(
            Binding::from_scalar("access-1", false).value.kind(),
            Kind::Text
        );
        assert_eq!(Binding::from_scalar("1e3", false).value.kind(), Kind::Text);
        assert_eq!(Binding::from_scalar("", false).value.kind(), Kind::Text);
    }

    /// A canary: what the evaluator prints reads back into the same value, for every kind of quantity.
    #[test]
    fn what_is_printed_reads_back_as_the_same_value() {
        for text in [
            "1ns",
            "999us",
            "1500ms",
            "61s",
            "1h",
            "1bit",
            "999kbit",
            "3gbit",
            "1b",
            "1000b",
            "1kib",
            "1mib",
            "0.000001%",
            "99.999999%",
            "100%",
        ] {
            let value = Binding::from_scalar(text, false).value;
            let printed = render(&value).unwrap().text;
            assert_eq!(
                Binding::from_scalar(&printed, false).value,
                value,
                "{text} -> {printed}"
            );
        }
    }
}
