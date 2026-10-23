//! Lua 5.3 lexer.

use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum Num {
    Int(i64),
    Float(f64),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Name(String),
    Kw(&'static str),
    Num(Num),
    Str(Vec<u8>),
    Sym(&'static str),
    Eof,
}

impl fmt::Display for Tok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tok::Name(n) => write!(f, "'{n}'"),
            Tok::Kw(k) | Tok::Sym(k) => write!(f, "'{k}'"),
            Tok::Num(Num::Int(i)) => write!(f, "'{i}'"),
            Tok::Num(Num::Float(x)) => write!(f, "'{x}'"),
            Tok::Str(s) => write!(f, "string \"{}\"", String::from_utf8_lossy(s)),
            Tok::Eof => write!(f, "<eof>"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pos {
    pub line: u32,
    pub col: u32,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub pos: Pos,
}

#[derive(Debug, Clone)]
pub struct LexError {
    pub pos: Pos,
    pub msg: String,
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.pos.line, self.pos.col, self.msg)
    }
}

pub const KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in",
    "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

// Longest first so that greedy matching works.
const SYMBOLS: &[&str] = &[
    "...", "..", "==", "~=", "<=", ">=", "<<", ">>", "//", "::", "+", "-", "*", "/", "%", "^", "#",
    "&", "~", "|", "<", ">", "=", "(", ")", "{", "}", "[", "]", ";", ":", ",", ".",
];

pub fn is_keyword(s: &str) -> bool {
    KEYWORDS.contains(&s)
}

pub fn keyword(s: &str) -> Option<&'static str> {
    KEYWORDS.iter().copied().find(|k| *k == s)
}

