//! Microcontroller simulator: runs a script against emulated composite
//! inputs/outputs, properties, a monitor and `async.httpGet`.

pub mod font;
pub mod screen;

use anyhow::{Context, Result, anyhow};
use mlua::{Function, HookTriggers, Lua, LuaOptions, StdLib, Table, Value, Variadic, VmState};
use screen::{Color, Screen};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

pub const CHANNELS: usize = 32;

/// Longest a single callback may run before it is treated as hung.
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq)]
pub enum PropValue {
    Number(f64),
    Bool(bool),
    Text(String),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Touch {
    pub x: f64,
    pub y: f64,
    pub down: bool,
}

/// State shared between the simulator and the Lua callbacks.
pub struct State {
    pub in_num: [f64; CHANNELS],
    pub in_bool: [bool; CHANNELS],
    pub out_num: [f64; CHANNELS],
    pub out_bool: [bool; CHANNELS],
    /// Constant inputs from the config; applied first every tick.
    pub base_num: HashMap<usize, f64>,
    pub base_bool: HashMap<usize, bool>,
    /// Values held from the debugger UI; applied last, so they win over the
    /// config, touch data and the bench.
    pub held_num: HashMap<usize, f64>,
    pub held_bool: HashMap<usize, bool>,
    pub props: HashMap<String, PropValue>,
    pub screen: Option<Screen>,
    pub drawing: bool,
    pub touch: [Touch; 2],
    pub touch_enabled: bool,
    pub logs: Vec<String>,
    pub http_requests: Vec<(u16, String)>,
    pub stop_requested: bool,
    callback_start: Option<Instant>,
}

pub enum Source {
    /// Run a file directly; `require` loads modules from the search path.
    File { main: PathBuf, search: Vec<PathBuf> },
    /// Run already bundled code (e.g. the minified output).
    Code { name: String, code: String },
}

pub struct SimOptions {
    pub screen: Option<(usize, usize)>,
    pub touch: bool,
    pub props: HashMap<String, PropValue>,
    pub numbers: HashMap<usize, f64>,
    pub bools: HashMap<usize, bool>,
    pub bench: Option<PathBuf>,
}

pub struct Sim {
    lua: Lua,
    bench: Option<Lua>,
    pub st: Rc<RefCell<State>>,
    pub tick: u64,
    /// Set when the script raised an error; the game stops the script then.
    pub error: Option<String>,
    http_tx: Sender<(u16, String, String)>,
    http_rx: Receiver<(u16, String, String)>,
    /// Files the running script was loaded from (for reload-on-change).
    pub files: Rc<RefCell<Vec<PathBuf>>>,
}

fn lerr(e: mlua::Error) -> anyhow::Error {
    anyhow!("{e}")
}

fn arg_f64(v: Option<f64>) -> f64 {
    v.unwrap_or(0.0)
}

fn channel_index(ch: f64) -> Option<usize> {
    let i = ch.floor();
    (1.0..=CHANNELS as f64).contains(&i).then(|| i as usize - 1)
}

fn to_text(lua: &Lua, v: Value) -> mlua::Result<String> {
    Ok(match lua.coerce_string(v.clone())? {
        Some(s) => s.to_string_lossy().to_string(),
        None => match v {
            Value::Nil => "nil".into(),
            Value::Boolean(b) => b.to_string(),
            other => other.to_string()?.to_string(),
        },
    })
}

/// Lets bench API functions be called as `sim:fn()` or `sim.fn()`.
fn strip_self(mut args: Variadic<Value>) -> Variadic<Value> {
    if matches!(args.first(), Some(Value::Table(_))) {
        args.remove(0);
    }
    args
}

impl Sim {
    pub fn new(source: Source, opts: SimOptions) -> Result<Sim> {
        let st = Rc::new(RefCell::new(State {
            in_num: [0.0; CHANNELS],
            in_bool: [false; CHANNELS],
            out_num: [0.0; CHANNELS],
            out_bool: [false; CHANNELS],
            base_num: opts
                .numbers
                .into_iter()
                .filter(|(k, _)| (1..=CHANNELS).contains(k))
                .map(|(k, v)| (k - 1, v))
                .collect(),
            base_bool: opts
                .bools
                .into_iter()
                .filter(|(k, _)| (1..=CHANNELS).contains(k))
                .map(|(k, v)| (k - 1, v))
                .collect(),
            held_num: HashMap::new(),
            held_bool: HashMap::new(),
            props: opts.props,
            screen: opts.screen.map(|(w, h)| Screen::new(w, h)),
            drawing: false,
            touch: [Touch::default(); 2],
            touch_enabled: opts.touch && opts.screen.is_some(),
            logs: Vec::new(),
            http_requests: Vec::new(),
            stop_requested: false,
            callback_start: None,
        }));
        let (http_tx, http_rx) = channel();
        let lua = Lua::new_with(
            StdLib::MATH | StdLib::STRING | StdLib::TABLE,
            LuaOptions::default(),
        )
        .map_err(lerr)?;
        let mut sim = Sim {
            lua,
            bench: None,
            st,
            tick: 0,
            error: None,
            http_tx,
            http_rx,
            files: Rc::new(RefCell::new(vec![])),
        };
        sim.install_api().map_err(lerr).context("installing API")?;

        let (name, code) = match &source {
            Source::File { main, search } => {
                sim.install_require(main, search.clone()).map_err(lerr)?;
                sim.files.borrow_mut().push(main.clone());
                (
                    format!("@{}", main.display()),
                    std::fs::read_to_string(main)
                        .with_context(|| format!("reading {}", main.display()))?,
                )
            }
            Source::Code { name, code } => (format!("={name}"), code.clone()),
        };

        if let Some(bench) = &opts.bench {
            sim.bench = Some(sim.make_bench(bench).map_err(lerr)?);
            sim.files.borrow_mut().push(bench.clone());
            sim.call_bench("onSimInit", ())
                .map_err(|e| anyhow!("bench onSimInit: {e}"))?;
        }

        sim.guarded(|lua| lua.load(&code).set_name(name).exec())
            .map_err(|e| anyhow!("{e}"))?;
        Ok(sim)
    }

