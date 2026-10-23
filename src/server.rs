//! `swlua sim`: a browser-based debugger. The simulator and the HTTP server
//! share one thread, so the Lua state never crosses threads.

use crate::sim::world::{Body, M3, Target, V3, World, v3};
use crate::sim::{PropValue, Sim, Touch};
use anyhow::{Result, anyhow};
use serde::Deserialize;
use serde_json::{Value as Json, json};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};
use tiny_http::{Header, Method, Request, Response, Server};

const INDEX_HTML: &str = include_str!("../web/index.html");
const MAX_LOGS: usize = 1000;

pub type Factory = Box<dyn Fn() -> Result<Sim>>;

struct App {
    factory: Factory,
    sim: Option<Sim>,
    load_error: Option<String>,
    paused: bool,
    speed: f64,
    tick_rate: f64,
    logs: VecDeque<(u64, String)>,
    log_seq: u64,
    mtimes: Vec<(PathBuf, Option<SystemTime>)>,
    auto_reload: bool,
    tps: f64,
    /// Properties edited in the UI, re-applied after every reload.
    ui_props: HashMap<String, PropValue>,
    /// Files to watch when the script failed to load before reporting any.
    fallback_watch: Vec<PathBuf>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum InputReq {
    Num { ch: usize, value: f64 },
    Bool { ch: usize, value: bool },
    Release { ch: usize, bool: bool },
}

#[derive(Deserialize)]
struct TouchReq {
    x: f64,
    y: f64,
    down: bool,
    #[serde(default)]
    which: usize,
}

#[derive(Deserialize)]
struct ControlReq {
    action: String,
    #[serde(default)]
    value: f64,
}

/// Flight controls from the 3D view; rates in degrees per second.
#[derive(Deserialize)]
struct FlyReq {
    speed: Option<f64>,
    yaw_rate: Option<f64>,
    pitch_rate: Option<f64>,
    roll_rate: Option<f64>,
    /// Snap back to level flight on the current heading.
    #[serde(default)]
    level: bool,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum TargetReq {
    /// Adds a target ahead of the vehicle, flying across its path.
    AddAhead {
        distance: f64,
    },
    Remove {
        id: u32,
    },
}

fn v(p: V3) -> Json {
    json!([p.x, p.y, p.z])
}

fn rot(m: &M3) -> Json {
    json!(m.0.iter().flatten().collect::<Vec<_>>())
}

fn world_json(w: &World) -> Json {
    let (h, p, r) = w.vehicle_hpr();
    let detected: std::collections::HashSet<u32> = w
        .radars
        .iter()
        .flat_map(|r| {
            r.slots
                .iter()
                .flatten()
                .filter(|c| c.ticks_since == 0)
                .map(|c| c.target)
        })
        .collect();
    json!({
        "vehicle": {
            "pos": v(w.vehicle.pos),
            "rot": rot(&w.vehicle.rot),
            "vel": v(w.vehicle.velocity()),
            "speed": w.vehicle.velocity().len(),
            "hpr": [h, p, r],
            "ang_vel": v(w.vehicle.ang_vel),
        },
        "targets": w.targets.iter().map(|t| json!({
            "id": t.id,
            "name": t.name,
            "pos": v(t.body.pos),
            "vel": v(t.body.velocity()),
            "trail": t.trail.iter().map(|p| v(*p)).collect::<Vec<_>>(),
            "detected": detected.contains(&t.id),
            "distance": t.body.pos.sub(w.vehicle.pos).len(),
        })).collect::<Vec<_>>(),
        "radars": w.radars.iter().map(|r| json!({
            "id": r.id,
            "name": r.name,
            "pos": v(r.position(&w.vehicle)),
            "rot": rot(&r.rotation(&w.vehicle.rot)),
            "fov_x": r.fov_x,
            "fov_y": r.fov_y,
            "range": r.range,
            "yaw": r.yaw,
            "pitch": r.pitch,
            "contacts": r.slots.iter().enumerate().filter_map(|(i, c)| c.as_ref().map(|c| json!({
                "slot": i + 1,
                "target": c.target,
                "distance": c.distance,
                "azimuth": c.azimuth,
                "elevation": c.elevation,
                "ticks": c.ticks_since,
                "world": v(c.world_pos),
            }))).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "physics": w.physics().to_vec(),
        "empty": w.is_empty(),
    })
}

#[derive(Deserialize)]
struct PropReq {
    name: String,
    value: Json,
}

fn mtime(p: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

impl App {
    fn log(&mut self, s: String) {
        self.log_seq += 1;
        self.logs.push_back((self.log_seq, s));
        while self.logs.len() > MAX_LOGS {
            self.logs.pop_front();
        }
    }

    fn reload(&mut self, keep_inputs: bool) {
        let carry = self.sim.as_ref().filter(|_| keep_inputs).map(|s| {
            let st = s.st.borrow();
            (st.held_num.clone(), st.held_bool.clone(), st.touch)
        });
        match (self.factory)() {
            Ok(sim) => {
                {
                    let mut st = sim.st.borrow_mut();
                    if let Some((hn, hb, touch)) = carry {
                        st.held_num = hn;
                        st.held_bool = hb;
                        st.touch = touch;
                    }
                    st.props.extend(self.ui_props.clone());
                }
                self.mtimes = sim
                    .files
                    .borrow()
                    .iter()
                    .map(|p| (p.clone(), mtime(p)))
                    .collect();
                self.sim = Some(sim);
                self.load_error = None;
                self.log("[swlua] script loaded".into());
            }
            Err(e) => {
                let msg = format!("{e:#}");
                self.log(format!("[swlua] load failed: {msg}"));
                self.load_error = Some(msg);
                // Keep watching the files that were known to be involved.
                if self.mtimes.is_empty() {
                    self.mtimes = self
                        .fallback_watch
                        .iter()
                        .map(|p| (p.clone(), mtime(p)))
                        .collect();
                }
            }
        }
        self.flush_logs();
    }

    fn files_changed(&mut self) -> bool {
        self.mtimes.iter().any(|(p, t)| mtime(p) != *t)
    }

    fn flush_logs(&mut self) {
        let logs = self.sim.as_ref().map(|s| s.take_logs()).unwrap_or_default();
        for l in logs {
            self.log(l);
        }
    }

    fn step(&mut self) {
        let Some(sim) = self.sim.as_mut() else { return };
        let r = sim.step();
        let stop = std::mem::take(&mut sim.st.borrow_mut().stop_requested);
        self.flush_logs();
        if let Err(e) = r {
            self.log(format!("[error] {e}"));
            self.paused = true;
        }
        if stop {
            self.log("[swlua] bench requested stop; paused".into());
            self.paused = true;
        }
    }

    fn state_json(&self, since: u64) -> Json {
        let logs: Vec<Json> = self
            .logs
            .iter()
            .filter(|(s, _)| *s > since)
            .map(|(s, t)| json!({"seq": s, "text": t}))
            .collect();
        let base = json!({
            "paused": self.paused,
            "speed": self.speed,
            "tps": self.tps,
            "auto_reload": self.auto_reload,
            "logs": logs,
            "load_error": self.load_error,
        });
        let Some(sim) = &self.sim else { return base };
        let st = sim.st.borrow();
        let props: HashMap<&String, Json> = st
            .props
            .iter()
            .map(|(k, v)| {
                (
                    k,
                    match v {
                        PropValue::Number(n) => json!(n),
                        PropValue::Bool(b) => json!(b),
                        PropValue::Text(t) => json!(t),
                    },
                )
            })
            .collect();
        let mut v = base;
        let o = v.as_object_mut().unwrap();
        o.insert("tick".into(), json!(sim.tick));
        o.insert("error".into(), json!(sim.error));
        o.insert(
            "screen".into(),
            st.screen
                .as_ref()
                .map_or(Json::Null, |s| json!({"w": s.w, "h": s.h})),
        );
        o.insert("touch_enabled".into(), json!(st.touch_enabled));
        o.insert("in_num".into(), json!(st.in_num.to_vec()));
        o.insert("in_bool".into(), json!(st.in_bool.to_vec()));
        o.insert("out_num".into(), json!(st.out_num.to_vec()));
        o.insert("out_bool".into(), json!(st.out_bool.to_vec()));
        o.insert(
            "held_num".into(),
            json!(
                st.held_num
                    .iter()
                    .map(|(k, v)| (k + 1, *v))
                    .collect::<HashMap<_, _>>()
            ),
        );
        o.insert(
            "held_bool".into(),
            json!(
                st.held_bool
                    .iter()
                    .map(|(k, v)| (k + 1, *v))
                    .collect::<HashMap<_, _>>()
            ),
        );
        o.insert("props".into(), json!(props));
        o.insert(
            "http".into(),
            json!(st.http_requests.iter().rev().take(10).collect::<Vec<_>>()),
        );
        v
    }

    fn handle(&mut self, mut req: Request) {
        let url = req.url().to_string();
        let (path, query) = url.split_once('?').unwrap_or((&url, ""));
        let mut body = String::new();
        let _ = req.as_reader().read_to_string(&mut body);
        let resp = match (req.method(), path) {
            (Method::Get, "/") => html(INDEX_HTML),
            (Method::Get, "/api/state") => {
                let since = query
                    .split('&')
                    .find_map(|kv| kv.strip_prefix("since="))
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                json_resp(&self.state_json(since))
            }
            (Method::Get, "/api/frame") => match self
                .sim
                .as_ref()
                .and_then(|s| s.st.borrow().screen.as_ref().map(|sc| sc.pixels.clone()))
            {
                Some(px) => Response::from_data(px)
                    .with_header(
                        Header::from_bytes("Content-Type", "application/octet-stream").unwrap(),
                    )
                    .boxed(),
                None => Response::empty(204).boxed(),
            },
            (Method::Get, "/api/screenshot.png") => {
                match self.sim.as_ref().and_then(|s| s.screen_png()) {
                    Some(png) => Response::from_data(png)
                        .with_header(Header::from_bytes("Content-Type", "image/png").unwrap())
                        .boxed(),
                    None => Response::empty(404).boxed(),
                }
            }
            (Method::Post, "/api/input") => self.post(&body, |app, r: InputReq| {
                let sim = app
                    .sim
                    .as_ref()
                    .ok_or_else(|| anyhow!("no script loaded"))?;
                let mut st = sim.st.borrow_mut();
                match r {
                    InputReq::Num { ch, value } if (1..=32).contains(&ch) => {
                        st.held_num.insert(ch - 1, value);
                    }
                    InputReq::Bool { ch, value } if (1..=32).contains(&ch) => {
                        st.held_bool.insert(ch - 1, value);
                    }
                    InputReq::Release { ch, bool } if (1..=32).contains(&ch) => {
                        if bool {
                            st.held_bool.remove(&(ch - 1));
                        } else {
                            st.held_num.remove(&(ch - 1));
                        }
                    }
                    _ => return Err(anyhow!("channel out of range")),
                }
                Ok(())
            }),
            (Method::Post, "/api/touch") => self.post(&body, |app, r: TouchReq| {
                let sim = app
                    .sim
                    .as_ref()
                    .ok_or_else(|| anyhow!("no script loaded"))?;
                sim.st.borrow_mut().touch[r.which.min(1)] = Touch {
                    x: r.x,
                    y: r.y,
                    down: r.down,
                };
                Ok(())
            }),
            (Method::Post, "/api/property") => self.post(&body, |app, r: PropReq| {
                let sim = app
                    .sim
                    .as_ref()
                    .ok_or_else(|| anyhow!("no script loaded"))?;
                let v = match r.value {
                    Json::Bool(b) => PropValue::Bool(b),
                    Json::Number(n) => PropValue::Number(n.as_f64().unwrap_or(0.0)),
                    Json::String(s) => PropValue::Text(s),
                    _ => return Err(anyhow!("unsupported property value")),
                };
                sim.st.borrow_mut().props.insert(r.name.clone(), v.clone());
                app.ui_props.insert(r.name, v);
                Ok(())
            }),
            (Method::Get, "/api/world") => match &self.sim {
                Some(sim) => json_resp(&world_json(&sim.st.borrow().world)),
                None => json_resp(&Json::Null),
            },
            (Method::Post, "/api/world/fly") => self.post(&body, |app, r: FlyReq| {
                let sim = app
                    .sim
                    .as_ref()
                    .ok_or_else(|| anyhow!("no script loaded"))?;
                let mut st = sim.st.borrow_mut();
                let b = &mut st.world.vehicle;
                if let Some(s) = r.speed {
                    b.speed = Some(s.max(0.0));
                }
                if let Some(y) = r.yaw_rate {
                    b.ang_vel.y = y.to_radians();
                }
                if let Some(p) = r.pitch_rate {
                    b.ang_vel.x = -p.to_radians();
                }
                if let Some(rr) = r.roll_rate {
                    b.ang_vel.z = -rr.to_radians();
                }
                if r.level {
                    let f = b.rot.forward();
                    b.rot = M3::from_hpr(f.x.atan2(f.z), 0.0, 0.0);
                    b.ang_vel = V3::default();
                }
                Ok(())
            }),
            (Method::Post, "/api/world/target") => self.post(&body, |app, r: TargetReq| {
                let sim = app
                    .sim
                    .as_ref()
                    .ok_or_else(|| anyhow!("no script loaded"))?;
                let mut st = sim.st.borrow_mut();
                let w = &mut st.world;
                match r {
                    TargetReq::AddAhead { distance } => {
                        let id = w.new_id();
                        let f = w.vehicle.rot.forward();
                        let flat = v3(f.x, 0.0, f.z);
                        let flat = if flat.len() < 1e-6 {
                            v3(0.0, 0.0, 1.0)
                        } else {
                            flat.scale(1.0 / flat.len())
                        };
                        let right = v3(flat.z, 0.0, -flat.x);
                        let pos = w
                            .vehicle
                            .pos
                            .add(flat.scale(distance))
                            .add(right.scale(-distance * 0.2));
                        let body = Body {
                            pos,
                            vel: right.scale(40.0),
                            ..Default::default()
                        };
                        w.targets.push(Target {
                            id,
                            name: format!("target {id}"),
                            body,
                            trail: vec![],
                        });
                    }
                    TargetReq::Remove { id } => w.targets.retain(|t| t.id != id),
                }
                Ok(())
            }),
            (Method::Post, "/api/control") => self.post(&body, |app, r: ControlReq| {
                match r.action.as_str() {
                    "pause" => app.paused = true,
                    "resume" => {
                        if app.sim.as_ref().is_some_and(|s| s.error.is_some()) {
                            app.reload(true);
                        }
                        app.paused = false;
                    }
                    "step" => {
                        app.paused = true;
                        app.step();
                    }
                    "reset" => app.reload(true),
                    "speed" => app.speed = r.value.clamp(0.01, 100.0),
                    "auto_reload" => app.auto_reload = r.value != 0.0,
                    "clear_logs" => app.logs.clear(),
                    other => return Err(anyhow!("unknown action {other}")),
                }
                Ok(())
            }),
            _ => Response::from_string("not found")
                .with_status_code(404)
                .boxed(),
        };
        let _ = req.respond(resp);
    }

    fn post<T: for<'de> Deserialize<'de>>(
        &mut self,
        body: &str,
        f: impl FnOnce(&mut Self, T) -> Result<()>,
    ) -> tiny_http::ResponseBox {
        match serde_json::from_str::<T>(body)
            .map_err(anyhow::Error::from)
            .and_then(|r| f(self, r))
        {
            Ok(()) => json_resp(&json!({"ok": true})),
            Err(e) => Response::from_string(json!({"error": e.to_string()}).to_string())
                .with_status_code(400)
                .boxed(),
        }
    }
}

fn html(s: &str) -> tiny_http::ResponseBox {
    Response::from_string(s)
        .with_header(Header::from_bytes("Content-Type", "text/html; charset=utf-8").unwrap())
        .boxed()
}

fn json_resp(v: &Json) -> tiny_http::ResponseBox {
    Response::from_string(v.to_string())
        .with_header(Header::from_bytes("Content-Type", "application/json").unwrap())
        .boxed()
}

pub fn serve(
    factory: Factory,
    watch: Vec<PathBuf>,
    addr: &str,
    tick_rate: f64,
    start_paused: bool,
) -> Result<()> {
    let server = Server::http(addr).map_err(|e| anyhow!("cannot listen on {addr}: {e}"))?;
    eprintln!("swlua debugger running at http://{addr}  (Ctrl+C to quit)");
    let mut app = App {
        factory,
        sim: None,
        load_error: None,
        paused: start_paused,
        speed: 1.0,
        tick_rate,
        logs: VecDeque::new(),
        log_seq: 0,
        mtimes: vec![],
        auto_reload: true,
        tps: 0.0,
        ui_props: HashMap::new(),
        fallback_watch: watch,
    };
    app.reload(false);
    if let Some(e) = &app.load_error {
        eprintln!("{e}");
    }

    let mut next_tick = Instant::now();
    let mut last_watch = Instant::now();
    let mut tps_window = (Instant::now(), 0u32);
    let mut printed = app.log_seq;
    loop {
        // Serve everything that is waiting, then run due ticks.
        while let Ok(Some(req)) = server.try_recv() {
            app.handle(req);
        }
        if last_watch.elapsed() > Duration::from_millis(500) {
            last_watch = Instant::now();
            if app.auto_reload && app.files_changed() {
                app.log("[swlua] change detected, reloading".into());
                app.reload(true);
            }
        }
        let period = Duration::from_secs_f64(1.0 / (app.tick_rate * app.speed));
        let now = Instant::now();
        if app.paused || app.sim.is_none() {
            next_tick = now + period;
        } else {
            let mut n = 0;
            while next_tick <= now && n < 10 && !app.paused {
                app.step();
                next_tick += period;
                n += 1;
                tps_window.1 += 1;
            }
            if next_tick < now {
                // Falling behind; drop the backlog rather than spiral.
                next_tick = now + period;
            }
        }
        if tps_window.0.elapsed() >= Duration::from_secs(1) {
            app.tps = tps_window.1 as f64 / tps_window.0.elapsed().as_secs_f64();
            tps_window = (Instant::now(), 0);
        }
        // Mirror log lines to the terminal.
        for (seq, line) in app.logs.iter() {
            if *seq > printed {
                eprintln!("{line}");
            }
        }
        printed = app.log_seq;
        let wait = next_tick
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(5));
        if let Ok(Some(req)) = server.recv_timeout(wait) {
            app.handle(req);
        }
    }
}
