//! Recursive descent parser for Lua 5.3.

use super::ast::*;
use super::lexer::{LexError, Lexer, Pos, Tok, Token};
use std::fmt;

#[derive(Debug, Clone)]
pub struct ParseError {
    pub pos: Pos,
    pub msg: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.pos.line, self.pos.col, self.msg)
    }
}

impl std::error::Error for ParseError {}

impl From<LexError> for ParseError {
    fn from(e: LexError) -> Self {
        ParseError {
            pos: e.pos,
            msg: e.msg,
        }
    }
}

type PResult<T> = Result<T, ParseError>;

pub fn parse(src: &str) -> PResult<Block> {
    let toks = Lexer::new(src).tokenize()?;
    let mut p = Parser { toks, i: 0 };
    let b = p.block()?;
    if p.tok() != &Tok::Eof {
        return p.err(format!("'<eof>' expected near {}", p.tok()));
    }
    Ok(b)
}

struct Parser {
    toks: Vec<Token>,
    i: usize,
}

impl Parser {
    fn tok(&self) -> &Tok {
        &self.toks[self.i].tok
    }

    fn peek_tok(&self) -> &Tok {
        &self.toks[(self.i + 1).min(self.toks.len() - 1)].tok
    }

    fn pos(&self) -> Pos {
        self.toks[self.i].pos
    }

    fn line(&self) -> u32 {
        self.pos().line
    }

    fn advance(&mut self) -> Tok {
        let t = self.toks[self.i].tok.clone();
        if self.i < self.toks.len() - 1 {
            self.i += 1;
        }
        t
    }

    fn err<T>(&self, msg: impl Into<String>) -> PResult<T> {
        Err(ParseError {
            pos: self.pos(),
            msg: msg.into(),
        })
    }

    fn is_sym(&self, s: &str) -> bool {
        matches!(self.tok(), Tok::Sym(x) if *x == s)
    }

    fn is_kw(&self, s: &str) -> bool {
        matches!(self.tok(), Tok::Kw(x) if *x == s)
    }

