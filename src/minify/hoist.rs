//! Caches frequently used API functions, globals and constants in locals at
//! the top of the chunk, e.g. `screen.drawRect` -> `local a=screen.drawRect`.

use super::printer::{format_number, is_identifier, quote_string};
use super::resolve::Resolution;
use super::visit::{Ctx, exprs_block};
use crate::lua::ast::*;
use crate::lua::lexer::Num;
use std::collections::{HashMap, HashSet};

/// Global tables whose fields are stable for the lifetime of the script.
const API_TABLES: &[&str] = &[
    "screen", "input", "output", "property", "map", "async", "math", "string", "table",
];

/// Read-only globals that are safe to capture at load time.
const API_GLOBALS: &[&str] = &[
    "screen",
    "input",
    "output",
    "property",
    "map",
    "async",
    "math",
    "string",
    "table",
    "pairs",
    "ipairs",
    "next",
    "tostring",
    "tonumber",
    "type",
    "select",
    "setmetatable",
    "getmetatable",
    "rawget",
    "rawset",
    "rawequal",
    "rawlen",
    "error",
    "assert",
    "pcall",
    "xpcall",
    "unpack",
];

/// Leave headroom below Lua's 200 local / 255 upvalue limits.
const MAX_HOISTS: usize = 100;

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum Key {
    Field(String, String),
    Global(String),
    Str(Vec<u8>),
    Num(String),
}

impl Key {
    fn printed_len(&self) -> usize {
        match self {
            Key::Field(g, k) => g.len() + 1 + k.len(),
            Key::Global(g) => g.len(),
            Key::Str(s) => quote_string(s).len(),
            Key::Num(n) => n.len(),
        }
    }

    fn to_expr(&self) -> Expr {
        match self {
            Key::Field(g, k) => Expr::Index(
                Box::new(Expr::Name(Name::new(g.clone(), 0))),
                Box::new(Expr::Str(k.clone().into_bytes())),
            ),
            Key::Global(g) => Expr::Name(Name::new(g.clone(), 0)),
            Key::Str(s) => Expr::Str(s.clone()),
            Key::Num(n) => Expr::Num(crate::lua::lexer::parse_number(n).unwrap()),
        }
    }
}

pub struct HoistOptions {
    pub api: bool,
    pub constants: bool,
}

/// Returns the number of hoisted values.
pub fn hoist(chunk: &mut Block, res: &Resolution, opts: &HoistOptions) -> usize {
    let readonly = |g: &str| res.globals.get(g).is_some_and(|i| !i.assigned);
    let existing_locals = res.vars.len();

    // Pass 1: API table fields. Tables the script writes into are skipped.
    let mut patched: HashSet<String> = HashSet::new();
    exprs_block(chunk, &mut |e, ctx| {
        if let (Ctx::Target, Expr::Index(b, _)) = (ctx, &*e)
            && let Expr::Name(n) = b.as_ref()
            && n.var.is_none()
        {
            patched.insert(n.text.clone());
        }
        true
    });
    let readonly = |g: &str| readonly(g) && !patched.contains(g);
    let mut fields: HashMap<Key, usize> = HashMap::new();
    if opts.api {
        exprs_block(chunk, &mut |e, ctx| {
            if let Some(k) = field_key(e, &readonly) {
                if ctx == Ctx::Value {
                    *fields.entry(k).or_default() += 1;
                }
                return ctx == Ctx::Value;
            }
            true
        });
    }
    let field_set = select(fields, existing_locals, MAX_HOISTS);

    // Pass 2: bare globals and literals not already covered by pass 1.
    let mut rest: HashMap<Key, usize> = HashMap::new();
    walk(chunk, &mut |e, ctx| {
        if ctx == Ctx::Target {
            return true;
        }
        if field_key(e, &readonly).is_some_and(|k| field_set.contains(&k)) {
            return false;
        }
        if let Some(k) = value_key(e, &readonly, opts) {
            *rest.entry(k).or_default() += 1;
        }
        true
    });
    let rest_set = select(
        rest,
        existing_locals + field_set.len(),
        MAX_HOISTS - field_set.len(),
    );

    let mut all: Vec<Key> = field_set.into_iter().chain(rest_set).collect();
    if all.is_empty() {
        return 0;
    }
    all.sort_by_key(|k| format!("{k:?}"));
    let names: HashMap<Key, String> = all
        .iter()
        .enumerate()
        .map(|(i, k)| (k.clone(), format!("\u{1}h{i}")))
        .collect();

    // Pass 3: rewrite.
    walk(chunk, &mut |e, ctx| {
        if ctx == Ctx::Target {
            return true;
        }
        let key = field_key(e, &readonly)
            .filter(|k| names.contains_key(k))
            .or_else(|| value_key(e, &readonly, opts));
        if let Some(n) = key.and_then(|k| names.get(&k)) {
            *e = Expr::Name(Name::new(n.clone(), 0));
            return false;
        }
        true
    });
    let decl_names = all.iter().map(|k| Name::new(names[k].clone(), 0)).collect();
    let decl_exprs = all.iter().map(Key::to_expr).collect();
    chunk.insert(0, Stat::Local(decl_names, decl_exprs));
    all.len()
}

