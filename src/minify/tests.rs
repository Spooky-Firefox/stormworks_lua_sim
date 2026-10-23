//! Minified code must behave exactly like the original.

use super::*;
use mlua::{Lua, Variadic};
use std::cell::RefCell;
use std::rc::Rc;

fn minify(src: &str, opts: &Options) -> Result<String, crate::lua::parser::ParseError> {
    Ok(minify_ast(&mut crate::lua::parser::parse(src)?, opts))
}

fn run(src: &str) -> String {
    let lua = Lua::new();
    let out = Rc::new(RefCell::new(String::new()));
    let o = out.clone();
    let print = lua
        .create_function(move |_, args: Variadic<mlua::Value>| {
            let parts: Vec<String> = args
                .iter()
                .map(|v| v.to_string().unwrap_or_else(|_| "?".into()))
                .collect();
            let mut o = o.borrow_mut();
            o.push_str(&parts.join("\t"));
            o.push('\n');
            Ok(())
        })
        .unwrap();
    lua.globals().set("print", print).unwrap();
    // Fake Stormworks tables so API hoisting can be exercised.
    lua.load(
        r#"
        screen = {drawRect = function(...) print("rect", ...) end, drawText = function(...) print("text", ...) end}
        input = {getNumber = function(i) return i * 1.5 end}
        "#,
    )
    .exec()
    .unwrap();
    if let Err(e) = lua.load(src).exec() {
        out.borrow_mut().push_str(&format!(
            "error: {}\n",
            e.to_string().lines().next().unwrap_or("")
        ));
    }

    out.borrow().clone()
}

