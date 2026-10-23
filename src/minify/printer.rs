//! Emits a syntax tree as compact Lua source.
//!
//! Whitespace is only inserted where two tokens would otherwise lex as
//! something else, and parentheses only where precedence requires them.

use crate::lua::ast::*;
use crate::lua::lexer::{Num, is_keyword};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Style {
    /// Single line, no optional whitespace.
    Compact,
    /// One statement per line with indentation; handy for debugging output.
    Pretty,
}

pub fn print_chunk(b: &Block, style: Style) -> String {
    let mut p = Printer {
        out: String::new(),
        last: String::new(),
        style,
        indent: 0,
    };
    p.block(b);
    p.out
}

struct Printer {
    out: String,
    /// The last token written (empty at the start or after a newline).
    last: String,
    style: Style,
    indent: usize,
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn needs_space(prev: &str, next: &str) -> bool {
    let (Some(a), Some(b)) = (prev.chars().last(), next.chars().next()) else {
        return false;
    };
    if is_word_char(a) && is_word_char(b) {
        return true;
    }
    let prev_is_num = prev
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_digit() || c == '.')
        && prev.chars().any(|c| c.is_ascii_digit());
    match (a, b) {
        ('-', '-') => true,
        // "1" ".." must not become the malformed numeral "1..".
        (_, '.') if prev_is_num => true,
        ('.', '.') => true,
        ('[', '[' | '=') => true,
        _ => false,
    }
}

impl Printer {
    fn tok(&mut self, s: &str) {
        if needs_space(&self.last, s) {
            self.out.push(' ');
        }
        self.out.push_str(s);
        self.last.clear();
        self.last.push_str(s);
    }

    /// Optional whitespace in pretty mode.
    fn sp(&mut self) {
        if self.style == Style::Pretty {
            self.out.push(' ');
            self.last.clear();
        }
    }

    fn newline(&mut self) {
        if self.style == Style::Pretty {
            self.out.push('\n');
            for _ in 0..self.indent {
                self.out.push_str("  ");
            }
            self.last.clear();
        }
    }

    fn block(&mut self, b: &Block) {
        let mut first = true;
        for st in b {
            if self.style == Style::Pretty && !(first && self.out.is_empty()) {
                self.newline();
            }
            // A statement starting with '(' could be read as a call on the
            // previous statement's last expression.
            if !first && starts_with_paren(st) {
                self.tok(";");
            }
            self.stat(st);
            first = false;
        }
    }

    fn body(&mut self, b: &Block) {
        self.indent += 1;
        self.block(b);
        self.indent -= 1;
        if !b.is_empty() {
            self.newline();
        } else {
            self.sp();
        }
    }

    fn names(&mut self, ns: &[Name]) {
        for (i, n) in ns.iter().enumerate() {
            if i > 0 {
                self.tok(",");
            }
            self.tok(&n.text);
        }
    }

    fn exprs(&mut self, es: &[Expr]) {
        for (i, e) in es.iter().enumerate() {
            if i > 0 {
                self.tok(",");
                self.sp();
            }
            self.expr(e, 0);
        }
    }

    fn assign_op(&mut self) {
        self.sp();
        self.tok("=");
        self.sp();
    }

