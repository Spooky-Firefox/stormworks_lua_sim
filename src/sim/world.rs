//! A small 3D world for testing sensor scripts: the player's vehicle, moving
//! targets, radars on pivots and a physics sensor.
//!
//! Coordinates follow the game's physics sensor: X = east, Y = up,
//! Z = north. Vehicle-local axes are X = right, Y = up, Z = forward.
//! Orientation is stored as a local-to-world rotation matrix; Euler angles
//! use R = Rz(z) * Ry(y) * Rx(x).

use std::collections::HashMap;
use std::f64::consts::TAU;

pub const DT: f64 = 1.0 / 60.0;
pub const RADAR_SLOTS: usize = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct V3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

pub fn v3(x: f64, y: f64, z: f64) -> V3 {
    V3 { x, y, z }
}

impl V3 {
    pub fn add(self, o: V3) -> V3 {
        v3(self.x + o.x, self.y + o.y, self.z + o.z)
    }
    pub fn sub(self, o: V3) -> V3 {
        v3(self.x - o.x, self.y - o.y, self.z - o.z)
    }
    pub fn scale(self, k: f64) -> V3 {
        v3(self.x * k, self.y * k, self.z * k)
    }
    pub fn dot(self, o: V3) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn len(self) -> f64 {
        self.dot(self).sqrt()
    }
}

/// Row-major 3x3 matrix.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct M3(pub [[f64; 3]; 3]);

impl M3 {
    pub const IDENTITY: M3 = M3([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);

    pub fn rx(a: f64) -> M3 {
        let (s, c) = a.sin_cos();
        M3([[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]])
    }
    /// Positive angles turn the forward axis (Z) towards the right (X).
    pub fn ry(a: f64) -> M3 {
        let (s, c) = a.sin_cos();
        M3([[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]])
    }
    pub fn rz(a: f64) -> M3 {
        let (s, c) = a.sin_cos();
        M3([[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]])
    }

    pub fn mul(&self, o: &M3) -> M3 {
        let mut r = [[0.0; 3]; 3];
        for (i, row) in r.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = (0..3).map(|k| self.0[i][k] * o.0[k][j]).sum();
            }
        }
        M3(r)
    }

    pub fn apply(&self, v: V3) -> V3 {
        let m = &self.0;
        v3(
            m[0][0] * v.x + m[0][1] * v.y + m[0][2] * v.z,
            m[1][0] * v.x + m[1][1] * v.y + m[1][2] * v.z,
            m[2][0] * v.x + m[2][1] * v.y + m[2][2] * v.z,
        )
    }

    pub fn transpose(&self) -> M3 {
        let m = &self.0;
        M3([
            [m[0][0], m[1][0], m[2][0]],
            [m[0][1], m[1][1], m[2][1]],
            [m[0][2], m[1][2], m[2][2]],
        ])
    }

    pub fn from_euler(x: f64, y: f64, z: f64) -> M3 {
        M3::rz(z).mul(&M3::ry(y)).mul(&M3::rx(x))
    }

    pub fn to_euler(self) -> V3 {
        let m = &self.0;
        let y = (-m[2][0]).clamp(-1.0, 1.0).asin();
        v3(m[2][1].atan2(m[2][2]), y, m[1][0].atan2(m[0][0]))
    }

    /// Heading (clockwise from north), pitch (up) and roll (right wing
    /// down), all in radians.
    pub fn from_hpr(heading: f64, pitch: f64, roll: f64) -> M3 {
        M3::ry(heading).mul(&M3::rx(-pitch)).mul(&M3::rz(-roll))
    }

    pub fn right(&self) -> V3 {
        v3(self.0[0][0], self.0[1][0], self.0[2][0])
    }
    pub fn up(&self) -> V3 {
        v3(self.0[0][1], self.0[1][1], self.0[2][1])
    }
    pub fn forward(&self) -> V3 {
        v3(self.0[0][2], self.0[1][2], self.0[2][2])
    }

    /// Re-orthonormalises after many small rotations.
    fn normalized(&self) -> M3 {
        let f = self.forward();
        let f = f.scale(1.0 / f.len());
        let u = self.up();
        let u = u.sub(f.scale(u.dot(f)));
        let u = u.scale(1.0 / u.len());
        // Left-handed basis: right = up x forward.
        let r = v3(
            u.y * f.z - u.z * f.y,
            u.z * f.x - u.x * f.z,
            u.x * f.y - u.y * f.x,
        );
        M3([[r.x, u.x, f.x], [r.y, u.y, f.y], [r.z, u.z, f.z]])
    }
}