fn check(src: &str) {
    let expected = run(src);
    for opts in [
        Options::default(),
        Options::whitespace_only(),
        Options {
            style: Style::Pretty,
            ..Default::default()
        },
    ] {
        let min = minify(src, &opts).unwrap_or_else(|e| panic!("parse failed: {e}"));
        let got = run(&min);
        // Error messages contain chunk positions that legitimately differ.
        let norm = |s: &str| {
            s.lines()
                .map(|l| if l.starts_with("error:") { "error" } else { l })
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert_eq!(
            norm(&expected),
            norm(&got),
            "\n--- original ---\n{src}\n--- minified ({opts:?}) ---\n{min}\n"
        );
    }
}

#[test]
fn scoping_and_shadowing() {
    check(
        r#"
        local x = 1
        local function f(x) return x + 1 end
        do local x = x + 10; print(x) end
        print(x, f(x))
        local a, b = 1
        local a = a or 5
        print(a, b)
        for i = 1, 3 do local i = i * 2; print(i) end
        for k, v in ipairs({4, 5, 6}) do print(k, v) end
        repeat local done = true until done
        local n = 0
        while n < 3 do n = n + 1 end
        print(n)
        local t = {}
        function t.add(a, b) return a + b end
        function t:mul(a) return self.k * a end
        t.k = 7
        print(t.add(2, 3), t:mul(3))
        local function rec(n) if n <= 0 then return 0 end return n + rec(n - 1) end
        print(rec(10))
        "#,
    );
}

#[test]
fn closures_and_upvalues() {
    check(
        r#"
        local fs = {}
        for i = 1, 3 do fs[i] = function() return i end end
        print(fs[1](), fs[2](), fs[3]())
        local function counter()
            local c = 0
            return function() c = c + 1; return c end
        end
        local c1, c2 = counter(), counter()
        c1(); c1()
        print(c1(), c2())
        local function va(...) local a, b = ...; return select('#', ...), a, b, (...) end
        print(va(1, 2, 3))
        print((va(1, 2)))
        "#,
    );
}

#[test]
fn globals_are_renamed_consistently() {
    check(
        r#"
        counter = 0
        function bump(n) counter = counter + (n or 1) end
        local function get() return counter end
        bump() bump(5)
        print(get(), counter)
        function onTick() bump() end
        onTick()
        print(counter)
        "#,
    );
}

#[test]
fn operators_and_literals() {
    check(
        r#"
        local a, b = 7, 2
        print(a // b, a / b, a % b, a ^ b, -a ^ 2, (-a) ^ 2, 2 ^ 3 ^ 2, (2 ^ 3) ^ 2)
        print(a & b, a | b, a ~ b, ~a, a << 2, a >> 1)
        print(1 .. 2, "a" .. "b" .. "c", ("x"):rep(3))
        print(a == b, a ~= b, a < b, not a, #"abc", - -a)
        print(1e6, 1000000, 0.5, 3.0, 0x10, 0xff, 1e300 * 10, -(0/0) ~= -(0/0))
        print(math.type(3.0), math.type(3), 2^53, 9007199254740993, 0xffffffffffffffff)
        print("quote's", 'dq"', "\0\1\2" == "\0\1\2", #"\0001", "tab\tnew\nline", "\\")
        print(a - -b, a+-b, 10 - 2 - 3, 10 - (2 - 3))
        local s = "x"
        print(s:upper(), s:rep(2, ","), ("%d"):format(5))
        print(1 < 2 == true, (1 < 2) == true, "a" < "b")
        "#,
    );
}

#[test]
fn tables_and_calls() {
    check(
        r#"
        local t = {1, 2, 3, x = 1, ["y z"] = 2, [10] = 3, ["end"] = 4, f = function(s) return s.x end}
        print(#t, t.x, t["y z"], t[10], t["end"], t:f())
        local function multi() return 1, 2, 3 end
        local u = {multi()}
        local v = {multi(), multi()}
        local w = {(multi())}
        print(#u, #v, #w)
        print(multi(), (multi()))
        print(type(print), tostring(nil), tonumber("0x10"))
        local str = "abc"
        print(str:len(), #str, string.format("%5.2f", 3.14159))
        "#,
    );
}

#[test]
fn statement_ambiguity() {
    check(
        r#"
        local f = function(x) return x end
        local a = f
        (print)("called")
        local t = {}
        t.x = 1;
        (t).y = 2
        print(t.x, t.y)
        goto skip
        print("not printed")
        ::skip::
        for i = 1, 3 do
            if i == 2 then goto continue end
            print(i)
            ::continue::
        end
        "#,
    );
}

#[test]
fn api_hoisting() {
    check(
        r#"
        function onDraw()
            screen.drawRect(1, 2, 3, 4)
            screen.drawRect(5, 6, 7, 8)
            screen.drawRect(9, 10, 11, 12)
            screen.drawText(1, 1, "hello world")
            screen.drawText(1, 7, "hello world")
            screen.drawText(1, 14, "hello world")
            print(input.getNumber(1), input.getNumber(2), input.getNumber(3))
            print(math.floor(1.5), math.floor(2.5), math.floor(-1.5), math.pi, math.pi, math.pi)
            local m = math
            print(m.max(1, 2), 1000.5, 1000.5, 1000.5, 1000.5)
        end
        onDraw()
        "#,
    );
    let min = minify(
        "screen.drawRect(1,2,3,4) screen.drawRect(1,2,3,4) screen.drawRect(1,2,3,4)",
        &Options::default(),
    )
    .unwrap();
    assert_eq!(
        min,
        "local a=screen.drawRect a(1,2,3,4)a(1,2,3,4)a(1,2,3,4)"
    );
}

#[test]
fn assigned_api_is_not_hoisted() {
    check(
        r#"
        screen.drawRect(1, 1, 1, 1) screen.drawRect(2, 2, 2, 2) screen.drawRect(3, 3, 3, 3)
        screen.drawRect = function() print("patched") end
        screen.drawRect(4, 4, 4, 4)
        "#,
    );
}

#[test]
fn many_locals_get_unique_names() {
    let mut src = String::new();
    for i in 0..150 {
        src.push_str(&format!("local v{i} = {i}\n"));
    }
    src.push_str("print(");
    for i in 0..150 {
        if i > 0 {
            src.push(',');
        }
        src.push_str(&format!("v{i}"));
    }
    src.push_str(")\n");
    check(&src);
}

#[test]
fn shadowed_names_avoid_globals() {
    check(
        r#"
        local function show(...) print(...) end
        a = 5
        local b = 3
        local function inner() local c = 2; return a + b + c end
        show(inner(), a, b)
        "#,
    );
}

/// Random expression trees, printed fully parenthesised, must evaluate the
/// same after minification (which drops every parenthesis it can).
#[test]
fn random_expressions_keep_precedence() {
    struct Rng(u64);
    impl Rng {
        fn next(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n
        }
    }
    fn expr(r: &mut Rng, depth: u32) -> String {
        if depth == 0 || r.next(4) == 0 {
            return ["1", "2", "3", "a", "b", "2.5", "0.5", "-1", "c"][r.next(9) as usize]
                .to_string();
        }
        if r.next(6) == 0 {
            let op = ["- ", "not ", "~ "][r.next(3) as usize];
            return format!("({op}{})", expr(r, depth - 1));
        }
        let ops = [
            "+", "-", "*", "/", "//", "%", "^", "..", "==", "<", "<=", "~=", "and", "or", "&", "|",
            "~", "<<", ">>",
        ];
        let op = ops[r.next(ops.len() as u64) as usize];
        format!("({} {op} {})", expr(r, depth - 1), expr(r, depth - 1))
    }
    let mut r = Rng(0x9E3779B97F4A7C15);
    let mut src = String::from(
        "local a, b, c = 7, 3, 4\nlocal function p(f) local ok, v = pcall(f) print(ok and tostring(v) or 'err') end\n",
    );
    for _ in 0..400 {
        src.push_str(&format!("p(function() return {} end)\n", expr(&mut r, 5)));
    }
    check(&src);
}