pub struct Lexer<'a> {
    src: &'a [u8],
    i: usize,
    line: u32,
    line_start: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Self {
        let mut lx = Lexer {
            src: src.as_bytes(),
            i: 0,
            line: 1,
            line_start: 0,
        };
        // Skip a shebang line, like the reference implementation.
        if lx.src.starts_with(b"#") {
            while lx.i < lx.src.len() && lx.src[lx.i] != b'\n' {
                lx.i += 1;
            }
        }
        lx
    }

    fn pos(&self) -> Pos {
        Pos {
            line: self.line,
            col: (self.i - self.line_start + 1) as u32,
        }
    }

    fn err<T>(&self, msg: impl Into<String>) -> Result<T, LexError> {
        Err(LexError {
            pos: self.pos(),
            msg: msg.into(),
        })
    }

    fn peek(&self) -> u8 {
        self.peek_at(0)
    }

    fn peek_at(&self, n: usize) -> u8 {
        *self.src.get(self.i + n).unwrap_or(&0)
    }

    fn newline(&mut self) {
        // Handles \n, \r, \r\n and \n\r as a single line break.
        let c = self.src[self.i];
        self.i += 1;
        let d = self.peek();
        if (d == b'\n' || d == b'\r') && d != c {
            self.i += 1;
        }
        self.line += 1;
        self.line_start = self.i;
    }

    pub fn tokenize(mut self) -> Result<Vec<Token>, LexError> {
        let mut out = Vec::new();
        loop {
            let t = self.next_token()?;
            let eof = t.tok == Tok::Eof;
            out.push(t);
            if eof {
                return Ok(out);
            }
        }
    }

    fn skip_trivia(&mut self) -> Result<(), LexError> {
        loop {
            match self.peek() {
                b'\n' | b'\r' => self.newline(),
                b' ' | b'\t' | 0x0b | 0x0c => self.i += 1,
                b'-' if self.peek_at(1) == b'-' => {
                    self.i += 2;
                    if self.peek() == b'['
                        && let Some(level) = self.long_bracket_level()
                    {
                        self.read_long_string(level)?;
                        continue;
                    }
                    while self.i < self.src.len() && !matches!(self.peek(), b'\n' | b'\r') {
                        self.i += 1;
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    /// If positioned at `[` followed by `=`* and `[`, returns the level.
    fn long_bracket_level(&self) -> Option<usize> {
        let mut j = self.i + 1;
        let mut level = 0;
        while self.src.get(j) == Some(&b'=') {
            level += 1;
            j += 1;
        }
        (self.src.get(j) == Some(&b'[')).then_some(level)
    }

    fn read_long_string(&mut self, level: usize) -> Result<Vec<u8>, LexError> {
        let start = self.pos();
        self.i += level + 2;
        // A newline immediately after the opening bracket is skipped.
        if matches!(self.peek(), b'\n' | b'\r') {
            self.newline();
        }
        let mut buf = Vec::new();
        loop {
            if self.i >= self.src.len() {
                return Err(LexError {
                    pos: start,
                    msg: "unfinished long string/comment".into(),
                });
            }
            match self.peek() {
                b']' => {
                    let mut j = self.i + 1;
                    let mut l = 0;
                    while self.src.get(j) == Some(&b'=') {
                        l += 1;
                        j += 1;
                    }
                    if l == level && self.src.get(j) == Some(&b']') {
                        self.i = j + 1;
                        return Ok(buf);
                    }
                    buf.push(b']');
                    self.i += 1;
                }
                b'\n' | b'\r' => {
                    self.newline();
                    buf.push(b'\n');
                }
                c => {
                    buf.push(c);
                    self.i += 1;
                }
            }
        }
    }

    fn next_token(&mut self) -> Result<Token, LexError> {
        self.skip_trivia()?;
        let pos = self.pos();
        let tok = self.read_tok()?;
        Ok(Token { tok, pos })
    }

    fn read_tok(&mut self) -> Result<Tok, LexError> {
        if self.i >= self.src.len() {
            return Ok(Tok::Eof);
        }
        let c = self.peek();
        if c.is_ascii_alphabetic() || c == b'_' {
            let s = self.i;
            while self.peek().is_ascii_alphanumeric() || self.peek() == b'_' {
                self.i += 1;
            }
            let word = std::str::from_utf8(&self.src[s..self.i]).unwrap();
            return Ok(match keyword(word) {
                Some(k) => Tok::Kw(k),
                None => Tok::Name(word.to_string()),
            });
        }
        if c.is_ascii_digit() || (c == b'.' && self.peek_at(1).is_ascii_digit()) {
            return self.read_number().map(Tok::Num);
        }
        if c == b'"' || c == b'\'' {
            return self.read_string(c).map(Tok::Str);
        }
        if c == b'['
            && let Some(level) = self.long_bracket_level()
        {
            return self.read_long_string(level).map(Tok::Str);
        }
        for s in SYMBOLS {
            if self.src[self.i..].starts_with(s.as_bytes()) {
                self.i += s.len();
                return Ok(Tok::Sym(s));
            }
        }
        self.err(format!("unexpected symbol '{}'", c as char))
    }

    fn read_number(&mut self) -> Result<Num, LexError> {
        let s = self.i;
        let hex = self.peek() == b'0' && matches!(self.peek_at(1), b'x' | b'X');
        let (exp_a, exp_b) = if hex { (b'p', b'P') } else { (b'e', b'E') };
        if hex {
            self.i += 2;
        }
        loop {
            let c = self.peek();
            if c == exp_a || c == exp_b {
                self.i += 1;
                if matches!(self.peek(), b'+' | b'-') {
                    self.i += 1;
                }
            } else if c.is_ascii_hexdigit() || c == b'.' {
                self.i += 1;
            } else {
                break;
            }
        }
        // Lua reads trailing alphanumerics into the numeral and then rejects it.
        if self.peek().is_ascii_alphanumeric() || self.peek() == b'_' {
            return self.err("malformed number");
        }
        let text = std::str::from_utf8(&self.src[s..self.i]).unwrap();
        parse_number(text)
            .ok_or(())
            .or_else(|_| self.err(format!("malformed number near '{text}'")))
    }

    fn read_string(&mut self, quote: u8) -> Result<Vec<u8>, LexError> {
        self.i += 1;
        let mut buf = Vec::new();
        loop {
            let c = self.peek();
            if self.i >= self.src.len() || c == b'\n' || c == b'\r' {
                return self.err("unfinished string");
            }
            self.i += 1;
            if c == quote {
                return Ok(buf);
            }
            if c != b'\\' {
                buf.push(c);
                continue;
            }
            let e = self.peek();
            match e {
                b'a' => buf.push(7),
                b'b' => buf.push(8),
                b'f' => buf.push(12),
                b'n' => buf.push(b'\n'),
                b'r' => buf.push(b'\r'),
                b't' => buf.push(b'\t'),
                b'v' => buf.push(11),
                b'\\' | b'"' | b'\'' => buf.push(e),
                b'\n' | b'\r' => {
                    buf.push(b'\n');
                    self.newline();
                    continue;
                }
                b'x' => {
                    let h =
                        std::str::from_utf8(self.src.get(self.i + 1..self.i + 3).unwrap_or(b""))
                            .ok()
                            .and_then(|h| u8::from_str_radix(h, 16).ok());
                    match h {
                        Some(v) => buf.push(v),
                        None => return self.err("hexadecimal digit expected"),
                    }
                    self.i += 3;
                    continue;
                }
                b'z' => {
                    self.i += 1;
                    loop {
                        match self.peek() {
                            b'\n' | b'\r' => self.newline(),
                            b' ' | b'\t' | 0x0b | 0x0c => self.i += 1,
                            _ => break,
                        }
                    }
                    continue;
                }
                b'u' => {
                    if self.peek_at(1) != b'{' {
                        return self.err("missing '{' in \\u{xxxx}");
                    }
                    self.i += 2;
                    let s = self.i;
                    while self.peek().is_ascii_hexdigit() {
                        self.i += 1;
                    }
                    if self.peek() != b'}' || s == self.i {
                        return self.err("malformed \\u{xxxx} escape");
                    }
                    let v =
                        u32::from_str_radix(std::str::from_utf8(&self.src[s..self.i]).unwrap(), 16)
                            .ok()
                            .filter(|v| *v <= 0x7FFF_FFFF);
                    let Some(v) = v else {
                        return self.err("UTF-8 value too large");
                    };
                    self.i += 1;
                    utf8_encode(v, &mut buf);
                    continue;
                }
                d if d.is_ascii_digit() => {
                    let mut v: u32 = 0;
                    let mut n = 0;
                    while n < 3 && self.peek().is_ascii_digit() {
                        v = v * 10 + (self.peek() - b'0') as u32;
                        self.i += 1;
                        n += 1;
                    }
                    if v > 255 {
                        return self.err("decimal escape too large");
                    }
                    buf.push(v as u8);
                    continue;
                }
                _ => return self.err("invalid escape sequence"),
            }
            self.i += 1;
        }
    }
}

/// Lua 5.3 style UTF-8 encoding (allows values up to 2^31).
fn utf8_encode(mut x: u32, buf: &mut Vec<u8>) {
    if x < 0x80 {
        buf.push(x as u8);
        return;
    }
    let mut tmp = [0u8; 8];
    let mut n = 1;
    let mut mfb: u32 = 0x3f;
    loop {
        tmp[8 - n] = 0x80 | (x & 0x3f) as u8;
        n += 1;
        x >>= 6;
        mfb >>= 1;
        if x <= mfb {
            break;
        }
    }
    tmp[8 - n] = ((!mfb << 1) | x) as u8;
    buf.extend_from_slice(&tmp[8 - n..]);
}

pub fn parse_number(text: &str) -> Option<Num> {
    let lower = text.to_ascii_lowercase();
    if let Some(h) = lower.strip_prefix("0x") {
        if !h.contains('.') && !h.contains('p') {
            if h.is_empty() {
                return None;
            }
            // Hex integers wrap around modulo 2^64.
            let mut v: u64 = 0;
            for c in h.chars() {
                v = v.wrapping_mul(16).wrapping_add(c.to_digit(16)? as u64);
            }
            return Some(Num::Int(v as i64));
        }
        let (mant, exp) = match h.split_once('p') {
            Some((m, e)) => (m, e.parse::<i32>().ok()?),
            None => (h, 0),
        };
        let (ip, fp) = mant.split_once('.').unwrap_or((mant, ""));
        if ip.is_empty() && fp.is_empty() {
            return None;
        }
        let mut v = 0f64;
        for c in ip.chars() {
            v = v * 16.0 + c.to_digit(16)? as f64;
        }
        let mut scale = 1.0 / 16.0;
        for c in fp.chars() {
            v += c.to_digit(16)? as f64 * scale;
            scale /= 16.0;
        }
        return Some(Num::Float(v * 2f64.powi(exp)));
    }
    if !lower.contains(['.', 'e'])
        && let Ok(i) = lower.parse::<i64>()
    {
        return Some(Num::Int(i));
    }
    if lower.ends_with('e') || lower.ends_with('+') || lower.ends_with('-') {
        return None;
    }
    lower.parse::<f64>().ok().map(Num::Float)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        Lexer::new(s)
            .tokenize()
            .unwrap()
            .into_iter()
            .map(|t| t.tok)
            .collect()
    }

    #[test]
    fn numbers() {
        assert_eq!(parse_number("10"), Some(Num::Int(10)));
        assert_eq!(parse_number("0xff"), Some(Num::Int(255)));
        assert_eq!(parse_number("0xffffffffffffffff"), Some(Num::Int(-1)));
        assert_eq!(parse_number("1e2"), Some(Num::Float(100.0)));
        assert_eq!(parse_number(".5"), Some(Num::Float(0.5)));
        assert_eq!(parse_number("3."), Some(Num::Float(3.0)));
        assert_eq!(parse_number("0x1p4"), Some(Num::Float(16.0)));
        assert_eq!(
            parse_number("9223372036854775808"),
            Some(Num::Float(9223372036854775808.0))
        );
    }

    #[test]
    fn strings_and_comments() {
        let t = toks("--[==[ c ]==] a = 'x\\n\\65\\x41\\u{48}' --tail\n b = [[\nhi]]");
        assert_eq!(
            t,
            vec![
                Tok::Name("a".into()),
                Tok::Sym("="),
                Tok::Str(b"x\nAAH".to_vec()),
                Tok::Name("b".into()),
                Tok::Sym("="),
                Tok::Str(b"hi".to_vec()),
                Tok::Eof
            ]
        );
    }
}