    /// Runs Lua with the hang detector armed.
    fn guarded<R>(&self, f: impl FnOnce(&Lua) -> mlua::Result<R>) -> mlua::Result<R> {
        self.st.borrow_mut().callback_start = Some(Instant::now());
        let r = f(&self.lua);
        self.st.borrow_mut().callback_start = None;
        r
    }

    fn install_api(&mut self) -> mlua::Result<()> {
        let lua = &self.lua;
        let g = lua.globals();
        // The game's environment has no file, OS or code loading access.
        for name in ["dofile", "loadfile", "load", "collectgarbage", "require"] {
            g.raw_set(name, Value::Nil)?;
        }

        let st = self.st.clone();
        lua.set_hook(
            HookTriggers::new().every_nth_instruction(10_000),
            move |_, _| {
                let start = st.borrow().callback_start;
                match start {
                    Some(t) if t.elapsed() > CALLBACK_TIMEOUT => {
                        Err(mlua::Error::runtime(format!(
                            "script ran for over {}s in one callback (infinite loop?)",
                            CALLBACK_TIMEOUT.as_secs()
                        )))
                    }
                    _ => Ok(VmState::Continue),
                }
            },
        )?;

        // input
        let input = lua.create_table()?;
        let st = self.st.clone();
        input.set(
            "getNumber",
            lua.create_function(move |_, ch: f64| {
                Ok(channel_index(ch).map_or(0.0, |i| st.borrow().in_num[i]))
            })?,
        )?;
        let st = self.st.clone();
        input.set(
            "getBool",
            lua.create_function(move |_, ch: f64| {
                Ok(channel_index(ch).is_some_and(|i| st.borrow().in_bool[i]))
            })?,
        )?;
        g.set("input", input)?;

        // output
        let output = lua.create_table()?;
        let st = self.st.clone();
        output.set(
            "setNumber",
            lua.create_function(move |_, (ch, v): (f64, Option<f64>)| {
                if let Some(i) = channel_index(ch) {
                    st.borrow_mut().out_num[i] = v.unwrap_or(0.0);
                }
                Ok(())
            })?,
        )?;
        let st = self.st.clone();
        output.set(
            "setBool",
            lua.create_function(move |_, (ch, v): (f64, Value)| {
                if let Some(i) = channel_index(ch) {
                    st.borrow_mut().out_bool[i] = !matches!(v, Value::Nil | Value::Boolean(false));
                }
                Ok(())
            })?,
        )?;
        g.set("output", output)?;

        // property
        let property = lua.create_table()?;
        let st = self.st.clone();
        property.set(
            "getNumber",
            lua.create_function(move |_, label: String| {
                Ok(match st.borrow().props.get(&label) {
                    Some(PropValue::Number(n)) => *n,
                    Some(PropValue::Text(t)) => t.trim().parse().unwrap_or(0.0),
                    _ => 0.0,
                })
            })?,
        )?;
        let st = self.st.clone();
        property.set(
            "getBool",
            lua.create_function(move |_, label: String| {
                Ok(matches!(
                    st.borrow().props.get(&label),
                    Some(PropValue::Bool(true))
                ))
            })?,
        )?;
        let st = self.st.clone();
        property.set(
            "getText",
            lua.create_function(move |_, label: String| {
                Ok(match st.borrow().props.get(&label) {
                    Some(PropValue::Text(t)) => t.clone(),
                    Some(PropValue::Number(n)) => n.to_string(),
                    Some(PropValue::Bool(b)) => b.to_string(),
                    None => String::new(),
                })
            })?,
        )?;
        g.set("property", property)?;

        g.set("screen", self.screen_table()?)?;

        // map
        let map = lua.create_table()?;
        map.set(
            "screenToMap",
            lua.create_function(|_, a: (f64, f64, f64, f64, f64, f64, f64)| {
                Ok(screen::screen_to_map(a.0, a.1, a.2, a.3, a.4, a.5, a.6))
            })?,
        )?;
        map.set(
            "mapToScreen",
            lua.create_function(|_, a: (f64, f64, f64, f64, f64, f64, f64)| {
                Ok(screen::map_to_screen(a.0, a.1, a.2, a.3, a.4, a.5, a.6))
            })?,
        )?;
        g.set("map", map)?;

        // async
        let asy = lua.create_table()?;
        let st = self.st.clone();
        let tx = self.http_tx.clone();
        asy.set(
            "httpGet",
            lua.create_function(move |_, (port, url): (f64, String)| {
                let port = port as u16;
                st.borrow_mut().http_requests.push((port, url.clone()));
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let body = http_get(port, &url).unwrap_or_else(|e| format!("connect(): {e}"));
                    let _ = tx.send((port, url, body));
                });
                Ok(())
            })?,
        )?;
        g.set("async", asy)?;

