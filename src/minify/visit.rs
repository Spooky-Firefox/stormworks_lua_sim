//! Small mutable tree walkers.

use crate::lua::ast::*;

/// Calls `f` on every `Name` (declarations and references).
pub fn names_block(b: &mut Block, f: &mut dyn FnMut(&mut Name)) {
    for st in b.iter_mut() {
        names_stat(st, f);
    }
}

fn names_func(fb: &mut FuncBody, f: &mut dyn FnMut(&mut Name)) {
    for p in fb.params.iter_mut() {
        f(p);
    }
    names_block(&mut fb.body, f);
}

fn names_stat(st: &mut Stat, f: &mut dyn FnMut(&mut Name)) {
    match st {
        Stat::LocalFunction(n, fb) | Stat::Function(n, _, fb) => {
            f(n);
            names_func(fb, f);
        }
        Stat::Local(ns, es) => {
            for n in ns.iter_mut() {
                f(n);
            }
            for e in es.iter_mut() {
                names_expr(e, f);
            }
        }
        Stat::GenFor(ns, es, b) => {
            for n in ns.iter_mut() {
                f(n);
            }
            for e in es.iter_mut() {
                names_expr(e, f);
            }
            names_block(b, f);
        }
        Stat::NumFor(n, a, b, c, body) => {
            f(n);
            names_expr(a, f);
            names_expr(b, f);
            if let Some(c) = c {
                names_expr(c, f);
            }
            names_block(body, f);
        }
        _ => {
            for_each_child(st, &mut |c| match c {
                Child::Expr(e) => names_expr(e, f),
                Child::Block(b) => names_block(b, f),
            });
        }
    }
}

fn names_expr(e: &mut Expr, f: &mut dyn FnMut(&mut Name)) {
    match e {
        Expr::Name(n) => f(n),
        Expr::Function(fb) => names_func(fb, f),
        _ => expr_children(e, &mut |c| names_expr(c, f)),
    }
}

/// Visits direct expression and block children of a statement that does not
/// declare names itself.
enum Child<'a> {
    Expr(&'a mut Expr),
    Block(&'a mut Block),
}

fn for_each_child(st: &mut Stat, f: &mut dyn FnMut(Child)) {
    match st {
        Stat::Call(e) => f(Child::Expr(e)),
        Stat::Assign(t, v) => {
            for e in t.iter_mut().chain(v.iter_mut()) {
                f(Child::Expr(e));
            }
        }
        Stat::Do(b) => f(Child::Block(b)),
        Stat::While(c, b) => {
            f(Child::Expr(c));
            f(Child::Block(b));
        }
        Stat::Repeat(b, c) => {
            f(Child::Block(b));
            f(Child::Expr(c));
        }
        Stat::If(cl, els) => {
            for (c, b) in cl.iter_mut() {
                f(Child::Expr(c));
                f(Child::Block(b));
            }
            if let Some(b) = els {
                f(Child::Block(b));
            }
        }
        Stat::Return(es) => {
            for e in es.iter_mut() {
                f(Child::Expr(e));
            }
        }
        Stat::Local(..)
        | Stat::LocalFunction(..)
        | Stat::Function(..)
        | Stat::NumFor(..)
        | Stat::GenFor(..)
        | Stat::Break
        | Stat::Goto(_)
        | Stat::Label(_) => {}
    }
}

/// Calls `f` on each direct child expression.
pub fn expr_children(e: &mut Expr, f: &mut dyn FnMut(&mut Expr)) {
    match e {
        Expr::Nil
        | Expr::True
        | Expr::False
        | Expr::Vararg
        | Expr::Num(_)
        | Expr::Str(_)
        | Expr::Name(_) => {}
        Expr::Function(_) => {}
        Expr::Table(fields) => {
            for fl in fields.iter_mut() {
                match fl {
                    Field::Pos(v) | Field::Named(_, v) => f(v),
                    Field::Keyed(k, v) => {
                        f(k);
                        f(v);
                    }
                }
            }
        }
        Expr::Bin(_, a, b) | Expr::Index(a, b) => {
            f(a);
            f(b);
        }
        Expr::Un(_, a) | Expr::Paren(a) => f(a),
        Expr::Call(c, args) | Expr::Method(c, _, args) => {
            f(c);
            for a in args.iter_mut() {
                f(a);
            }
        }
    }
}

/// Context in which an expression appears; lets rewriters skip assignment
/// targets.
#[derive(Clone, Copy, PartialEq)]
pub enum Ctx {
    Value,
    /// The expression is an assignment target (`x = ...` or `x.y = ...`).
    Target,
}

/// Pre-order walk over every expression, including inside nested functions.
/// The callback returns `false` to skip the children of an expression.
pub fn exprs_block(b: &mut Block, f: &mut dyn FnMut(&mut Expr, Ctx) -> bool) {
    for st in b.iter_mut() {
        exprs_stat(st, f);
    }
}

fn exprs_stat(st: &mut Stat, f: &mut dyn FnMut(&mut Expr, Ctx) -> bool) {
    match st {
        Stat::Assign(t, v) => {
            for e in t.iter_mut() {
                if f(e, Ctx::Target) {
                    // The object being indexed is still read as a value.
                    if let Expr::Index(a, k) = e {
                        exprs_expr(a, f);
                        exprs_expr(k, f);
                    }
                }
            }
            for e in v.iter_mut() {
                exprs_expr(e, f);
            }
        }
        Stat::Local(_, es) => {
            for e in es.iter_mut() {
                exprs_expr(e, f);
            }
        }
        Stat::GenFor(_, es, b) => {
            for e in es.iter_mut() {
                exprs_expr(e, f);
            }
            exprs_block(b, f);
        }
        Stat::NumFor(_, a, b, c, body) => {
            exprs_expr(a, f);
            exprs_expr(b, f);
            if let Some(c) = c {
                exprs_expr(c, f);
            }
            exprs_block(body, f);
        }
        Stat::LocalFunction(_, fb) | Stat::Function(_, _, fb) => exprs_block(&mut fb.body, f),
        _ => for_each_child(st, &mut |c| match c {
            Child::Expr(e) => exprs_expr(e, f),
            Child::Block(b) => exprs_block(b, f),
        }),
    }
}

pub fn exprs_expr(e: &mut Expr, f: &mut dyn FnMut(&mut Expr, Ctx) -> bool) {
    if !f(e, Ctx::Value) {
        return;
    }
    match e {
        Expr::Function(fb) => exprs_block(&mut fb.body, f),
        _ => expr_children(e, &mut |c| exprs_expr(c, f)),
    }
}