#[derive(Clone, Debug)]
pub struct Body {
    pub pos: V3,
    pub rot: M3,
    /// World-frame velocity, m/s. Ignored while `speed` is set.
    pub vel: V3,
    /// When set, the body moves along its own forward axis at this speed,
    /// like an aircraft.
    pub speed: Option<f64>,
    /// Local-frame angular velocity in rad/s: x = pitch down, y = yaw right,
    /// z = roll left.
    pub ang_vel: V3,
}

impl Default for Body {
    fn default() -> Self {
        Body {
            pos: V3::default(),
            rot: M3::IDENTITY,
            vel: V3::default(),
            speed: None,
            ang_vel: V3::default(),
        }
    }
}

impl Body {
    pub fn velocity(&self) -> V3 {
        match self.speed {
            Some(s) => self.rot.forward().scale(s),
            None => self.vel,
        }
    }

    fn step(&mut self, dt: f64) {
        let w = self.ang_vel;
        if w.len() > 0.0 {
            let d = M3::rx(w.x * dt)
                .mul(&M3::ry(w.y * dt))
                .mul(&M3::rz(w.z * dt));
            self.rot = self.rot.mul(&d).normalized();
            if self.speed.is_none() {
                // Turning bodies without a speed carry their velocity around.
                let yaw = M3::ry(w.y * dt);
                self.vel = yaw.apply(self.vel);
            }
        }
        self.pos = self.pos.add(self.velocity().scale(dt));
    }
}

#[derive(Clone, Debug)]
pub struct Target {
    pub id: u32,
    pub name: String,
    pub body: Body,
    pub trail: Vec<V3>,
}

#[derive(Clone, Debug, Default)]
pub struct Contact {
    pub target: u32,
    pub distance: f64,
    /// Turns, relative to the radar's facing; positive = right.
    pub azimuth: f64,
    /// Turns, positive = up.
    pub elevation: f64,
    pub ticks_since: u64,
    /// World position the radar reported (with noise), for the 3D view.
    pub world_pos: V3,
}

#[derive(Clone, Debug)]
pub struct Radar {
    pub id: u32,
    pub name: String,
    /// Mount position in vehicle-local coordinates.
    pub offset: V3,
    /// Full field of view in turns (as the radar's properties).
    pub fov_x: f64,
    pub fov_y: f64,
    pub range: f64,
    /// Standard deviation of position error, as a fraction of distance.
    pub noise: f64,
    /// Script output channels (1-based) holding pivot yaw / pitch in turns.
    pub yaw_output: Option<usize>,
    pub pitch_output: Option<usize>,
    /// Current pivot angles in turns.
    pub yaw: f64,
    pub pitch: f64,
    /// Pivot target angles in turns.
    pub yaw_target: f64,
    pub pitch_target: f64,
    /// Maximum pivot speed in turns per second (None = instant).
    pub slew: Option<f64>,
    /// Contacts stay in their slot this many ticks after leaving the beam.
    pub hold_ticks: u64,
    pub slots: [Option<Contact>; RADAR_SLOTS],
}

impl Radar {
    /// Radar-to-world rotation.
    pub fn rotation(&self, vehicle: &M3) -> M3 {
        vehicle
            .mul(&M3::ry(self.yaw * TAU))
            .mul(&M3::rx(-self.pitch * TAU))
    }

    pub fn position(&self, vehicle: &Body) -> V3 {
        vehicle.pos.add(vehicle.rot.apply(self.offset))
    }
}

pub struct World {
    pub vehicle: Body,
    pub targets: Vec<Target>,
    pub radars: Vec<Radar>,
    next_id: u32,
    rng: u64,
    ticks: u64,
}

impl Default for World {
    fn default() -> Self {
        World {
            vehicle: Body::default(),
            targets: vec![],
            radars: vec![],
            next_id: 1,
            rng: 0x2545F4914F6CDD1D,
            ticks: 0,
        }
    }
}

const TRAIL_LEN: usize = 120;
const TRAIL_EVERY: u64 = 15;