    fn stat(&mut self, st: &Stat) {
        match st {
            Stat::Call(e) => self.expr(e, 0),
            Stat::Assign(t, e) => {
                self.exprs(t);
                self.assign_op();
                self.exprs(e);
            }
            Stat::Local(n, e) => {
                self.tok("local");
                self.names(n);
                if !e.is_empty() {
                    self.assign_op();
                    self.exprs(e);
                }
            }
            Stat::LocalFunction(n, f) => {
                self.tok("local");
                self.tok("function");
                self.tok(&n.text);
                self.func_body(f);
            }
            Stat::Function(base, fields, f) => {
                self.tok("function");
                self.tok(&base.text);
                for fl in fields {
                    self.tok(".");
                    self.tok(fl);
                }
                self.func_body(f);
            }
            Stat::Do(b) => {
                self.tok("do");
                self.body(b);
                self.tok("end");
            }
            Stat::While(c, b) => {
                self.tok("while");
                self.expr(c, 0);
                self.tok("do");
                self.body(b);
                self.tok("end");
            }
            Stat::Repeat(b, c) => {
                self.tok("repeat");
                self.body(b);
                self.tok("until");
                self.expr(c, 0);
            }
            Stat::If(clauses, els) => {
                for (i, (c, b)) in clauses.iter().enumerate() {
                    self.tok(if i == 0 { "if" } else { "elseif" });
                    self.expr(c, 0);
                    self.tok("then");
                    self.body(b);
                }
                if let Some(b) = els {
                    self.tok("else");
                    self.body(b);
                }
                self.tok("end");
            }
            Stat::NumFor(v, a, b, c, body) => {
                self.tok("for");
                self.tok(&v.text);
                self.assign_op();
                self.expr(a, 0);
                self.tok(",");
                self.expr(b, 0);
                if let Some(c) = c {
                    self.tok(",");
                    self.expr(c, 0);
                }
                self.tok("do");
                self.body(body);
                self.tok("end");
            }
            Stat::GenFor(vars, exprs, body) => {
                self.tok("for");
                self.names(vars);
                self.tok("in");
                self.exprs(exprs);
                self.tok("do");
                self.body(body);
                self.tok("end");
            }
            Stat::Return(e) => {
                self.tok("return");
                self.exprs(e);
            }
            Stat::Break => self.tok("break"),
            Stat::Goto(l) => {
                self.tok("goto");
                self.tok(l);
            }
            Stat::Label(l) => {
                self.tok("::");
                self.tok(l);
                self.tok("::");
            }
        }
    }

    fn func_body(&mut self, f: &FuncBody) {
        self.tok("(");
        self.names(&f.params);
        if f.vararg {
            if !f.params.is_empty() {
                self.tok(",");
            }
            self.tok("...");
        }
        self.tok(")");
        self.body(&f.body);
        self.tok("end");
    }

    /// Prints `e` so that it parses correctly where an operand with binding
    /// strength `min_prec` is expected.
    fn expr(&mut self, e: &Expr, min_prec: u8) {
        let e = strip_parens(e);
        let prec = expr_prec(e);
        if prec < min_prec {
            self.tok("(");
            self.expr(e, 0);
            self.tok(")");
            return;
        }
        match e {
            Expr::Nil => self.tok("nil"),
            Expr::True => self.tok("true"),
            Expr::False => self.tok("false"),
            Expr::Vararg => self.tok("..."),
            Expr::Num(n) => self.tok(&format_number(n)),
            Expr::Str(s) => self.tok(&quote_string(s)),
            Expr::Function(f) => {
                self.tok("function");
                self.func_body(f);
            }
            Expr::Table(fields) => self.table(fields),
            Expr::Bin(op, a, b) => {
                let (lp, rp) = op.priority();
                // Left operand: lower or right-binding ops need parens.
                self.expr_bin_operand(a, lp, true);
                self.sp();
                self.tok(op.as_str());
                self.sp();
                self.expr_bin_operand(b, rp, false);
            }
            Expr::Un(op, a) => {
                self.tok(op.as_str());
                self.expr(a, UNARY_PRIORITY);
            }
            Expr::Name(n) => self.tok(&n.text),
            Expr::Index(base, key) => {
                self.prefix(base);
                match key.as_ref() {
                    Expr::Str(s) if is_identifier(s) => {
                        self.tok(".");
                        self.tok(std::str::from_utf8(s).unwrap());
                    }
                    k => {
                        self.tok("[");
                        self.expr(k, 0);
                        self.tok("]");
                    }
                }
            }
            Expr::Call(f, args) => {
                self.prefix(f);
                self.args(args);
            }
            Expr::Method(obj, name, args) => {
                self.prefix(obj);
                self.tok(":");
                self.tok(name);
                self.args(args);
            }
            Expr::Paren(inner) => {
                self.tok("(");
                self.expr(inner, 0);
                self.tok(")");
            }
        }
    }

    fn expr_bin_operand(&mut self, e: &Expr, prio: u8, left: bool) {
        let e = strip_parens(e);
        let need = match e {
            // `a op2 b op c` keeps (a op2 b) together iff lp(op) <= rp(op2).
            Expr::Bin(op2, ..) if left => prio > op2.priority().1,
            // `a op b op3 c` groups (b op3 c) iff lp(op3) > rp(op).
            Expr::Bin(op3, ..) => op3.priority().0 <= prio,
            // `-a ^ b` parses as -(a ^ b).
            Expr::Un(..) if left => prio > UNARY_PRIORITY,
            Expr::Num(n) if is_negative(n) && left => prio > UNARY_PRIORITY,
            _ => false,
        };
        if need {
            self.tok("(");
            self.expr(e, 0);
            self.tok(")");
        } else {
            self.expr(e, 0);
        }
    }