/// Like `exprs_block`, but string keys that print as `.name` or `name=` are
/// not visited: they are not literals in the output.
fn walk(b: &mut Block, f: &mut dyn FnMut(&mut Expr, Ctx) -> bool) {
    exprs_block(b, &mut |e, ctx| {
        walk_expr(e, ctx, f);
        false
    });
}

fn walk_expr(e: &mut Expr, ctx: Ctx, f: &mut dyn FnMut(&mut Expr, Ctx) -> bool) {
    if !f(e, ctx) {
        return;
    }
    match e {
        Expr::Function(fb) => walk(&mut fb.body, f),
        Expr::Index(base, key) => {
            walk_expr(base, Ctx::Value, f);
            if !matches!(key.as_ref(), Expr::Str(s) if is_identifier(s)) {
                walk_expr(key, Ctx::Value, f);
            }
        }
        Expr::Table(fields) => {
            for fl in fields.iter_mut() {
                match fl {
                    Field::Pos(v) | Field::Named(_, v) => walk_expr(v, Ctx::Value, f),
                    Field::Keyed(k, v) => {
                        if !matches!(k, Expr::Str(s) if is_identifier(s)) {
                            walk_expr(k, Ctx::Value, f);
                        }
                        walk_expr(v, Ctx::Value, f);
                    }
                }
            }
        }
        _ if ctx == Ctx::Target => {}
        _ => super::visit::expr_children(e, &mut |c| walk_expr(c, Ctx::Value, f)),
    }
}

fn field_key(e: &Expr, readonly: &dyn Fn(&str) -> bool) -> Option<Key> {
    let Expr::Index(base, key) = e else {
        return None;
    };
    let (Expr::Name(n), Expr::Str(k)) = (base.as_ref(), key.as_ref()) else {
        return None;
    };
    (n.var.is_none()
        && API_TABLES.contains(&n.text.as_str())
        && readonly(&n.text)
        && is_identifier(k))
    .then(|| Key::Field(n.text.clone(), String::from_utf8(k.clone()).unwrap()))
}

fn value_key(e: &Expr, readonly: &dyn Fn(&str) -> bool, opts: &HoistOptions) -> Option<Key> {
    match e {
        Expr::Name(n)
            if opts.api
                && n.var.is_none()
                && API_GLOBALS.contains(&n.text.as_str())
                && readonly(&n.text) =>
        {
            Some(Key::Global(n.text.clone()))
        }
        Expr::Str(s) if opts.constants => Some(Key::Str(s.clone())),
        Expr::Num(n) if opts.constants => match n {
            Num::Int(i) if *i < 0 => None,
            Num::Float(f) if f.is_sign_negative() || !f.is_finite() => None,
            n => Some(Key::Num(format_number(n))),
        },
        _ => None,
    }
}

fn select(counts: HashMap<Key, usize>, existing: usize, cap: usize) -> HashSet<Key> {
    // Rough length of the names the renamer will hand out.
    let name_len = if existing + counts.len() < 50 { 1 } else { 2 };
    let mut cands: Vec<(Key, isize)> = counts
        .into_iter()
        .filter_map(|(k, n)| {
            let l = k.printed_len() as isize;
            let n = n as isize;
            let saving = n * (l - name_len) - (l + name_len + 2);
            (saving > 0).then_some((k, saving))
        })
        .collect();
    cands.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| format!("{:?}", a.0).cmp(&format!("{:?}", b.0)))
    });
    cands.into_iter().take(cap).map(|(k, _)| k).collect()
}
