//! `sim:addController(path)`: runs another microcontroller script inside the
//! bench, so chained controllers (e.g. a radar processor feeding a HUD) can
//! be tested together. The bench copies values between them each tick.

use super::{PropValue, Sim, SimOptions, Source, State};
use mlua::{Lua, Table, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

fn channel(i: f64) -> Option<usize> {
    let i = i.floor();
    (1.0..=super::CHANNELS as f64)
        .contains(&i)
        .then(|| i as usize - 1)
}

fn prop_value(v: Value) -> mlua::Result<PropValue> {
    Ok(match v {
        Value::Boolean(b) => PropValue::Bool(b),
        Value::Integer(i) => PropValue::Number(i as f64),
        Value::Number(n) => PropValue::Number(n),
        Value::String(s) => PropValue::Text(s.to_string_lossy().to_string()),
        _ => {
            return Err(mlua::Error::runtime(
                "property values must be numbers, bools or strings",
            ));
        }
    })
}

pub fn install(
    bench: &Lua,
    outer: Rc<RefCell<State>>,
    files: Rc<RefCell<Vec<PathBuf>>>,
    search: Vec<PathBuf>,
    bench_dir: PathBuf,
) -> mlua::Result<()> {
    let api: Table = bench.globals().get("sim")?;
    let f = bench.create_function(
        move |lua, (_self, path, props): (Value, String, Option<Table>)| {
            let p = Path::new(&path);
            let main = if p.is_absolute() {
                p.to_path_buf()
            } else {
                bench_dir.join(p)
            };
            let mut prop_map = HashMap::new();
            if let Some(t) = props {
                for pair in t.pairs::<String, Value>() {
                    let (k, v) = pair?;
                    prop_map.insert(k, prop_value(v)?);
                }
            }
            let opts = SimOptions {
                screen: None,
                touch: false,
                props: prop_map,
                numbers: HashMap::new(),
                bools: HashMap::new(),
                bench: None,
            };
            // A controller in its own project uses that project's library folders.
            let search = match main.parent().map(crate::config::Config::find) {
                Some(Ok(Some(cfg))) => cfg.lib_dirs(),
                _ => search.clone(),
            };
            let ctl = Sim::new(
                Source::File {
                    main: main.clone(),
                    search,
                },
                opts,
            )
            .map_err(|e| {
                mlua::Error::runtime(format!("loading controller {}: {e:#}", main.display()))
            })?;
            files
                .borrow_mut()
                .extend(ctl.files.borrow().iter().cloned());
            let name = main
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let name = match main
                .parent()
                .and_then(|p| p.parent())
                .and_then(|p| p.file_name())
            {
                // src/main.lua inside a project folder: use the folder name.
                Some(dir) if name == "main" => dir.to_string_lossy().to_string(),
                _ => name,
            };
            make_handle(lua, Rc::new(RefCell::new(ctl)), outer.clone(), name)
        },
    )?;
    api.set("addController", f)?;
    Ok(())
}

/// `ctl:setInputs(numbers, bools, num_start, bool_start)`.
type SetInputsArgs = (
    Value,
    Option<Vec<f64>>,
    Option<Vec<bool>>,
    Option<usize>,
    Option<usize>,
);

fn make_handle(
    lua: &Lua,
    ctl: Rc<RefCell<Sim>>,
    outer: Rc<RefCell<State>>,
    name: String,
) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    let c = ctl.clone();
    t.set(
        "setNumber",
        lua.create_function(move |_, (_s, i, v): (Value, f64, f64)| {
            if let Some(i) = channel(i) {
                c.borrow().st.borrow_mut().in_num[i] = v;
            }
            Ok(())
        })?,
    )?;
    let c = ctl.clone();
    t.set(
        "setBool",
        lua.create_function(move |_, (_s, i, v): (Value, f64, bool)| {
            if let Some(i) = channel(i) {
                c.borrow().st.borrow_mut().in_bool[i] = v;
            }
            Ok(())
        })?,
    )?;
    let c = ctl.clone();
    t.set(
        "setInputs",
        lua.create_function(move |_, (_s, nums, bools, ns, bs): SetInputsArgs| {
            let sim = c.borrow();
            let mut st = sim.st.borrow_mut();
            let (ns, bs) = (ns.unwrap_or(1).max(1) - 1, bs.unwrap_or(1).max(1) - 1);
            for (i, v) in nums.unwrap_or_default().into_iter().enumerate() {
                if let Some(slot) = st.in_num.get_mut(ns + i) {
                    *slot = v;
                }
            }
            for (i, v) in bools.unwrap_or_default().into_iter().enumerate() {
                if let Some(slot) = st.in_bool.get_mut(bs + i) {
                    *slot = v;
                }
            }
            Ok(())
        })?,
    )?;
    let c = ctl.clone();
    t.set(
        "getNumber",
        lua.create_function(move |_, (_s, i): (Value, f64)| {
            Ok(channel(i).map_or(0.0, |i| c.borrow().st.borrow().out_num[i]))
        })?,
    )?;
    let c = ctl.clone();
    t.set(
        "getBool",
        lua.create_function(move |_, (_s, i): (Value, f64)| {
            Ok(channel(i).is_some_and(|i| c.borrow().st.borrow().out_bool[i]))
        })?,
    )?;
    let c = ctl.clone();
    t.set(
        "getOutputs",
        lua.create_function(move |_, _s: Value| {
            let sim = c.borrow();
            let st = sim.st.borrow();
            Ok((st.out_num.to_vec(), st.out_bool.to_vec()))
        })?,
    )?;
    let c = ctl.clone();
    t.set(
        "setProperty",
        lua.create_function(move |_, (_s, k, v): (Value, String, Value)| {
            c.borrow().st.borrow_mut().props.insert(k, prop_value(v)?);
            Ok(())
        })?,
    )?;
    let c = ctl.clone();
    t.set(
        "tick",
        lua.create_function(move |_, _s: Value| {
            let mut sim = c.borrow_mut();
            let r = sim.step();
            let logs = sim.take_logs();
            {
                let mut o = outer.borrow_mut();
                o.logs
                    .extend(logs.into_iter().map(|l| format!("[{name}] {l}")));
            }
            r.map_err(|e| mlua::Error::runtime(format!("controller {name}: {e}")))
        })?,
    )?;
    Ok(t)
}