    fn accept_sym(&mut self, s: &str) -> bool {
        if self.is_sym(s) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn accept_kw(&mut self, s: &str) -> bool {
        if self.is_kw(s) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect_sym(&mut self, s: &str) -> PResult<()> {
        if self.accept_sym(s) {
            Ok(())
        } else {
            self.err(format!("'{s}' expected near {}", self.tok()))
        }
    }

    fn expect_kw(&mut self, s: &str) -> PResult<()> {
        if self.accept_kw(s) {
            Ok(())
        } else {
            self.err(format!("'{s}' expected near {}", self.tok()))
        }
    }

    fn expect_match(&mut self, what: &str, who: &str, line: u32) -> PResult<()> {
        let ok = if what.chars().all(|c| c.is_ascii_alphabetic()) {
            self.accept_kw(what)
        } else {
            self.accept_sym(what)
        };
        if ok {
            Ok(())
        } else if line == self.line() {
            self.err(format!("'{what}' expected near {}", self.tok()))
        } else {
            self.err(format!(
                "'{what}' expected (to close '{who}' at line {line}) near {}",
                self.tok()
            ))
        }
    }

    fn name(&mut self) -> PResult<Name> {
        let line = self.line();
        match self.tok() {
            Tok::Name(_) => match self.advance() {
                Tok::Name(n) => Ok(Name::new(n, line)),
                _ => unreachable!(),
            },
            t => self.err(format!("<name> expected near {t}")),
        }
    }

    fn block_follow(&self, with_until: bool) -> bool {
        match self.tok() {
            Tok::Eof => true,
            Tok::Kw("else" | "elseif" | "end") => true,
            Tok::Kw("until") => with_until,
            _ => false,
        }
    }

    fn block(&mut self) -> PResult<Block> {
        let mut stats = Vec::new();
        while !self.block_follow(true) {
            if self.is_kw("return") {
                self.advance();
                let mut exprs = Vec::new();
                if !self.block_follow(true) && !self.is_sym(";") {
                    exprs = self.exprlist()?;
                }
                self.accept_sym(";");
                stats.push(Stat::Return(exprs));
                if !self.block_follow(true) {
                    return self.err(format!("'<eof>' expected near {}", self.tok()));
                }
                break;
            }
            if let Some(s) = self.statement()? {
                stats.push(s);
            }
        }
        Ok(stats)
    }

    fn statement(&mut self) -> PResult<Option<Stat>> {
        let line = self.line();
        let s = match self.tok().clone() {
            Tok::Sym(";") => {
                self.advance();
                return Ok(None);
            }
            Tok::Kw("if") => self.if_stat(line)?,
            Tok::Kw("while") => {
                self.advance();
                let cond = self.expr()?;
                self.expect_kw("do")?;
                let body = self.block()?;
                self.expect_match("end", "while", line)?;
                Stat::While(cond, body)
            }
            Tok::Kw("do") => {
                self.advance();
                let body = self.block()?;
                self.expect_match("end", "do", line)?;
                Stat::Do(body)
            }
            Tok::Kw("for") => self.for_stat(line)?,
            Tok::Kw("repeat") => {
                self.advance();
                let body = self.block()?;
                self.expect_match("until", "repeat", line)?;
                let cond = self.expr()?;
                Stat::Repeat(body, cond)
            }
            Tok::Kw("function") => {
                self.advance();
                let base = self.name()?;
                let mut fields = Vec::new();
                let mut method = false;
                while self.accept_sym(".") {
                    fields.push(self.name()?.text);
                }
                if self.accept_sym(":") {
                    fields.push(self.name()?.text);
                    method = true;
                }
                let body = self.func_body(method, line)?;
                Stat::Function(base, fields, body)
            }
            Tok::Kw("local") => {
                self.advance();
                if self.accept_kw("function") {
                    let name = self.name()?;
                    let body = self.func_body(false, line)?;
                    Stat::LocalFunction(name, body)
                } else {
                    let mut names = vec![self.name()?];
                    while self.accept_sym(",") {
                        names.push(self.name()?);
                    }
                    let exprs = if self.accept_sym("=") {
                        self.exprlist()?
                    } else {
                        vec![]
                    };
                    Stat::Local(names, exprs)
                }
            }
            Tok::Sym("::") => {
                self.advance();
                let n = self.name()?;
                self.expect_sym("::")?;
                Stat::Label(n.text)
            }
            Tok::Kw("break") => {
                self.advance();
                Stat::Break
            }
            Tok::Kw("goto") => {
                self.advance();
                Stat::Goto(self.name()?.text)
            }
            _ => self.expr_stat()?,
        };
        Ok(Some(s))
    }

    fn if_stat(&mut self, line: u32) -> PResult<Stat> {
        self.advance();
        let mut clauses = Vec::new();
        let cond = self.expr()?;
        self.expect_kw("then")?;
        clauses.push((cond, self.block()?));
        let mut else_block = None;
        loop {
            if self.accept_kw("elseif") {
                let cond = self.expr()?;
                self.expect_kw("then")?;
                clauses.push((cond, self.block()?));
            } else if self.accept_kw("else") {
                else_block = Some(self.block()?);
                self.expect_match("end", "if", line)?;
                break;
            } else {
                self.expect_match("end", "if", line)?;
                break;
            }
        }
        Ok(Stat::If(clauses, else_block))
    }

    fn for_stat(&mut self, line: u32) -> PResult<Stat> {
        self.advance();
        let first = self.name()?;
        if self.accept_sym("=") {
            let start = self.expr()?;
            self.expect_sym(",")?;
            let end = self.expr()?;
            let step = if self.accept_sym(",") {
                Some(self.expr()?)
            } else {
                None
            };
            self.expect_kw("do")?;
            let body = self.block()?;
            self.expect_match("end", "for", line)?;
            return Ok(Stat::NumFor(first, start, end, step, body));
        }
        let mut names = vec![first];
        while self.accept_sym(",") {
            names.push(self.name()?);
        }
        if !self.accept_kw("in") {
            return self.err(format!("'=' or 'in' expected near {}", self.tok()));
        }
        let exprs = self.exprlist()?;
        self.expect_kw("do")?;
        let body = self.block()?;
        self.expect_match("end", "for", line)?;
        Ok(Stat::GenFor(names, exprs, body))
    }

    fn expr_stat(&mut self) -> PResult<Stat> {
        let e = self.suffixed_expr()?;
        if self.is_sym("=") || self.is_sym(",") {
            let mut targets = vec![e];
            while self.accept_sym(",") {
                targets.push(self.suffixed_expr()?);
            }
            self.expect_sym("=")?;
            for t in &targets {
                if !matches!(t, Expr::Name(_) | Expr::Index(..)) {
                    return self.err("syntax error near '='");
                }
            }
            let exprs = self.exprlist()?;
            return Ok(Stat::Assign(targets, exprs));
        }
        if !matches!(e, Expr::Call(..) | Expr::Method(..)) {
            return self.err(format!("syntax error near {}", self.tok()));
        }
        Ok(Stat::Call(e))
    }

    fn func_body(&mut self, method: bool, line: u32) -> PResult<FuncBody> {
        let mut params = Vec::new();
        if method {
            params.push(Name::new("self", self.line()));
        }
        let mut vararg = false;
        self.expect_sym("(")?;
        if !self.is_sym(")") {
            loop {
                if self.accept_sym("...") {
                    vararg = true;
                    break;
                }
                params.push(self.name()?);
                if !self.accept_sym(",") {
                    break;
                }
            }
        }
        self.expect_sym(")")?;
        let body = self.block()?;
        self.expect_match("end", "function", line)?;
        Ok(FuncBody {
            params,
            vararg,
            body,
        })
    }

    fn exprlist(&mut self) -> PResult<Vec<Expr>> {
        let mut v = vec![self.expr()?];
        while self.accept_sym(",") {
            v.push(self.expr()?);
        }
        Ok(v)
    }

    pub fn expr(&mut self) -> PResult<Expr> {
        self.subexpr(0)
    }

    fn subexpr(&mut self, limit: u8) -> PResult<Expr> {
        let unop = match self.tok() {
            Tok::Kw("not") => Some(UnOp::Not),
            Tok::Sym("-") => Some(UnOp::Neg),
            Tok::Sym("#") => Some(UnOp::Len),
            Tok::Sym("~") => Some(UnOp::BNot),
            _ => None,
        };
        let mut e = if let Some(op) = unop {
            self.advance();
            let operand = self.subexpr(UNARY_PRIORITY)?;
            Expr::Un(op, Box::new(operand))
        } else {
            self.simple_expr()?
        };
        loop {
            let op = match self.tok() {
                Tok::Sym(s) | Tok::Kw(s) => BinOp::from_tok(s),
                _ => None,
            };
            let Some(op) = op else { break };
            let (lp, rp) = op.priority();
            if lp <= limit {
                break;
            }
            self.advance();
            let rhs = self.subexpr(rp)?;
            e = Expr::Bin(op, Box::new(e), Box::new(rhs));
        }
        Ok(e)
    }

    fn simple_expr(&mut self) -> PResult<Expr> {
        let line = self.line();
        let e = match self.tok().clone() {
            Tok::Num(n) => Expr::Num(n),
            Tok::Str(s) => Expr::Str(s),
            Tok::Kw("nil") => Expr::Nil,
            Tok::Kw("true") => Expr::True,
            Tok::Kw("false") => Expr::False,
            Tok::Sym("...") => Expr::Vararg,
            Tok::Sym("{") => return self.table(),
            Tok::Kw("function") => {
                self.advance();
                return Ok(Expr::Function(Box::new(self.func_body(false, line)?)));
            }
            _ => return self.suffixed_expr(),
        };
        self.advance();
        Ok(e)
    }

    fn primary_expr(&mut self) -> PResult<Expr> {
        match self.tok().clone() {
            Tok::Name(_) => Ok(Expr::Name(self.name()?)),
            Tok::Sym("(") => {
                let line = self.line();
                self.advance();
                let e = self.expr()?;
                self.expect_match(")", "(", line)?;
                Ok(Expr::Paren(Box::new(e)))
            }
            t => self.err(format!("unexpected symbol near {t}")),
        }
    }

    fn suffixed_expr(&mut self) -> PResult<Expr> {
        let mut e = self.primary_expr()?;
        loop {
            match self.tok().clone() {
                Tok::Sym(".") => {
                    self.advance();
                    let n = self.name()?;
                    e = Expr::Index(Box::new(e), Box::new(Expr::Str(n.text.into_bytes())));
                }
                Tok::Sym("[") => {
                    self.advance();
                    let k = self.expr()?;
                    self.expect_sym("]")?;
                    e = Expr::Index(Box::new(e), Box::new(k));
                }
                Tok::Sym(":") => {
                    self.advance();
                    let n = self.name()?;
                    let args = self.call_args()?;
                    e = Expr::Method(Box::new(e), n.text, args);
                }
                Tok::Sym("(") | Tok::Sym("{") | Tok::Str(_) => {
                    let args = self.call_args()?;
                    e = Expr::Call(Box::new(e), args);
                }
                _ => return Ok(e),
            }
        }
    }

    fn call_args(&mut self) -> PResult<Vec<Expr>> {
        match self.tok().clone() {
            Tok::Str(s) => {
                self.advance();
                Ok(vec![Expr::Str(s)])
            }
            Tok::Sym("{") => Ok(vec![self.table()?]),
            Tok::Sym("(") => {
                let line = self.line();
                self.advance();
                if self.accept_sym(")") {
                    return Ok(vec![]);
                }
                let args = self.exprlist()?;
                self.expect_match(")", "(", line)?;
                Ok(args)
            }
            t => self.err(format!("function arguments expected near {t}")),
        }
    }

    fn table(&mut self) -> PResult<Expr> {
        let line = self.line();
        self.expect_sym("{")?;
        let mut fields = Vec::new();
        while !self.is_sym("}") {
            if self.is_sym("[") {
                self.advance();
                let k = self.expr()?;
                self.expect_sym("]")?;
                self.expect_sym("=")?;
                fields.push(Field::Keyed(k, self.expr()?));
            } else if matches!(self.tok(), Tok::Name(_)) && matches!(self.peek_tok(), Tok::Sym("="))
            {
                let n = self.name()?;
                self.advance();
                fields.push(Field::Named(n.text, self.expr()?));
            } else {
                fields.push(Field::Pos(self.expr()?));
            }
            if !self.accept_sym(",") && !self.accept_sym(";") {
                break;
            }
        }
        self.expect_match("}", "{", line)?;
        Ok(Expr::Table(fields))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_code() {
        let src = r#"
            local a, b = 1, 2
            function t.x:y(z, ...) return self, z end
            for i = 1, 10, 2 do a = a + i end
            for k, v in pairs({1, 2, x = 3, [4] = 5}) do end
            if a then b() elseif c then d"s" else e{} end
            repeat local q = 1 until q
            goto done ::done::
            local f = function() return -a ^ 2 .. "x" end
            print(("x"):rep(3), #t, ~a, a // 2 & 3)
        "#;
        parse(src).unwrap();
    }

    #[test]
    fn rejects_bad_code() {
        assert!(parse("x = ").is_err());
        assert!(parse("f() = 1").is_err());
        assert!(parse("return 1 x = 2").is_err());
        assert!(parse("if x then").is_err());
    }
}
