//! Lua 5.3 syntax tree.

use super::lexer::Num;

pub type Block = Vec<Stat>;

/// An identifier occurrence. `var` is filled in by the resolver:
/// `Some(id)` for locals, `None` for globals.
#[derive(Clone, Debug)]
pub struct Name {
    pub text: String,
    pub var: Option<usize>,
    pub line: u32,
}

impl Name {
    pub fn new(text: impl Into<String>, line: u32) -> Self {
        Name {
            text: text.into(),
            var: None,
            line,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FuncBody {
    pub params: Vec<Name>,
    pub vararg: bool,
    pub body: Block,
}

#[derive(Clone, Debug)]
pub enum Stat {
    /// A function or method call used as a statement.
    Call(Expr),
    Assign(Vec<Expr>, Vec<Expr>),
    Local(Vec<Name>, Vec<Expr>),
    LocalFunction(Name, FuncBody),
    /// `function a.b.c(...)`. Method definitions (`a:b`) are desugared by the
    /// parser into a field plus an explicit leading `self` parameter.
    Function(Name, Vec<String>, FuncBody),
    Do(Block),
    While(Expr, Block),
    Repeat(Block, Expr),
    If(Vec<(Expr, Block)>, Option<Block>),
    NumFor(Name, Expr, Expr, Option<Expr>, Block),
    GenFor(Vec<Name>, Vec<Expr>, Block),
    Return(Vec<Expr>),
    Break,
    Goto(String),
    Label(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BinOp {
    Or,
    And,
    Lt,
    Gt,
    Le,
    Ge,
    Ne,
    Eq,
    BOr,
    BXor,
    BAnd,
    Shl,
    Shr,
    Concat,
    Add,
    Sub,
    Mul,
    Div,
    IDiv,
    Mod,
    Pow,
}

impl BinOp {
    pub fn from_tok(s: &str) -> Option<BinOp> {
        use BinOp::*;
        Some(match s {
            "or" => Or,
            "and" => And,
            "<" => Lt,
            ">" => Gt,
            "<=" => Le,
            ">=" => Ge,
            "~=" => Ne,
            "==" => Eq,
            "|" => BOr,
            "~" => BXor,
            "&" => BAnd,
            "<<" => Shl,
            ">>" => Shr,
            ".." => Concat,
            "+" => Add,
            "-" => Sub,
            "*" => Mul,
            "/" => Div,
            "//" => IDiv,
            "%" => Mod,
            "^" => Pow,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        use BinOp::*;
        match self {
            Or => "or",
            And => "and",
            Lt => "<",
            Gt => ">",
            Le => "<=",
            Ge => ">=",
            Ne => "~=",
            Eq => "==",
            BOr => "|",
            BXor => "~",
            BAnd => "&",
            Shl => "<<",
            Shr => ">>",
            Concat => "..",
            Add => "+",
            Sub => "-",
            Mul => "*",
            Div => "/",
            IDiv => "//",
            Mod => "%",
            Pow => "^",
        }
    }

    /// (left, right) binding priority, as in lparser.c.
    pub fn priority(self) -> (u8, u8) {
        use BinOp::*;
        match self {
            Or => (1, 1),
            And => (2, 2),
            Lt | Gt | Le | Ge | Ne | Eq => (3, 3),
            BOr => (4, 4),
            BXor => (5, 5),
            BAnd => (6, 6),
            Shl | Shr => (7, 7),
            Concat => (9, 8),
            Add | Sub => (10, 10),
            Mul | Div | IDiv | Mod => (11, 11),
            Pow => (14, 13),
        }
    }
}

pub const UNARY_PRIORITY: u8 = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UnOp {
    Neg,
    Not,
    Len,
    BNot,
}

impl UnOp {
    pub fn as_str(self) -> &'static str {
        match self {
            UnOp::Neg => "-",
            UnOp::Not => "not",
            UnOp::Len => "#",
            UnOp::BNot => "~",
        }
    }
}

#[derive(Clone, Debug)]
pub enum Field {
    Pos(Expr),
    Named(String, Expr),
    Keyed(Expr, Expr),
}

#[derive(Clone, Debug)]
pub enum Expr {
    Nil,
    True,
    False,
    Vararg,
    Num(Num),
    Str(Vec<u8>),
    Function(Box<FuncBody>),
    Table(Vec<Field>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Un(UnOp, Box<Expr>),
    Name(Name),
    Index(Box<Expr>, Box<Expr>),
    Call(Box<Expr>, Vec<Expr>),
    Method(Box<Expr>, String, Vec<Expr>),
    Paren(Box<Expr>),
}

impl Expr {
    pub fn is_multi(&self) -> bool {
        matches!(self, Expr::Call(..) | Expr::Method(..) | Expr::Vararg)
    }
}
