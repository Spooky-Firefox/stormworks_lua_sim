//! Static checks for things that work in plain Lua but not in a
//! microcontroller, or that are probably typos.

use crate::lua::ast::*;
use crate::minify::resolve::resolve;
use crate::minify::visit::{Ctx, exprs_block};
use std::collections::{BTreeMap, BTreeSet};

pub const API: &[(&str, &[&str])] = &[
    (
        "screen",
        &[
            "setColor",
            "drawClear",
            "drawLine",
            "drawCircle",
            "drawCircleF",
            "drawRect",
            "drawRectF",
            "drawTriangle",
            "drawTriangleF",
            "drawText",
            "drawTextBox",
            "drawMap",
            "setMapColorOcean",
            "setMapColorShallows",
            "setMapColorLand",
            "setMapColorGrass",
            "setMapColorSand",
            "setMapColorSnow",
            "setMapColorRock",
            "setMapColorGravel",
            "getWidth",
            "getHeight",
        ],
    ),
    ("input", &["getNumber", "getBool"]),
    ("output", &["setNumber", "setBool"]),
    ("property", &["getNumber", "getBool", "getText"]),
    ("map", &["screenToMap", "mapToScreen"]),
    ("async", &["httpGet"]),
    ("debug", &["log"]),
];

/// Globals that exist in the game's Lua environment.
const ENV_GLOBALS: &[&str] = &[
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
    "_ENV",
    "_G",
    "_VERSION",
];

const UNAVAILABLE: &[(&str, &str)] = &[
    ("io", "file access is not available in microcontrollers"),
    ("os", "the os library is not available in microcontrollers"),
    ("load", "code loading is not available in microcontrollers"),
    (
        "loadfile",
        "code loading is not available in microcontrollers",
    ),
    (
        "dofile",
        "code loading is not available in microcontrollers",
    ),
    (
        "require",
        "require is only resolved by `swlua build`; inline modules before pasting",
    ),
    (
        "print",
        "print output is not shown in game; use debug.log (visible in DebugView)",
    ),
    (
        "coroutine",
        "coroutines are not available in microcontrollers",
    ),
    (
        "utf8",
        "the utf8 library is not available in microcontrollers",
    ),
];

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Diagnostic {
    pub line: u32,
    pub msg: String,
}

pub fn check(chunk: &Block, allow_require: bool) -> Vec<Diagnostic> {
    let mut chunk = chunk.clone();
    let res = resolve(&mut chunk);
    let mut out = BTreeSet::new();

    let assigned: BTreeSet<&str> = res
        .globals
        .iter()
        .filter(|(_, g)| g.assigned)
        .map(|(n, _)| n.as_str())
        .collect();
    let mut first_use: BTreeMap<String, u32> = BTreeMap::new();
    let mut fields: Vec<(String, String, u32)> = Vec::new();
    collect_names(&mut chunk, &mut first_use);
    exprs_block(&mut chunk, &mut |e, ctx| {
        if let Expr::Index(b, k) = e
            && let (Expr::Name(n), Expr::Str(s)) = (b.as_ref(), k.as_ref())
            && n.var.is_none()
            && ctx == Ctx::Value
        {
            fields.push((n.text.clone(), String::from_utf8_lossy(s).into(), n.line));
        }
        true
    });

    for (name, line) in &first_use {
        if assigned.contains(name.as_str()) {
            continue;
        }
        if let Some((_, why)) = UNAVAILABLE.iter().find(|(n, _)| n == name) {
            if !(allow_require && name == "require") {
                out.insert(Diagnostic {
                    line: *line,
                    msg: format!("'{name}': {why}"),
                });
            }
            continue;
        }
        let known = ENV_GLOBALS.contains(&name.as_str()) || API.iter().any(|(t, _)| t == name);
        if !known {
            out.insert(Diagnostic {
                line: *line,
                msg: format!("global '{name}' is read but never assigned (typo?)"),
            });
        }
    }
    for (table, field, line) in fields {
        if assigned.contains(table.as_str()) {
            continue;
        }
        if let Some((_, members)) = API.iter().find(|(t, _)| *t == table)
            && !members.contains(&field.as_str())
        {
            let hint = members
                .iter()
                .find(|m| m.eq_ignore_ascii_case(&field))
                .map(|m| format!(" (did you mean '{table}.{m}'?)"))
                .unwrap_or_default();
            out.insert(Diagnostic {
                line,
                msg: format!("'{table}.{field}' is not part of the API{hint}"),
            });
        }
    }
    let has = |n: &str| assigned.contains(n);
    if !has("onTick") && !has("onDraw") {
        out.insert(Diagnostic {
            line: 0,
            msg: "script defines neither onTick nor onDraw, so the game never calls it".into(),
        });
    }
    out.into_iter().collect()
}

fn collect_names(chunk: &mut Block, first_use: &mut BTreeMap<String, u32>) {
    crate::minify::visit::names_block(chunk, &mut |n| {
        if n.var.is_none() {
            first_use.entry(n.text.clone()).or_insert(n.line);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lua::parser::parse;

    #[test]
    fn finds_problems() {
        let src = "function onTick() screen.drawrect(1,2,3,4) print(x) local f = io.open end";
        let d = check(&parse(src).unwrap(), false);
        let msgs: Vec<&str> = d.iter().map(|d| d.msg.as_str()).collect();
        assert!(
            msgs.iter()
                .any(|m| m.contains("did you mean 'screen.drawRect'")),
            "{msgs:?}"
        );
        assert!(msgs.iter().any(|m| m.contains("'print'")), "{msgs:?}");
        assert!(
            msgs.iter()
                .any(|m| m.contains("'x' is read but never assigned")),
            "{msgs:?}"
        );
        assert!(msgs.iter().any(|m| m.contains("'io'")), "{msgs:?}");
    }

    #[test]
    fn clean_script() {
        let src = "local t = 0 function onTick() t = t + 1 output.setNumber(1, math.sin(t)) end";
        assert!(check(&parse(src).unwrap(), false).is_empty());
    }
}
