//! Sums typed into the launcher: `12*7.5`, `2^10`, `sqrt(2)`, `80*15%`.
//!
//! A query is a sum only if it reads as one through to the end and asks for
//! something to be worked out — an operator or a function, not a bare
//! number — so a search for "2026" stays a search. The answer is listed
//! first; Enter copies it.

use std::{f64::consts, iter::Peekable, str::Chars};

/// The answer to `query`, if it is a sum, written as it would be read.
pub fn answer(query: &str) -> Option<String> {
    let text = query.trim();
    let text = text.strip_prefix('=').unwrap_or(text).trim();
    if text.is_empty() || !text.chars().any(|c| c.is_ascii_digit() || c == 'π') {
        return None;
    }
    let mut parser = Parser {
        chars: text.chars().peekable(),
        worked: query.trim_start().starts_with('='),
    };
    let value = parser.expr()?;
    parser.skip_space();
    if parser.chars.peek().is_some() || !parser.worked || !value.is_finite() {
        return None;
    }
    Some(format(value))
}

/// Few enough digits to read, no more than the sum earned.
pub fn format(value: f64) -> String {
    if value == value.trunc() && value.abs() < 1e15 {
        return format!("{value:.0}");
    }
    if value.abs() >= 1e15 || value.abs() < 1e-6 {
        return format!("{value:.6e}");
    }
    let text = format!("{value:.10}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    text.to_owned()
}

struct Parser<'a> {
    chars: Peekable<Chars<'a>>,
    /// Something was worked out: an operator, a function, a percentage.
    worked: bool,
}

impl Parser<'_> {
    fn skip_space(&mut self) {
        while self.chars.peek().is_some_and(|c| c.is_whitespace()) {
            self.chars.next();
        }
    }

    fn eat(&mut self, wanted: &[char]) -> Option<char> {
        self.skip_space();
        let c = *self.chars.peek()?;
        wanted.contains(&c).then(|| {
            self.chars.next();
            c
        })
    }

    /// Sums and differences.
    fn expr(&mut self) -> Option<f64> {
        let mut value = self.term()?;
        while let Some(op) = self.eat(&['+', '-', '−']) {
            self.worked = true;
            let rhs = self.term()?;
            value = if op == '+' { value + rhs } else { value - rhs };
        }
        Some(value)
    }

    /// Products and quotients.
    fn term(&mut self) -> Option<f64> {
        let mut value = self.power()?;
        while let Some(op) = self.eat(&['*', '/', '×', '÷', 'x']) {
            self.worked = true;
            let rhs = self.power()?;
            value = match op {
                '/' | '÷' => value / rhs,
                _ => value * rhs,
            };
        }
        Some(value)
    }

    /// Powers, right to left: `2^3^2` is `2^9`.
    fn power(&mut self) -> Option<f64> {
        let base = self.unary()?;
        if self.eat(&['^']).is_some() {
            self.worked = true;
            let exponent = self.power()?;
            return Some(base.powf(exponent));
        }
        Some(base)
    }

    fn unary(&mut self) -> Option<f64> {
        if let Some(sign) = self.eat(&['-', '−', '+']) {
            let value = self.unary()?;
            return Some(if sign == '+' { value } else { -value });
        }
        let value = self.primary()?;
        // A percentage is a hundredth.
        if self.eat(&['%']).is_some() {
            self.worked = true;
            return Some(value / 100.0);
        }
        Some(value)
    }

    fn primary(&mut self) -> Option<f64> {
        self.skip_space();
        let c = *self.chars.peek()?;
        if c == '(' {
            self.chars.next();
            let value = self.expr()?;
            self.eat(&[')'])?;
            return Some(value);
        }
        if c.is_ascii_digit() || c == '.' {
            return self.number();
        }
        if c == 'π' {
            self.chars.next();
            return Some(consts::PI);
        }
        if c.is_ascii_alphabetic() {
            let mut name = String::new();
            while let Some(&c) = self.chars.peek().filter(|c| c.is_ascii_alphanumeric()) {
                name.push(c);
                self.chars.next();
            }
            return self.named(&name.to_ascii_lowercase());
        }
        None
    }

    fn number(&mut self) -> Option<f64> {
        let mut text = String::new();
        while let Some(&c) = self
            .chars
            .peek()
            .filter(|c| c.is_ascii_digit() || **c == '.' || **c == '_')
        {
            if c != '_' {
                text.push(c);
            }
            self.chars.next();
        }
        text.parse().ok()
    }

    /// A constant, or a function of what's in brackets after it.
    fn named(&mut self, name: &str) -> Option<f64> {
        match name {
            "pi" => return Some(consts::PI),
            "e" => return Some(consts::E),
            "tau" => return Some(consts::TAU),
            _ => {}
        }
        let function: fn(f64) -> f64 = match name {
            "sqrt" => f64::sqrt,
            "abs" => f64::abs,
            "round" => f64::round,
            "floor" => f64::floor,
            "ceil" => f64::ceil,
            "ln" => f64::ln,
            "log" => f64::log10,
            "exp" => f64::exp,
            "sin" => f64::sin,
            "cos" => f64::cos,
            "tan" => f64::tan,
            _ => return None,
        };
        self.eat(&['('])?;
        let value = self.expr()?;
        self.eat(&[')'])?;
        self.worked = true;
        Some(function(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_are_worked_out() {
        assert_eq!(answer("12*7.5").as_deref(), Some("90"));
        assert_eq!(answer("2^10").as_deref(), Some("1024"));
        assert_eq!(answer("2^3^2").as_deref(), Some("512"));
        assert_eq!(answer("(1 + 2) × 3").as_deref(), Some("9"));
        assert_eq!(answer("80 * 15%").as_deref(), Some("12"));
        assert_eq!(answer("1/3").as_deref(), Some("0.3333333333"));
        assert_eq!(answer("sqrt(2)").as_deref(), Some("1.4142135624"));
        assert_eq!(answer("-4 + 1").as_deref(), Some("-3"));
        assert_eq!(answer("10 / 4").as_deref(), Some("2.5"));
        assert_eq!(answer("1_000 * 3").as_deref(), Some("3000"));
        assert_eq!(answer("= 42").as_deref(), Some("42"));
    }

    #[test]
    fn searches_stay_searches() {
        for query in [
            "2026", "firefox", "7zip", "e", "pi", "sqrt", "2 +", "(1", "1/0", "x11", "10x",
        ] {
            assert_eq!(answer(query), None, "{query}");
        }
    }
}