    /// Prints an expression used as the base of a call or index.
    fn prefix(&mut self, e: &Expr) {
        let e = strip_parens(e);
        match e {
            Expr::Name(_)
            | Expr::Index(..)
            | Expr::Call(..)
            | Expr::Method(..)
            | Expr::Paren(_) => self.expr(e, 0),
            _ => {
                self.tok("(");
                self.expr(e, 0);
                self.tok(")");
            }
        }
    }

    fn args(&mut self, args: &[Expr]) {
        if let [a] = args {
            match strip_parens(a) {
                Expr::Str(s) => return self.tok(&quote_string(s)),
                Expr::Table(f) => return self.table(f),
                _ => {}
            }
        }
        self.tok("(");
        self.exprs(args);
        self.tok(")");
    }

    fn table(&mut self, fields: &[Field]) {
        self.tok("{");
        for (i, f) in fields.iter().enumerate() {
            if i > 0 {
                self.tok(",");
                self.sp();
            }
            match f {
                Field::Pos(v) => self.expr(v, 0),
                Field::Named(n, v) => {
                    self.tok(n);
                    self.assign_op();
                    self.expr(v, 0);
                }
                Field::Keyed(k, v) => {
                    match strip_parens(k) {
                        Expr::Str(s) if is_identifier(s) => {
                            self.tok(std::str::from_utf8(s).unwrap())
                        }
                        k => {
                            self.tok("[");
                            self.expr(k, 0);
                            self.tok("]");
                        }
                    }
                    self.assign_op();
                    self.expr(v, 0);
                }
            }
        }
        self.tok("}");
    }
}

/// Removes parentheses that do not change meaning. Parentheses around calls
/// and varargs truncate to a single value and are kept.
fn strip_parens(mut e: &Expr) -> &Expr {
    while let Expr::Paren(inner) = e {
        match inner.as_ref() {
            Expr::Paren(_) => e = inner,
            i if i.is_multi() => return e,
            i => e = i,
        }
    }
    e
}

fn is_negative(n: &Num) -> bool {
    match n {
        Num::Int(i) => *i < 0,
        Num::Float(f) => f.is_sign_negative() && !f.is_nan(),
    }
}

fn expr_prec(e: &Expr) -> u8 {
    match e {
        Expr::Num(n) if is_negative(n) => UNARY_PRIORITY,
        Expr::Bin(op, ..) => op.priority().0,
        Expr::Un(..) => UNARY_PRIORITY,
        _ => 100,
    }
}

fn starts_with_paren(st: &Stat) -> bool {
    /// Whether printing `e` in prefix position starts with '('.
    fn prefix_starts_with_paren(e: &Expr) -> bool {
        match strip_parens(e) {
            Expr::Name(_) => false,
            Expr::Index(b, _) | Expr::Call(b, _) | Expr::Method(b, _, _) => {
                prefix_starts_with_paren(b)
            }
            // Kept parentheses, or a non-prefix expression that `prefix` wraps.
            _ => true,
        }
    }
    match st {
        Stat::Call(e) => prefix_starts_with_paren(e),
        Stat::Assign(t, _) => t.first().is_some_and(prefix_starts_with_paren),
        _ => false,
    }
}