        // debug.log and print both go to the simulator log.
        let debug = lua.create_table()?;
        let st = self.st.clone();
        debug.set(
            "log",
            lua.create_function(move |lua, v: Value| {
                let s = to_text(lua, v)?;
                st.borrow_mut().logs.push(s);
                Ok(())
            })?,
        )?;
        g.set("debug", debug)?;
        let st = self.st.clone();
        g.set(
            "print",
            lua.create_function(move |lua, args: Variadic<Value>| {
                let parts: mlua::Result<Vec<String>> =
                    args.into_iter().map(|v| to_text(lua, v)).collect();
                st.borrow_mut().logs.push(parts?.join("\t"));
                Ok(())
            })?,
        )?;
        Ok(())
    }

    fn screen_table(&self) -> mlua::Result<Table> {
        let lua = &self.lua;
        let t = lua.create_table()?;

        macro_rules! draw {
            ($name:literal, |$s:ident, $args:ident : $ty:ty| $body:expr) => {{
                let st = self.st.clone();
                t.set(
                    $name,
                    lua.create_function(move |_, $args: $ty| {
                        let mut guard = st.borrow_mut();
                        if !guard.drawing {
                            return Ok(());
                        }
                        if let Some($s) = guard.screen.as_mut() {
                            $body;
                        }
                        Ok(())
                    })?,
                )?;
            }};
        }
        type O = Option<f64>;
        draw!("setColor", |s, a: (O, O, O, O)| s.color = Color::rgba(
            arg_f64(a.0),
            arg_f64(a.1),
            arg_f64(a.2),
            a.3.unwrap_or(255.0)
        ));
        draw!("drawClear", |s, _a: ()| s.clear());
        draw!("drawLine", |s, a: (O, O, O, O)| s.line(
            arg_f64(a.0),
            arg_f64(a.1),
            arg_f64(a.2),
            arg_f64(a.3)
        ));
        draw!("drawCircle", |s, a: (O, O, O)| s.circle(
            arg_f64(a.0),
            arg_f64(a.1),
            arg_f64(a.2)
        ));
        draw!("drawCircleF", |s, a: (O, O, O)| s.circle_fill(
            arg_f64(a.0),
            arg_f64(a.1),
            arg_f64(a.2)
        ));
        draw!("drawRect", |s, a: (O, O, O, O)| s.rect(
            arg_f64(a.0),
            arg_f64(a.1),
            arg_f64(a.2),
            arg_f64(a.3)
        ));
        draw!("drawRectF", |s, a: (O, O, O, O)| s.rect_fill(
            arg_f64(a.0),
            arg_f64(a.1),
            arg_f64(a.2),
            arg_f64(a.3)
        ));
        draw!("drawTriangle", |s, a: (O, O, O, O, O, O)| s.triangle(
            arg_f64(a.0),
            arg_f64(a.1),
            arg_f64(a.2),
            arg_f64(a.3),
            arg_f64(a.4),
            arg_f64(a.5)
        ));
        draw!("drawTriangleF", |s, a: (O, O, O, O, O, O)| s.triangle_fill(
            arg_f64(a.0),
            arg_f64(a.1),
            arg_f64(a.2),
            arg_f64(a.3),
            arg_f64(a.4),
            arg_f64(a.5)
        ));
        draw!("drawMap", |s, a: (O, O, O)| s.map(
            arg_f64(a.0),
            arg_f64(a.1),
            a.2.unwrap_or(1.0)
        ));

        let st = self.st.clone();
        t.set(
            "drawText",
            lua.create_function(move |lua, (x, y, text): (O, O, Value)| {
                let text = to_text(lua, text)?;
                let mut g = st.borrow_mut();
                if g.drawing
                    && let Some(s) = g.screen.as_mut()
                {
                    s.text(arg_f64(x), arg_f64(y), &text);
                }
                Ok(())
            })?,
        )?;
        let st = self.st.clone();
        t.set(
            "drawTextBox",
            lua.create_function(
                move |lua, (x, y, w, h, text, ha, va): (O, O, O, O, Value, O, O)| {
                    let text = to_text(lua, text)?;
                    let mut g = st.borrow_mut();
                    if g.drawing
                        && let Some(s) = g.screen.as_mut()
                    {
                        s.text_box(
                            arg_f64(x),
                            arg_f64(y),
                            arg_f64(w),
                            arg_f64(h),
                            &text,
                            ha.unwrap_or(-1.0),
                            va.unwrap_or(-1.0),
                        );
                    }
                    Ok(())
                },
            )?,
        )?;

        for (name, idx) in [
            ("setMapColorOcean", 0),
            ("setMapColorShallows", 1),
            ("setMapColorLand", 2),
            ("setMapColorGrass", 3),
            ("setMapColorSand", 4),
            ("setMapColorSnow", 5),
            ("setMapColorRock", 6),
            ("setMapColorGravel", 7),
        ] {
            let st = self.st.clone();
            t.set(
                name,
                lua.create_function(move |_, (r, g, b, a): (O, O, O, O)| {
                    if let Some(s) = st.borrow_mut().screen.as_mut() {
                        let c = Color::rgba(arg_f64(r), arg_f64(g), arg_f64(b), a.unwrap_or(255.0));
                        let m = &mut s.map_colors;
                        *[
                            &mut m.ocean,
                            &mut m.shallows,
                            &mut m.land,
                            &mut m.grass,
                            &mut m.sand,
                            &mut m.snow,
                            &mut m.rock,
                            &mut m.gravel,
                        ][idx] = c;
                    }
                    Ok(())
                })?,
            )?;
        }

        let st = self.st.clone();
        t.set(
            "getWidth",
            lua.create_function(move |_, ()| Ok(st.borrow().screen.as_ref().map_or(0, |s| s.w)))?,
        )?;
        let st = self.st.clone();
        t.set(
            "getHeight",
            lua.create_function(move |_, ()| Ok(st.borrow().screen.as_ref().map_or(0, |s| s.h)))?,
        )?;
        Ok(t)
    }

    /// Development-only `require`, mirroring what `swlua build` inlines.
    fn install_require(&self, main: &Path, search: Vec<PathBuf>) -> mlua::Result<()> {
        let mut dirs = vec![main.parent().unwrap_or(Path::new(".")).to_path_buf()];
        dirs.extend(search);
        let cache = self.lua.create_table()?;
        let files = self.files.clone();
        let req = self.lua.create_function(move |lua, name: String| {
            let cached: Value = cache.get(name.as_str())?;
            if !cached.is_nil() {
                return Ok(cached);
            }
            let path = crate::bundle::find_module(&dirs, &name)
                .ok_or_else(|| mlua::Error::runtime(format!("module '{name}' not found")))?;
            files.borrow_mut().push(path.clone());
            let code = std::fs::read_to_string(&path).map_err(mlua::Error::external)?;
            let f: Function = lua
                .load(&code)
                .set_name(format!("@{}", path.display()))
                .into_function()?;
            let v: Value = f.call(name.as_str())?;
            let v = if v.is_nil() { Value::Boolean(true) } else { v };
            cache.set(name.as_str(), v.clone())?;
            Ok(v)
        })?;
        self.lua.globals().set("require", req)?;
        Ok(())
    }

    fn make_bench(&self, path: &Path) -> mlua::Result<Lua> {
        let bench = Lua::new();
        let api = bench.create_table()?;
        let st = self.st.clone();
        macro_rules! func {
            ($name:literal, |$lua:ident, $args:ident| $body:expr) => {{
                let st = st.clone();
                #[allow(unused_variables)]
                let f = bench.create_function(move |$lua, raw: Variadic<Value>| {
                    let $args = strip_self(raw);
                    #[allow(unused_mut)]
                    let mut s = st.borrow_mut();
                    let r: mlua::Result<Value> = ($body)(&mut s);
                    r
                })?;
                api.set($name, f)?;
            }};
        }
        fn num(args: &Variadic<Value>, i: usize) -> f64 {
            match args.get(i) {
                Some(Value::Integer(n)) => *n as f64,
                Some(Value::Number(n)) => *n,
                _ => 0.0,
            }
        }
        fn truthy(args: &Variadic<Value>, i: usize) -> bool {
            !matches!(
                args.get(i),
                None | Some(Value::Nil) | Some(Value::Boolean(false))
            )
        }
        func!("setNumber", |lua, a| |s: &mut State| {
            if let Some(i) = channel_index(num(&a, 0)) {
                s.in_num[i] = num(&a, 1);
            }
            Ok(Value::Nil)
        });
        func!("setBool", |lua, a| |s: &mut State| {
            if let Some(i) = channel_index(num(&a, 0)) {
                s.in_bool[i] = truthy(&a, 1);
            }
            Ok(Value::Nil)
        });
        func!("getNumber", |lua, a| |s: &mut State| Ok(Value::Number(
            channel_index(num(&a, 0)).map_or(0.0, |i| s.in_num[i])
        )));
        func!("getBool", |lua, a| |s: &mut State| Ok(Value::Boolean(
            channel_index(num(&a, 0)).is_some_and(|i| s.in_bool[i])
        )));
        func!("getOutputNumber", |lua, a| |s: &mut State| Ok(
            Value::Number(channel_index(num(&a, 0)).map_or(0.0, |i| s.out_num[i]))
        ));
        func!("getOutputBool", |lua, a| |s: &mut State| Ok(
            Value::Boolean(channel_index(num(&a, 0)).is_some_and(|i| s.out_bool[i]))
        ));
        func!("setProperty", |lua, a| |s: &mut State| {
            let name = match a.first() {
                Some(Value::String(n)) => n.to_string_lossy().to_string(),
                _ => {
                    return Err(mlua::Error::runtime(
                        "setProperty(name, value): name must be a string",
                    ));
                }
            };
            let v = match a.get(1) {
                Some(Value::Boolean(b)) => PropValue::Bool(*b),
                Some(Value::Integer(n)) => PropValue::Number(*n as f64),
                Some(Value::Number(n)) => PropValue::Number(*n),
                Some(Value::String(t)) => PropValue::Text(t.to_string_lossy().to_string()),
                _ => {
                    return Err(mlua::Error::runtime(
                        "setProperty(name, value): value must be a number, bool or string",
                    ));
                }
            };
            s.props.insert(name, v);
            Ok(Value::Nil)
        });
        func!("touch", |lua, a| |s: &mut State| {
            // touch(x, y, pressed[, which])
            let which = if num(&a, 3) >= 2.0 { 1 } else { 0 };
            s.touch[which] = Touch {
                x: num(&a, 0),
                y: num(&a, 1),
                down: truthy(&a, 2),
            };
            Ok(Value::Nil)
        });
        func!("log", |lua, a| |s: &mut State| {
            let parts: Vec<String> = a
                .iter()
                .map(|v| v.to_string().unwrap_or_default())
                .collect();
            s.logs.push(format!("[bench] {}", parts.join("\t")));
            Ok(Value::Nil)
        });
        func!("stop", |lua, a| |s: &mut State| {
            s.stop_requested = true;
            Ok(Value::Nil)
        });
        func!("getScreenSize", |lua, a| |s: &mut State| {
            let (w, h) = s.screen.as_ref().map_or((0, 0), |s| (s.w, s.h));
            Ok(Value::Table(lua.create_sequence_from([w, h])?))
        });
        bench.globals().set("sim", api)?;
        let code = std::fs::read_to_string(path)
            .map_err(|e| mlua::Error::runtime(format!("reading {}: {e}", path.display())))?;
        bench
            .load(&code)
            .set_name(format!("@{}", path.display()))
            .exec()?;
        Ok(bench)
    }

    fn call_bench(&self, name: &str, args: impl mlua::IntoLuaMulti) -> mlua::Result<()> {
        let Some(b) = &self.bench else { return Ok(()) };
        match b.globals().get::<Value>(name)? {
            Value::Function(f) => {
                let sim: Value = b.globals().get("sim")?;
                let mut all = args.into_lua_multi(b)?;
                all.push_front(sim);
                f.call::<()>(all)
            }
            _ => Ok(()),
        }
    }

    fn call_script(&self, name: &str, args: impl mlua::IntoLuaMulti) -> mlua::Result<()> {
        match self.lua.globals().get::<Value>(name)? {
            Value::Function(f) => self.guarded(|_| f.call::<()>(args)),
            _ => Ok(()),
        }
    }

    /// Advances one game tick (1/60 s).
    pub fn step(&mut self) -> Result<(), String> {
        if let Some(e) = &self.error {
            return Err(e.clone());
        }
        let r = self.step_inner();
        if let Err(e) = &r {
            self.error = Some(e.clone());
        }
        r
    }

    fn step_inner(&mut self) -> Result<(), String> {
        {
            let mut s = self.st.borrow_mut();
            let s = &mut *s;
            for (&i, &v) in &s.base_num {
                s.in_num[i] = v;
            }
            for (&i, &v) in &s.base_bool {
                s.in_bool[i] = v;
            }
            if s.touch_enabled {
                // Monitor composite: 1/2 = size, 3-6 = touch positions, bools 1-2 = pressed.
                let (w, h) = s.screen.as_ref().map_or((0, 0), |sc| (sc.w, sc.h));
                s.in_num[0] = w as f64;
                s.in_num[1] = h as f64;
                s.in_num[2] = s.touch[0].x;
                s.in_num[3] = s.touch[0].y;
                s.in_num[4] = s.touch[1].x;
                s.in_num[5] = s.touch[1].y;
                s.in_bool[0] = s.touch[0].down;
                s.in_bool[1] = s.touch[1].down;
            }
        }
        // Inputs persist between ticks unless something sets them, as in game.
        self.call_bench("onSimTick", self.tick)
            .map_err(|e| format!("bench onSimTick: {e}"))?;
        {
            let mut s = self.st.borrow_mut();
            let s = &mut *s;
            for (&i, &v) in &s.held_num {
                s.in_num[i] = v;
            }
            for (&i, &v) in &s.held_bool {
                s.in_bool[i] = v;
            }
        }

        while let Ok((port, url, body)) = self.http_rx.try_recv() {
            self.call_script("httpReply", (port, url, body))
                .map_err(|e| e.to_string())?;
        }
        self.call_script("onTick", ()).map_err(|e| e.to_string())?;
        self.draw()?;
        self.call_bench("onSimAfterTick", self.tick)
            .map_err(|e| format!("bench onSimAfterTick: {e}"))?;
        self.tick += 1;
        Ok(())
    }

    fn draw(&mut self) -> Result<(), String> {
        {
            let mut s = self.st.borrow_mut();
            let Some(sc) = s.screen.as_mut() else {
                return Ok(());
            };
            sc.reset();
            s.drawing = true;
        }
        let r = self.call_script("onDraw", ());
        self.st.borrow_mut().drawing = false;
        r.map_err(|e| e.to_string())
    }

    pub fn take_logs(&self) -> Vec<String> {
        std::mem::take(&mut self.st.borrow_mut().logs)
    }

    pub fn screen_png(&self) -> Option<Vec<u8>> {
        let s = self.st.borrow();
        let sc = s.screen.as_ref()?;
        Some(encode_png(sc.w, sc.h, &sc.pixels))
    }
}