impl World {
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty() && self.radars.is_empty()
    }

    pub fn new_id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id - 1
    }

    fn gauss(&mut self) -> f64 {
        // Box-Muller on a xorshift generator; deterministic between runs.
        let mut u = || {
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            ((self.rng >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        };
        let (a, b) = (u(), u());
        (-2.0 * a.ln()).sqrt() * (TAU * b).cos()
    }

    pub fn radar_mut(&mut self, id: u32) -> Option<&mut Radar> {
        self.radars.iter_mut().find(|r| r.id == id)
    }

    pub fn target_mut(&mut self, id: u32) -> Option<&mut Target> {
        self.targets.iter_mut().find(|t| t.id == id)
    }

    /// Advances one tick. `outputs` are the script's number outputs, used to
    /// drive radar pivots.
    pub fn step(&mut self, outputs: &[f64]) {
        self.ticks += 1;
        self.vehicle.step(DT);
        for t in &mut self.targets {
            t.body.step(DT);
            if self.ticks.is_multiple_of(TRAIL_EVERY) {
                t.trail.push(t.body.pos);
                if t.trail.len() > TRAIL_LEN {
                    t.trail.remove(0);
                }
            }
        }
        for i in 0..self.radars.len() {
            let r = &mut self.radars[i];
            let read = |ch: Option<usize>| ch.and_then(|c| outputs.get(c.wrapping_sub(1)).copied());
            if let Some(v) = read(r.yaw_output) {
                r.yaw_target = v;
            }
            if let Some(v) = read(r.pitch_output) {
                r.pitch_target = v;
            }
            let max = r.slew.map_or(f64::INFINITY, |s| s * DT);
            r.yaw += (r.yaw_target - r.yaw).clamp(-max, max);
            r.pitch += (r.pitch_target - r.pitch).clamp(-max, max);
            self.scan(i);
        }
    }

    fn scan(&mut self, i: usize) {
        let (pos, rot_t) = {
            let r = &self.radars[i];
            (
                r.position(&self.vehicle),
                r.rotation(&self.vehicle.rot).transpose(),
            )
        };
        let mut seen: HashMap<u32, Contact> = HashMap::new();
        for t in 0..self.targets.len() {
            let (id, tpos) = (self.targets[t].id, self.targets[t].body.pos);
            let r = &self.radars[i];
            let local = rot_t.apply(tpos.sub(pos));
            let dist = local.len();
            if dist > r.range || dist < 1e-6 {
                continue;
            }
            let az = local.x.atan2(local.z) / TAU;
            let el = local
                .y
                .atan2((local.x * local.x + local.z * local.z).sqrt())
                / TAU;
            if az.abs() > r.fov_x / 2.0 || el.abs() > r.fov_y / 2.0 {
                continue;
            }
            let sigma = r.noise * dist;
            let noisy = local.add(v3(self.gauss(), self.gauss(), self.gauss()).scale(sigma));
            let nd = noisy.len();
            let c = Contact {
                target: id,
                distance: nd,
                azimuth: noisy.x.atan2(noisy.z) / TAU,
                elevation: noisy
                    .y
                    .atan2((noisy.x * noisy.x + noisy.z * noisy.z).sqrt())
                    / TAU,
                ticks_since: 0,
                world_pos: pos.add(rot_t.transpose().apply(noisy)),
            };
            seen.insert(id, c);
        }
        let r = &mut self.radars[i];
        // Existing contacts keep their slot.
        for slot in r.slots.iter_mut() {
            if let Some(c) = slot {
                match seen.remove(&c.target) {
                    Some(fresh) => *c = fresh,
                    None => {
                        c.ticks_since += 1;
                        if c.ticks_since > r.hold_ticks {
                            *slot = None;
                        }
                    }
                }
            }
        }
        let mut fresh: Vec<Contact> = seen.into_values().collect();
        fresh.sort_by(|a, b| a.distance.total_cmp(&b.distance));
        for c in fresh {
            if let Some(slot) = r.slots.iter_mut().find(|s| s.is_none()) {
                *slot = Some(c);
            }
        }
    }

    /// Radar composite: per slot (distance, azimuth, elevation, time since
    /// detection in ticks) and a detected flag.
    pub fn radar_composite(&self, id: u32) -> Option<([f64; 32], [bool; 8])> {
        let r = self.radars.iter().find(|r| r.id == id)?;
        let mut n = [0.0; 32];
        let mut b = [false; 8];
        for (i, s) in r.slots.iter().enumerate() {
            if let Some(c) = s {
                n[i * 4] = c.distance;
                n[i * 4 + 1] = c.azimuth;
                n[i * 4 + 2] = c.elevation;
                n[i * 4 + 3] = c.ticks_since as f64;
                b[i] = c.ticks_since <= r.hold_ticks;
            }
        }
        Some((n, b))
    }

    /// Physics sensor composite numbers 1-18.
    pub fn physics(&self) -> [f64; 18] {
        let v = &self.vehicle;
        let e = v.rot.to_euler();
        let vel = v.velocity();
        let local_vel = v.rot.transpose().apply(vel);
        let w = v.ang_vel;
        let tilt = |axis: V3| axis.y.clamp(-1.0, 1.0).asin() / TAU;
        let f = v.rot.forward();
        [
            v.pos.x,
            v.pos.y,
            v.pos.z,
            e.x,
            e.y,
            e.z,
            local_vel.x,
            local_vel.y,
            local_vel.z,
            w.x,
            w.y,
            w.z,
            vel.len(),
            w.len(),
            tilt(v.rot.forward()),
            tilt(v.rot.right()),
            tilt(v.rot.up()),
            // Compass: 0 = north, counter-clockwise positive (east = -0.25).
            -f.x.atan2(f.z) / TAU,
        ]
    }

    /// Human-friendly heading/pitch/roll of the vehicle in degrees.
    pub fn vehicle_hpr(&self) -> (f64, f64, f64) {
        hpr(&self.vehicle.rot)
    }
}

pub fn hpr(rot: &M3) -> (f64, f64, f64) {
    let f = rot.forward();
    let heading = f.x.atan2(f.z).to_degrees().rem_euclid(360.0);
    let pitch = f.y.clamp(-1.0, 1.0).asin().to_degrees();
    // Roll: angle of the right axis below the horizon, about forward.
    let r = rot.right();
    let roll = (-r.y).atan2(rot.up().y).to_degrees();
    (heading, pitch, roll)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn euler_roundtrip() {
        let m = M3::from_euler(0.3, -0.7, 1.2);
        let e = m.to_euler();
        assert!(
            close(e.x, 0.3) && close(e.y, -0.7) && close(e.z, 1.2),
            "{e:?}"
        );
    }

    #[test]
    fn heading_pitch_roll() {
        let m = M3::from_hpr(90f64.to_radians(), 0.0, 0.0);
        assert!(close(m.forward().x, 1.0), "heading 90 faces east");
        let m = M3::from_hpr(0.0, 30f64.to_radians(), 0.0);
        assert!(close(m.forward().y, 0.5), "positive pitch is nose up");
        let (h, p, r) = hpr(&M3::from_hpr(
            45f64.to_radians(),
            10f64.to_radians(),
            20f64.to_radians(),
        ));
        assert!(
            close(h, 45.0) && close(p, 10.0) && close(r, 20.0),
            "{h} {p} {r}"
        );
    }

    #[test]
    fn radar_sees_target_in_beam() {
        let mut w = World::default();
        let id = w.new_id();
        w.targets.push(Target {
            id,
            name: "t".into(),
            body: Body {
                pos: v3(100.0, 0.0, 1000.0),
                ..Default::default()
            },
            trail: vec![],
        });
        let rid = w.new_id();
        w.radars.push(Radar {
            id: rid,
            name: "r".into(),
            offset: V3::default(),
            fov_x: 0.05,
            fov_y: 0.05,
            range: 5000.0,
            noise: 0.0,
            yaw_output: Some(1),
            pitch_output: None,
            yaw: 0.0,
            pitch: 0.0,
            yaw_target: 0.0,
            pitch_target: 0.0,
            slew: None,
            hold_ticks: 0,
            slots: Default::default(),
        });
        w.step(&[0.0]);
        let (n, b) = w.radar_composite(rid).unwrap();
        assert!(b[0]);
        assert!(close(n[0], (100f64 * 100.0 + 1000.0 * 1000.0).sqrt()));
        assert!(
            close(n[1], 100f64.atan2(1000.0) / TAU),
            "positive azimuth to the right"
        );
        // Turning the pivot away loses the target.
        w.step(&[0.25]);
        assert!(!w.radar_composite(rid).unwrap().1[0]);
    }

    #[test]
    fn aircraft_motion() {
        let mut b = Body {
            speed: Some(60.0),
            ..Default::default()
        };
        for _ in 0..60 {
            b.step(DT);
        }
        assert!(close(b.pos.z, 60.0));
        // Yawing right at 90 deg/s for a second ends up facing east.
        let mut b = Body {
            ang_vel: v3(0.0, 90f64.to_radians(), 0.0),
            ..Default::default()
        };
        for _ in 0..60 {
            b.step(DT);
        }
        assert!((b.rot.forward().x - 1.0).abs() < 1e-3);
    }
}