pub fn is_identifier(s: &[u8]) -> bool {
    let Ok(s) = std::str::from_utf8(s) else {
        return false;
    };
    let mut cs = s.chars();
    matches!(cs.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && cs.all(is_word_char)
        && !is_keyword(s)
}

pub fn format_number(n: &Num) -> String {
    match *n {
        Num::Int(i) => {
            if i == i64::MIN {
                "0x8000000000000000".to_string()
            } else {
                let dec = i.to_string();
                // Large round numbers can be shorter in hex, e.g. 0xFFFFFF.
                let hex = if i < 0 {
                    format!("-0x{:x}", -i)
                } else {
                    format!("0x{i:x}")
                };
                if hex.len() < dec.len() { hex } else { dec }
            }
        }
        Num::Float(f) => format_float(f),
    }
}

fn format_float(f: f64) -> String {
    if f.is_nan() {
        return "(0/0)".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 {
            "1e999".into()
        } else {
            "-1e999".into()
        };
    }
    let neg = f.is_sign_negative();
    let a = f.abs();
    let plain = format!("{a}");
    let exp = format!("{a:e}");
    let mut best = if exp.len() < plain.len() { exp } else { plain };
    if let Some(rest) = best.strip_prefix("0.") {
        best = format!(".{rest}");
    }
    if !best.contains(['.', 'e', 'E']) {
        // Must stay a float in Lua 5.3: "3" is an integer, "3." is not.
        best.push('.');
    }
    if neg { format!("-{best}") } else { best }
}

pub fn quote_string(s: &[u8]) -> String {
    let dq = s.iter().filter(|&&c| c == b'"').count();
    let sq = s.iter().filter(|&&c| c == b'\'').count();
    let q = if sq < dq { b'\'' } else { b'"' };
    let utf8_ok = std::str::from_utf8(s).is_ok();
    let mut out = Vec::with_capacity(s.len() + 2);
    out.push(q);
    for (i, &c) in s.iter().enumerate() {
        match c {
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\t' => out.extend_from_slice(b"\\t"),
            c if c == q => {
                out.push(b'\\');
                out.push(c);
            }
            c if c < 32 || c == 127 || (c >= 128 && !utf8_ok) => {
                let next_digit = s.get(i + 1).is_some_and(|d| d.is_ascii_digit());
                let esc = if next_digit {
                    format!("\\{c:03}")
                } else {
                    format!("\\{c}")
                };
                out.extend_from_slice(esc.as_bytes());
            }
            c => out.push(c),
        }
    }
    out.push(q);
    String::from_utf8(out).expect("escaped string is valid UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lua::parser::parse;

    fn roundtrip(src: &str) -> String {
        print_chunk(&parse(src).unwrap(), Style::Compact)
    }

    #[test]
    fn numbers() {
        assert_eq!(format_float(0.5), ".5");
        assert_eq!(format_float(3.0), "3.");
        assert_eq!(format_float(1e6), "1e6");
        assert_eq!(format_float(0.25), ".25");
        assert_eq!(format_float(1.5e-7), "1.5e-7");
        assert_eq!(format_number(&Num::Int(1 << 60)), "0x1000000000000000");
        assert_eq!(format_number(&Num::Int(100)), "100");
    }

    #[test]
    fn precedence() {
        assert_eq!(roundtrip("x = (a + b) * c"), "x=(a+b)*c");
        assert_eq!(roundtrip("x = a + (b * c)"), "x=a+b*c");
        assert_eq!(roundtrip("x = (a - b) - c"), "x=a-b-c");
        assert_eq!(roundtrip("x = a - (b - c)"), "x=a-(b-c)");
        assert_eq!(roundtrip("x = (a .. b) .. c"), "x=(a..b)..c");
        assert_eq!(roundtrip("x = a .. (b .. c)"), "x=a..b..c");
        assert_eq!(roundtrip("x = (-a) ^ 2"), "x=(-a)^2");
        assert_eq!(roundtrip("x = -(a ^ 2)"), "x=-a^2");
        assert_eq!(roundtrip("x = - -a"), "x=- -a");
        assert_eq!(roundtrip("x = (f())"), "x=(f())");
        assert_eq!(roundtrip("x = ('a'):rep(2)"), "x=(\"a\"):rep(2)");
        assert_eq!(roundtrip("x = 1 .. 2"), "x=1 ..2");
        assert_eq!(roundtrip("a = b;(f)()"), "a=b f()");
        assert_eq!(roundtrip("a = b;(f)().x = 1"), "a=b f().x=1");
        assert_eq!(roundtrip("a = b\n(f)()"), "a=b(f)()");
        assert_eq!(roundtrip("a = b;(f or g)()"), "a=b;(f or g)()");
        assert_eq!(roundtrip("f('x') g({1})"), "f\"x\"g{1}");
        assert_eq!(
            roundtrip("local t = {['a'] = 1, ['end'] = 2}"),
            "local t={a=1,[\"end\"]=2}"
        );
    }

    #[test]
    fn strings() {
        assert_eq!(quote_string(b"it's"), "\"it's\"");
        assert_eq!(quote_string(b"say \"hi\""), "'say \"hi\"'");
        assert_eq!(quote_string(b"\x001"), "\"\\0001\"");
        assert_eq!(quote_string("ü".as_bytes()), "\"ü\"");
    }
}
