//! The `world` table available to test bench scripts.

use super::State;
use super::world::*;
use mlua::{Lua, Table, Value};
use std::cell::RefCell;
use std::f64::consts::TAU;
use std::rc::Rc;

fn vec_arg(v: Option<Value>) -> mlua::Result<Option<V3>> {
    let Some(Value::Table(t)) = v else {
        return Ok(None);
    };
    let get = |k: &str, i: i64| -> mlua::Result<f64> {
        let v: Option<f64> = t.get(k)?;
        match v {
            Some(v) => Ok(v),
            None => Ok(t.get::<Option<f64>>(i)?.unwrap_or(0.0)),
        }
    };
    Ok(Some(v3(get("x", 1)?, get("y", 2)?, get("z", 3)?)))
}

fn vec_table(lua: &Lua, v: V3) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("x", v.x)?;
    t.set("y", v.y)?;
    t.set("z", v.z)?;
    Ok(t)
}

/// Applies the common body fields of a vehicle or target description.
fn apply_body(b: &mut Body, t: &Table) -> mlua::Result<()> {
    if let Some(p) = vec_arg(t.get("pos")?)? {
        b.pos = p;
    }
    let (h0, p0, r0) = hpr(&b.rot);
    let heading: Option<f64> = t.get("heading")?;
    let pitch: Option<f64> = t.get("pitch")?;
    let roll: Option<f64> = t.get("roll")?;
    if heading.is_some() || pitch.is_some() || roll.is_some() {
        b.rot = M3::from_hpr(
            heading.unwrap_or(h0).to_radians(),
            pitch.unwrap_or(p0).to_radians(),
            roll.unwrap_or(r0).to_radians(),
        );
    }
    if let Some(e) = vec_arg(t.get("euler")?)? {
        b.rot = M3::from_euler(e.x, e.y, e.z);
    }
    if let Some(v) = vec_arg(t.get("vel")?)? {
        b.vel = v;
        b.speed = None;
    }
    if let Some(s) = t.get::<Option<f64>>("speed")? {
        b.speed = Some(s);
    }
    let yaw: Option<f64> = t.get("yaw_rate")?;
    let pitch_rate: Option<f64> = t.get("pitch_rate")?;
    let roll_rate: Option<f64> = t.get("roll_rate")?;
    // turn_rate is an alias for yaw_rate.
    let yaw = yaw.or(t.get("turn_rate")?);
    if let Some(y) = yaw {
        b.ang_vel.y = y.to_radians();
    }
    if let Some(p) = pitch_rate {
        b.ang_vel.x = -p.to_radians();
    }
    if let Some(r) = roll_rate {
        b.ang_vel.z = -r.to_radians();
    }
    Ok(())
}

fn body_table(lua: &Lua, b: &Body) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("pos", vec_table(lua, b.pos)?)?;
    t.set("vel", vec_table(lua, b.velocity())?)?;
    let (h, p, r) = hpr(&b.rot);
    t.set("heading", h)?;
    t.set("pitch", p)?;
    t.set("roll", r)?;
    t.set("speed", b.velocity().len())?;
    Ok(t)
}

