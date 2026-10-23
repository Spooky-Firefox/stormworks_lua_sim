//! Scope resolution: binds every name occurrence to a local variable or a
//! global, and records which locals may not share a name.

use crate::lua::ast::*;
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Default)]
pub struct VarInfo {
    pub name: String,
    pub refs: usize,
    /// Locals that are visible at a point where this one is referenced (or
    /// vice versa), so they must get different names.
    pub conflicts: HashSet<usize>,
    /// Globals referenced while this local is in scope.
    pub globals_in_scope: HashSet<String>,
}

#[derive(Debug, Default, Clone)]
pub struct GlobalInfo {
    pub refs: usize,
    pub assigned: bool,
}

#[derive(Debug, Default)]
pub struct Resolution {
    pub vars: Vec<VarInfo>,
    pub globals: BTreeMap<String, GlobalInfo>,
}

pub fn resolve(block: &mut Block) -> Resolution {
    let mut r = Resolver {
        res: Resolution::default(),
        stack: Vec::new(),
    };
    r.block(block);
    r.res
}

struct Resolver {
    res: Resolution,
    /// Currently visible locals, in declaration order.
    stack: Vec<usize>,
}

impl Resolver {
    fn declare(&mut self, n: &mut Name) {
        let id = self.res.vars.len();
        self.res.vars.push(VarInfo {
            name: n.text.clone(),
            ..Default::default()
        });
        n.var = Some(id);
        self.stack.push(id);
    }

    fn reference(&mut self, n: &mut Name, assign: bool) {
        let found = self
            .stack
            .iter()
            .rposition(|&v| self.res.vars[v].name == n.text);
        match found {
            Some(k) => {
                let v = self.stack[k];
                n.var = Some(v);
                self.res.vars[v].refs += 1;
                for &w in &self.stack[k + 1..] {
                    self.res.vars[v].conflicts.insert(w);
                    self.res.vars[w].conflicts.insert(v);
                }
            }
            None => {
                n.var = None;
                let g = self.res.globals.entry(n.text.clone()).or_default();
                g.refs += 1;
                g.assigned |= assign;
                for &w in &self.stack {
                    self.res.vars[w].globals_in_scope.insert(n.text.clone());
                }
            }
        }
    }

    fn scoped<F: FnOnce(&mut Self)>(&mut self, f: F) {
        let mark = self.stack.len();
        f(self);
        self.stack.truncate(mark);
    }

    fn block(&mut self, b: &mut Block) {
        self.scoped(|s| {
            for st in b.iter_mut() {
                s.stat(st);
            }
        });
    }

    fn func(&mut self, f: &mut FuncBody) {
        self.scoped(|s| {
            for p in f.params.iter_mut() {
                s.declare(p);
            }
            for st in f.body.iter_mut() {
                s.stat(st);
            }
        });
    }

    fn stat(&mut self, st: &mut Stat) {
        match st {
            Stat::Call(e) => self.expr(e),
            Stat::Assign(targets, exprs) => {
                for e in exprs.iter_mut() {
                    self.expr(e);
                }
                for t in targets.iter_mut() {
                    match t {
                        Expr::Name(n) => self.reference(n, true),
                        e => self.expr(e),
                    }
                }
            }
            Stat::Local(names, exprs) => {
                for e in exprs.iter_mut() {
                    self.expr(e);
                }
                for n in names.iter_mut() {
                    self.declare(n);
                }
            }
            Stat::LocalFunction(n, f) => {
                self.declare(n);
                self.func(f);
            }
            Stat::Function(base, fields, f) => {
                self.reference(base, fields.is_empty());
                self.func(f);
            }
            Stat::Do(b) => self.block(b),
            Stat::While(c, b) => {
                self.expr(c);
                self.block(b);
            }
            Stat::Repeat(b, c) => {
                // The condition can see the body's locals.
                self.scoped(|s| {
                    for st in b.iter_mut() {
                        s.stat(st);
                    }
                    s.expr(c);
                });
            }
            Stat::If(clauses, els) => {
                for (c, b) in clauses.iter_mut() {
                    self.expr(c);
                    self.block(b);
                }
                if let Some(b) = els {
                    self.block(b);
                }
            }
            Stat::NumFor(v, a, b, c, body) => {
                self.expr(a);
                self.expr(b);
                if let Some(c) = c {
                    self.expr(c);
                }
                self.scoped(|s| {
                    s.declare(v);
                    s.block(body);
                });
            }
            Stat::GenFor(vars, exprs, body) => {
                for e in exprs.iter_mut() {
                    self.expr(e);
                }
                self.scoped(|s| {
                    for v in vars.iter_mut() {
                        s.declare(v);
                    }
                    s.block(body);
                });
            }
            Stat::Return(exprs) => {
                for e in exprs.iter_mut() {
                    self.expr(e);
                }
            }
            Stat::Break | Stat::Goto(_) | Stat::Label(_) => {}
        }
    }

    fn expr(&mut self, e: &mut Expr) {
        match e {
            Expr::Nil | Expr::True | Expr::False | Expr::Vararg | Expr::Num(_) | Expr::Str(_) => {}
            Expr::Function(f) => self.func(f),
            Expr::Table(fields) => {
                for f in fields.iter_mut() {
                    match f {
                        Field::Pos(v) | Field::Named(_, v) => self.expr(v),
                        Field::Keyed(k, v) => {
                            self.expr(k);
                            self.expr(v);
                        }
                    }
                }
            }
            Expr::Bin(_, a, b) | Expr::Index(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            Expr::Un(_, a) | Expr::Paren(a) => self.expr(a),
            Expr::Name(n) => self.reference(n, false),
            Expr::Call(f, args) | Expr::Method(f, _, args) => {
                self.expr(f);
                for a in args.iter_mut() {
                    self.expr(a);
                }
            }
        }
    }
}
