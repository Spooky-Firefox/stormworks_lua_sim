//! Removes top-level definitions nothing reads, such as helpers from a
//! shared library that this script does not call.

use super::rename::RESERVED_GLOBALS;
use super::resolve::{Resolution, resolve};
use crate::lua::ast::*;

/// Removes unused definitions until none are left; returns how many.
pub fn remove_unused(chunk: &mut Block) -> usize {
    let mut removed = 0;
    loop {
        let res = resolve(chunk);
        if ["_ENV", "_G", "load", "rawget", "rawset", "getfenv"]
            .iter()
            .any(|g| res.globals.contains_key(*g))
        {
            // Globals may be reached by name at runtime.
            return removed;
        }
        let n = prune(chunk, &res);
        if n == 0 {
            return removed;
        }
        removed += n;
    }
}

fn unused_global(res: &Resolution, n: &Name) -> bool {
    n.var.is_none()
        && !RESERVED_GLOBALS.contains(&n.text.as_str())
        && res.globals.get(&n.text).is_some_and(|g| g.reads() == 0)
}

fn unused_local(res: &Resolution, n: &Name) -> bool {
    n.var.is_some_and(|v| res.vars[v].refs == 0)
}

/// Statements at the top of the chunk, including inside top-level `do`
/// blocks (which is where bundled modules end up).
fn prune(b: &mut Block, res: &Resolution) -> usize {
    let mut removed = 0;
    b.retain_mut(|st| {
        let drop = match st {
            Stat::Function(n, fields, _) => fields.is_empty() && unused_global(res, n),
            Stat::LocalFunction(n, _) => unused_local(res, n),
            Stat::Assign(targets, exprs) => {
                targets.len() == exprs.len()
                    && targets
                        .iter()
                        .all(|t| matches!(t, Expr::Name(n) if unused_global(res, n)))
                    && exprs.iter().all(pure)
            }
            Stat::Local(names, exprs) => {
                names.len() == exprs.len()
                    && names.iter().all(|n| unused_local(res, n))
                    && exprs.iter().all(pure)
            }
            Stat::Do(inner) => {
                removed += prune(inner, res);
                inner.is_empty()
            }
            _ => false,
        };
        removed += drop as usize;
        !drop
    });
    removed
}

/// Expressions without side effects. Calls are only allowed into `math`.
fn pure(e: &Expr) -> bool {
    match e {
        Expr::Nil
        | Expr::True
        | Expr::False
        | Expr::Num(_)
        | Expr::Str(_)
        | Expr::Function(_)
        | Expr::Name(_) => true,
        Expr::Table(fields) => fields.iter().all(|f| match f {
            Field::Pos(v) | Field::Named(_, v) => pure(v),
            Field::Keyed(k, v) => pure(k) && pure(v),
        }),
        Expr::Bin(_, a, b) | Expr::Index(a, b) => pure(a) && pure(b),
        Expr::Un(_, a) | Expr::Paren(a) => pure(a),
        Expr::Call(f, args) => {
            matches!(f.as_ref(), Expr::Index(t, _) if matches!(t.as_ref(), Expr::Name(n) if n.text == "math" && n.var.is_none()))
                && args.iter().all(pure)
        }
        Expr::Vararg | Expr::Method(..) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lua::parser::parse;
    use crate::minify::printer::{Style, print_chunk};

    fn run(src: &str) -> String {
        let mut b = parse(src).unwrap();
        remove_unused(&mut b);
        print_chunk(&b, Style::Compact)
    }

    #[test]
    fn removes_unused_chains() {
        assert_eq!(
            run(
                "K = math.tan(1) function helper() return K end function used() return 1 end function onTick() used() end"
            ),
            "function used()return 1 end function onTick()used()end"
        );
        // Bundled module blocks are pruned, and emptied ones disappear.
        assert_eq!(
            run("do function a() end end x = 1 print(x)"),
            "x=1 print(x)"
        );
    }

    #[test]
    fn keeps_side_effects_and_dynamic_access() {
        assert_eq!(
            run("y = f() function onDraw() end"),
            "y=f()function onDraw()end"
        );
        assert_eq!(
            run("function g() end print(_G['g'])"),
            "function g()end print(_G.g)"
        );
        assert_eq!(run("t = {} t.x = 1"), "t={}t.x=1");
        assert_eq!(
            run("local function r() end local q = 1 print(q)"),
            "local q=1 print(q)"
        );
    }
}