pub fn install(lua: &Lua, st: Rc<RefCell<State>>) -> mlua::Result<()> {
    let w = lua.create_table()?;

    macro_rules! f {
        ($name:literal, |$lua:ident, $s:ident, $world:ident, $args:ident : $ty:ty| $body:expr) => {{
            let st = st.clone();
            w.set(
                $name,
                #[allow(unused_variables)]
                lua.create_function(move |$lua, $args: $ty| {
                    #[allow(unused_mut, unused_variables)]
                    let mut guard = st.borrow_mut();
                    #[allow(unused_variables)]
                    let $s = &mut *guard;
                    let $world = &mut $s.world;
                    #[allow(unused_braces)]
                    let r: mlua::Result<_> = { $body };
                    r
                })?,
            )?;
        }};
    }

    f!("setVehicle", |lua, s, world, t: Table| apply_body(
        &mut world.vehicle,
        &t
    ));
    f!("getVehicle", |lua, s, world, _a: ()| body_table(
        lua,
        &world.vehicle
    ));
    f!("addTarget", |lua, s, world, t: Option<Table>| {
        let id = world.new_id();
        let mut target = Target {
            id,
            name: format!("target {id}"),
            body: Body::default(),
            trail: vec![],
        };
        if let Some(t) = t {
            apply_body(&mut target.body, &t)?;
            if let Some(n) = t.get::<Option<String>>("name")? {
                target.name = n;
            }
        }
        world.targets.push(target);
        Ok(id)
    });
    f!("setTarget", |lua, s, world, a: (u32, Table)| {
        let t = world
            .target_mut(a.0)
            .ok_or_else(|| mlua::Error::runtime(format!("no target {}", a.0)))?;
        apply_body(&mut t.body, &a.1)
    });
    f!("removeTarget", |lua, s, world, id: u32| {
        world.targets.retain(|t| t.id != id);
        Ok(())
    });
    f!("getTargets", |lua, s, world, _a: ()| {
        let list = lua.create_table()?;
        for t in &world.targets {
            let e = body_table(lua, &t.body)?;
            e.set("id", t.id)?;
            e.set("name", t.name.clone())?;
            list.push(e)?;
        }
        Ok(list)
    });
    f!("addRadar", |lua, s, world, t: Option<Table>| {
        let id = world.new_id();
        let t = match t {
            Some(t) => t,
            None => lua.create_table()?,
        };
        let radar = Radar {
            id,
            name: t
                .get::<Option<String>>("name")?
                .unwrap_or_else(|| format!("radar {id}")),
            offset: vec_arg(t.get("offset")?)?.unwrap_or_default(),
            fov_x: t.get::<Option<f64>>("fov_x")?.unwrap_or(0.05),
            fov_y: t.get::<Option<f64>>("fov_y")?.unwrap_or(0.05),
            range: t.get::<Option<f64>>("range")?.unwrap_or(5000.0),
            noise: t.get::<Option<f64>>("noise")?.unwrap_or(0.005),
            yaw_output: t.get("yaw_output")?,
            pitch_output: t.get("pitch_output")?,
            yaw: 0.0,
            pitch: 0.0,
            yaw_target: 0.0,
            pitch_target: 0.0,
            slew: t.get("slew")?,
            hold_ticks: t.get::<Option<u64>>("hold_ticks")?.unwrap_or(0),
            slots: Default::default(),
        };
        world.radars.push(radar);
        Ok(id)
    });
    f!("aimRadar", |lua, s, world, a: (u32, f64, f64)| {
        let r = world
            .radar_mut(a.0)
            .ok_or_else(|| mlua::Error::runtime(format!("no radar {}", a.0)))?;
        r.yaw_target = a.1;
        r.pitch_target = a.2;
        Ok(())
    });
    f!("getRadar", |lua, s, world, id: u32| {
        let r = world
            .radar_mut(id)
            .ok_or_else(|| mlua::Error::runtime(format!("no radar {id}")))?;
        let t = lua.create_table()?;
        t.set("yaw", r.yaw)?;
        t.set("pitch", r.pitch)?;
        let contacts = lua.create_table()?;
        for (i, c) in r.slots.iter().enumerate() {
            if let Some(c) = c {
                let e = lua.create_table()?;
                e.set("slot", i + 1)?;
                e.set("target", c.target)?;
                e.set("distance", c.distance)?;
                e.set("azimuth", c.azimuth)?;
                e.set("elevation", c.elevation)?;
                e.set("ticks", c.ticks_since)?;
                contacts.push(e)?;
            }
        }
        t.set("contacts", contacts)?;
        Ok(t)
    });
    f!(
        "writeRadar",
        |lua, s, world, a: (u32, Option<usize>, Option<usize>)| {
            let (n, b) = world
                .radar_composite(a.0)
                .ok_or_else(|| mlua::Error::runtime(format!("no radar {}", a.0)))?;
            let (ns, bs) = (a.1.unwrap_or(1).max(1) - 1, a.2.unwrap_or(1).max(1) - 1);
            for (i, v) in n.iter().enumerate() {
                if let Some(slot) = s.in_num.get_mut(ns + i) {
                    *slot = *v;
                }
            }
            for (i, v) in b.iter().enumerate() {
                if let Some(slot) = s.in_bool.get_mut(bs + i) {
                    *slot = *v;
                }
            }
            Ok(())
        }
    );
    f!("radarValues", |lua, s, world, id: u32| {
        let (n, b) = world
            .radar_composite(id)
            .ok_or_else(|| mlua::Error::runtime(format!("no radar {id}")))?;
        Ok((n.to_vec(), b.to_vec()))
    });
    f!("toLocal", |lua, s, world, p: Value| {
        let p = vec_arg(Some(p))?.unwrap_or_default();
        vec_table(
            lua,
            world
                .vehicle
                .rot
                .transpose()
                .apply(p.sub(world.vehicle.pos)),
        )
    });
    f!("physics", |lua, s, world, _a: ()| lua
        .create_sequence_from(world.physics()));
    f!("writePhysics", |lua, s, world, start: Option<usize>| {
        let p = world.physics();
        let ns = start.unwrap_or(1).max(1) - 1;
        for (i, v) in p.iter().enumerate() {
            if let Some(slot) = s.in_num.get_mut(ns + i) {
                *slot = *v;
            }
        }
        Ok(())
    });
    f!("clear", |lua, s, world, _a: ()| {
        *world = World::default();
        Ok(())
    });
    // Handy constant: radar angles are in turns.
    w.set("TAU", TAU)?;
    lua.globals().set("world", w)?;
    Ok(())
}