pub fn encode_png(w: usize, h: usize, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().expect("png header");
        wr.write_image_data(rgba).expect("png data");
    }
    out
}

/// The game only allows HTTP GET to localhost.
fn http_get(port: u16, url: &str) -> std::io::Result<String> {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect_timeout(
        &([127, 0, 0, 1], port).into(),
        Duration::from_secs(2),
    )?;
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    let path = if url.starts_with('/') {
        url.to_string()
    } else {
        format!("/{url}")
    };
    write!(
        s,
        "GET {path} HTTP/1.0\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n"
    )?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf).to_string();
    Ok(match text.split_once("\r\n\r\n") {
        Some((_, body)) => body.to_string(),
        None => text,
    })
}

/// Converts TOML property values from the config.
pub fn props_from_toml(
    map: &std::collections::BTreeMap<String, toml::Value>,
) -> Result<HashMap<String, PropValue>> {
    map.iter()
        .map(|(k, v)| {
            let p = match v {
                toml::Value::Boolean(b) => PropValue::Bool(*b),
                toml::Value::Integer(i) => PropValue::Number(*i as f64),
                toml::Value::Float(f) => PropValue::Number(*f),
                toml::Value::String(s) => PropValue::Text(s.clone()),
                other => anyhow::bail!("property '{k}': unsupported value {other}"),
            };
            Ok((k.clone(), p))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> SimOptions {
        SimOptions {
            screen: Some((32, 32)),
            touch: false,
            props: HashMap::new(),
            numbers: HashMap::new(),
            bools: HashMap::new(),
            bench: None,
        }
    }

    #[test]
    fn runs_ticks_and_draws() {
        let code = r#"
            t = 0
            function onTick()
                t = t + input.getNumber(1)
                output.setNumber(1, t)
                output.setBool(2, t > 2)
            end
            function onDraw()
                screen.setColor(255, 0, 0)
                screen.drawRectF(0, 0, 4, 4)
                screen.drawText(0, 10, "HI " .. screen.getWidth())
            end
        "#;
        let mut o = opts();
        o.numbers.insert(1, 1.5);
        let mut sim = Sim::new(
            Source::Code {
                name: "t".into(),
                code: code.into(),
            },
            o,
        )
        .unwrap();
        for _ in 0..3 {
            sim.step().unwrap();
        }
        let s = sim.st.borrow();
        assert_eq!(s.out_num[0], 4.5);
        assert!(s.out_bool[1]);
        let px = &s.screen.as_ref().unwrap().pixels;
        assert_eq!(&px[0..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn reports_errors_and_hangs() {
        let mut sim = Sim::new(
            Source::Code {
                name: "t".into(),
                code: "function onTick() error('boom') end".into(),
            },
            opts(),
        )
        .unwrap();
        assert!(sim.step().unwrap_err().contains("boom"));
        assert!(sim.step().is_err(), "script stays stopped after an error");

        let mut sim = Sim::new(
            Source::Code {
                name: "t".into(),
                code: "function onTick() while true do end end".into(),
            },
            opts(),
        )
        .unwrap();
        assert!(sim.step().unwrap_err().contains("infinite loop"));
    }

    #[test]
    fn sandboxed() {
        let code =
            "assert(io == nil and os == nil and load == nil and require == nil) debug.log(1.0)";
        let sim = Sim::new(
            Source::Code {
                name: "t".into(),
                code: code.into(),
            },
            opts(),
        )
        .unwrap();
        assert_eq!(sim.take_logs(), vec!["1.0"]);
    }
}
