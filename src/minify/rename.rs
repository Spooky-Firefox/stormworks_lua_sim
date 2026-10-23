//! Assigns short names to locals and (optionally) user globals.

use super::resolve::Resolution;
use crate::lua::ast::*;
use crate::lua::lexer::is_keyword;
use std::collections::{HashMap, HashSet};

/// Globals the game calls or provides; these keep their names.
pub const RESERVED_GLOBALS: &[&str] = &[
    // callbacks
    "onTick",
    "onDraw",
    "httpReply",
    // Stormworks API
    "input",
    "output",
    "property",
    "screen",
    "map",
    "async",
    "debug",
    // standard library available in the game
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
    "print",
    "_ENV",
    "_G",
    "_VERSION",
    "utf8",
    "coroutine",
    "require",
    "load",
    "os",
    "io",
    "collectgarbage",
    "self",
];

/// Generates a, b, ..., Z, _, aa, ab, ... skipping keywords.
pub struct NameGen {
    n: usize,
}

const FIRST: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ_";
const REST: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ_0123456789";

pub fn nth_name(mut n: usize) -> String {
    let mut s = vec![FIRST[n % FIRST.len()]];
    n /= FIRST.len();
    while n > 0 {
        n -= 1;
        s.push(REST[n % REST.len()]);
        n /= REST.len();
    }
    String::from_utf8(s).unwrap()
}

impl NameGen {
    pub fn new() -> Self {
        NameGen { n: 0 }
    }
}

impl Iterator for NameGen {
    type Item = String;
    fn next(&mut self) -> Option<String> {
        loop {
            let s = nth_name(self.n);
            self.n += 1;
            if !is_keyword(&s) {
                return Some(s);
            }
        }
    }
}

pub struct RenameOptions {
    pub locals: bool,
    pub globals: bool,
}

/// Returns new names for locals (by var id) and globals (by original name).
pub fn plan(res: &Resolution, opts: &RenameOptions) -> (Vec<String>, HashMap<String, String>) {
    let reserved: HashSet<&str> = RESERVED_GLOBALS.iter().copied().collect();
    let dynamic_env = res.globals.contains_key("_ENV")
        || res.globals.contains_key("_G")
        || res.globals.contains_key("load")
        || res.globals.contains_key("rawget")
        || res.globals.contains_key("rawset");

    // Globals: rename those defined by the script itself.
    let mut global_map: HashMap<String, String> = HashMap::new();
    let kept: HashSet<String> = res
        .globals
        .iter()
        .filter(|(n, g)| {
            !opts.globals || dynamic_env || !g.assigned || reserved.contains(n.as_str())
        })
        .map(|(n, _)| n.clone())
        .collect();
    if opts.globals && !dynamic_env {
        let mut renamable: Vec<(&String, usize)> = res
            .globals
            .iter()
            .filter(|(n, _)| !kept.contains(*n))
            .map(|(n, g)| (n, g.refs))
            .collect();
        renamable.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        // Without local renaming, new global names must not collide with locals.
        let local_names: HashSet<&str> = if opts.locals {
            HashSet::new()
        } else {
            res.vars.iter().map(|v| v.name.as_str()).collect()
        };
        let mut names =
            NameGen::new().filter(|n| !kept.contains(n) && !local_names.contains(n.as_str()));
        for (g, _) in renamable {
            global_map.insert(g.clone(), names.next().unwrap());
        }
    }
    let final_global = |g: &str| global_map.get(g).cloned().unwrap_or_else(|| g.to_string());

    // Locals: greedy colouring, most referenced first.
    let mut order: Vec<usize> = (0..res.vars.len()).collect();
    order.sort_by(|&a, &b| res.vars[b].refs.cmp(&res.vars[a].refs).then(a.cmp(&b)));
    let mut out: Vec<Option<String>> = vec![None; res.vars.len()];
    for v in order {
        let info = &res.vars[v];
        if !opts.locals {
            out[v] = Some(info.name.clone());
            continue;
        }
        let forbidden: HashSet<String> = info
            .globals_in_scope
            .iter()
            .map(|g| final_global(g))
            .chain(info.conflicts.iter().filter_map(|w| out[*w].clone()))
            .collect();
        let name = NameGen::new().find(|n| !forbidden.contains(n)).unwrap();
        out[v] = Some(name);
    }
    (out.into_iter().map(Option::unwrap).collect(), global_map)
}

/// Rewrites every name in the tree according to a plan.
pub fn apply(b: &mut Block, locals: &[String], globals: &HashMap<String, String>) {
    let mut f = |n: &mut Name| match n.var {
        Some(v) => n.text = locals[v].clone(),
        None => {
            if let Some(g) = globals.get(&n.text) {
                n.text = g.clone();
            }
        }
    };
    super::visit::names_block(b, &mut f);
}
