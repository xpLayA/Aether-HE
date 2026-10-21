//! Host-driven per-key effects, ported from `legacy/python/effects.py`.
//!
//! The firmware effects hold one colour; these animate a whole palette by
//! rendering a colour per key every frame and streaming it to the board.
//! Motion is time-based, so effects look the same at any frame rate; spawners
//! are normalised so particle density is rate-independent too.
//!
//! Reactive kinds ([`Kind::reactive`]) light up from the keys you press: the
//! caller feeds presses in with [`Engine::press`].

use std::time::{Duration, Instant};

use aether_proto::KEY_SLOTS;

pub type Rgb = [u8; 3];
pub type Frame = [Rgb; KEY_SLOTS];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Static,
    Wave,
    Breathe,
    Aurora,
    Comet,
    Tide,
    Twinkle,
    Rain,
    Striation,
    Ripple,
    Splash,
    Cross,
    Sparks,
    Afterglow,
    Heatmap,
    Lightning,
}

impl Kind {
    pub const ALL: [Kind; 16] = [
        Kind::Wave,
        Kind::Breathe,
        Kind::Aurora,
        Kind::Comet,
        Kind::Tide,
        Kind::Twinkle,
        Kind::Rain,
        Kind::Striation,
        Kind::Static,
        Kind::Ripple,
        Kind::Splash,
        Kind::Cross,
        Kind::Sparks,
        Kind::Afterglow,
        Kind::Heatmap,
        Kind::Lightning,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Static => "static",
            Kind::Wave => "wave",
            Kind::Breathe => "breathe",
            Kind::Aurora => "aurora",
            Kind::Comet => "comet",
            Kind::Tide => "tide",
            Kind::Twinkle => "twinkle",
            Kind::Rain => "rain",
            Kind::Striation => "striation",
            Kind::Ripple => "ripple",
            Kind::Splash => "splash",
            Kind::Cross => "cross",
            Kind::Sparks => "sparks",
            Kind::Afterglow => "afterglow",
            Kind::Heatmap => "heatmap",
            Kind::Lightning => "lightning",
        }
    }

    /// Lights up from key presses (needs [`Engine::press`]); dark otherwise.
    pub fn reactive(self) -> bool {
        matches!(self, Kind::Ripple | Kind::Splash | Kind::Cross | Kind::Sparks | Kind::Afterglow | Kind::Heatmap | Kind::Lightning)
    }

    pub fn from_name(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.name().eq_ignore_ascii_case(s))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Direction {
    #[default]
    Right,
    Left,
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Params {
    pub kind: Kind,
    pub palette: Vec<Rgb>,
    pub bg: Rgb,
    /// 0..1
    pub speed: f32,
    /// 0..1
    pub brightness: f32,
    pub direction: Direction,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            kind: Kind::Wave,
            palette: vec![[255, 0, 0], [255, 160, 0], [0, 255, 80], [0, 140, 255], [170, 0, 255]],
            bg: [0, 0, 0],
            speed: 0.5,
            brightness: 1.0,
            direction: Direction::Right,
        }
    }
}

/// A key's firmware index and its centre in layout coordinates.
#[derive(Clone, Copy, Debug)]
pub struct KeyPos {
    pub index: usize,
    pub x: f32,
    pub y: f32,
}

// ---- colour helpers (same maths as effects.py) ----

const GAMMA: f32 = 1.6;

fn gamma(c: Rgb) -> Rgb {
    c.map(|v| ((v as f32 / 255.0).powf(GAMMA) * 255.0).round() as u8)
}

fn scale(c: Rgb, f: f32) -> Rgb {
    if f >= 1.0 {
        return c;
    }
    if f <= 0.0 {
        return [0; 3];
    }
    c.map(|v| (v as f32 * f).clamp(0.0, 255.0) as u8)
}

fn lerp(a: Rgb, b: Rgb, t: f32) -> Rgb {
    if t <= 0.0 {
        return a;
    }
    if t >= 1.0 {
        return b;
    }
    [0, 1, 2].map(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t) as u8)
}

/// Sample a looping palette at a fractional position, blending neighbours.
fn palette_at(pal: &[Rgb], pos: f32) -> Rgb {
    let n = pal.len();
    if n == 1 {
        return pal[0];
    }
    let pos = pos.rem_euclid(n as f32);
    let i = pos as usize % n;
    lerp(pal[i], pal[(i + 1) % n], pos - pos.floor())
}

struct Particle {
    born: f32,
    lane: usize,
    color: Rgb,
}

/// One key press, spreading out from the key at (x, y).
struct Impulse {
    born: f32,
    idx: usize,
    x: f32,
    y: f32,
    color: Rgb,
    /// Sparks: velocities (board widths per second).
    sparks: Vec<(f32, f32)>,
    /// Lightning: the keys the bolt jumps through.
    path: Vec<usize>,
}

const MAX_IMPULSES: usize = 48;

/// Renders frames for one effect over a board's keys.
pub struct Engine {
    params: Params,
    palette: Vec<Rgb>,
    keys: Vec<(usize, f32, f32)>,
    /// Keys grouped into columns (top to bottom) and rows (left to right).
    columns: Vec<Vec<usize>>,
    rows: Vec<Vec<usize>>,
    spawn_norm: f32,
    twinkles: Vec<Option<(f32, Rgb)>>,
    particles: Vec<Particle>,
    last_spawn: f32,
    /// Key centres in board widths on both axes, so distances are round.
    pos: Vec<(usize, f32, f32)>,
    pending: Vec<usize>,
    impulses: Vec<Impulse>,
    heat: Vec<f32>,
    last_t: f32,
    next_color: usize,
}

const MAX_PARTICLES: usize = 96;

impl Engine {
    pub fn new(keys: &[KeyPos], params: Params, fps: f32) -> Engine {
        let (minx, maxx) = minmax(keys.iter().map(|k| k.x));
        let (miny, maxy) = minmax(keys.iter().map(|k| k.y));
        let norm = |v: f32, lo: f32, hi: f32| if hi > lo { (v - lo) / (hi - lo) } else { 0.0 };
        let nk: Vec<(usize, f32, f32)> = keys
            .iter()
            .filter(|k| k.index < KEY_SLOTS)
            .map(|k| (k.index, norm(k.x, minx, maxx), norm(k.y, miny, maxy)))
            .collect();

        let mut columns: Vec<Vec<usize>> = vec![Vec::new(); 22];
        let mut rows: Vec<Vec<usize>> = Vec::new();
        let mut row_ys: Vec<f32> = Vec::new();
        let mut sorted = nk.clone();
        sorted.sort_by(|a, b| a.2.total_cmp(&b.2).then(a.1.total_cmp(&b.1)));
        for &(i, x, y) in &sorted {
            columns[(x * 21.0).round() as usize].push(i);
            match row_ys.iter().position(|&ry| (ry - y).abs() < 0.08) {
                Some(r) => rows[r].push(i),
                None => {
                    row_ys.push(y);
                    rows.push(vec![i]);
                }
            }
        }
        columns.retain(|c| !c.is_empty());

        let palette: Vec<Rgb> =
            if params.palette.is_empty() { vec![[255; 3]] } else { params.palette.iter().map(|&c| gamma(c)).collect() };
        let w = if maxx > minx { maxx - minx } else { 1.0 };
        let pos = keys.iter().filter(|k| k.index < KEY_SLOTS).map(|k| (k.index, (k.x - minx) / w, (k.y - miny) / w)).collect();
        Engine {
            palette,
            params: Params { speed: params.speed.clamp(0.0, 1.0), brightness: params.brightness.clamp(0.0, 1.0), ..params },
            keys: nk,
            columns,
            rows,
            spawn_norm: 30.0 / fps.max(1.0),
            twinkles: vec![None; KEY_SLOTS],
            particles: Vec::new(),
            last_spawn: 0.0,
            pos,
            pending: Vec::new(),
            impulses: Vec::new(),
            heat: vec![0.0; KEY_SLOTS],
            last_t: 0.0,
            next_color: 0,
        }
    }

    /// A key went down; reactive kinds start from it on the next frame.
    pub fn press(&mut self, idx: usize) {
        if self.params.kind.reactive() && idx < KEY_SLOTS && self.pending.len() < MAX_IMPULSES {
            self.pending.push(idx);
        }
    }

    pub fn params(&self) -> &Params {
        &self.params
    }

    fn nx(&self, idx: usize) -> f32 {
        self.keys.iter().find(|k| k.0 == idx).map_or(0.0, |k| k.1)
    }

    fn flow(&self, x: f32, y: f32) -> f32 {
        match self.params.direction {
            Direction::Right => x,
            Direction::Left => 1.0 - x,
            Direction::Up => 1.0 - y,
            Direction::Down => y,
        }
    }

    fn pick(&self) -> Rgb {
        self.palette[fastrand::usize(..self.palette.len())]
    }

    /// The colours for time `t` (seconds since start). Unlisted slots are black.
    pub fn render(&mut self, t: f32) -> Frame {
        let p = self.params.clone();
        let (s, b, pal) = (p.speed, p.brightness, &self.palette.clone());
        let span = pal.len() as f32;
        let bg = scale(p.bg, b);
        let mut f: Frame = [[0; 3]; KEY_SLOTS];
        match p.kind {
            Kind::Static => {
                let n = self.keys.len().max(1) as f32;
                for (i, &(idx, ..)) in self.keys.iter().enumerate() {
                    f[idx] = scale(palette_at(pal, i as f32 / n * span), b);
                }
            }
            Kind::Wave => {
                let flow = 0.15 + s * 1.1;
                for &(idx, x, y) in &self.keys {
                    f[idx] = scale(palette_at(pal, self.flow(x, y) * span + t * flow), b);
                }
            }
            Kind::Breathe => {
                let base = palette_at(pal, t * (0.1 + s * 0.7));
                let env = 0.06 + 0.94 * ((t * (0.6 + s * 3.0)).sin() * 0.5 + 0.5);
                let c = scale(lerp(p.bg, base, env), b);
                for &(idx, ..) in &self.keys {
                    f[idx] = c;
                }
            }
            Kind::Aurora => {
                let flow = 0.05 + s * 0.4;
                for &(idx, x, y) in &self.keys {
                    let pos = (x * 0.6 + y * 0.4) * span + t * flow;
                    let wob = 0.45 + 0.55 * ((t * 0.7 + y * 3.0).sin() * 0.5 + 0.5);
                    f[idx] = scale(lerp(p.bg, palette_at(pal, pos), wob), b);
                }
            }
            Kind::Comet => {
                let head = (t * (0.12 + s * 0.55)).rem_euclid(1.0);
                let col = palette_at(pal, t * (0.2 + s * 0.8));
                let tail = 0.28;
                for &(idx, x, y) in &self.keys {
                    let d = (head - self.flow(x, y)).rem_euclid(1.0);
                    f[idx] = if d < tail { scale(lerp(p.bg, col, (1.0 - d / tail).powi(2)), b) } else { bg };
                }
            }
            Kind::Tide => {
                let level = (t * (0.2 + s)).sin() * 0.5 + 0.5;
                let col = palette_at(pal, t * (0.15 + s * 0.5));
                let up = p.direction != Direction::Up;
                for &(idx, _, y) in &self.keys {
                    let h = if up { 1.0 - y } else { y };
                    f[idx] = if h <= level {
                        scale(col, b)
                    } else if h - level < 0.10 {
                        scale(lerp(p.bg, col, 1.0 - (h - level) / 0.10), b)
                    } else {
                        bg
                    };
                }
            }
            Kind::Twinkle => {
                let life = 1.4 - s;
                let n = self.keys.len().max(1) as f32;
                let chance = (0.10 + s * 0.35) / n * 6.0 * self.spawn_norm;
                for k in 0..self.keys.len() {
                    let idx = self.keys[k].0;
                    if self.twinkles[idx].is_none() && fastrand::f32() < chance {
                        self.twinkles[idx] = Some((t, self.pick()));
                    }
                    f[idx] = match self.twinkles[idx] {
                        Some((born, c)) if (t - born) / life < 1.0 => {
                            scale(lerp(p.bg, c, ((t - born) / life * std::f32::consts::PI).sin()), b)
                        }
                        Some(_) => {
                            self.twinkles[idx] = None;
                            bg
                        }
                        None => bg,
                    };
                }
            }
            Kind::Rain => {
                for &(idx, ..) in &self.keys {
                    f[idx] = bg;
                }
                let fall = 2.6 + s * 7.0;
                const TRAIL: f32 = 5.0;
                if fastrand::f32() < (0.55 + s * 0.9) * self.spawn_norm {
                    let lane = fastrand::usize(..self.columns.len());
                    let color = self.pick();
                    self.particles.push(Particle { born: t, lane, color });
                }
                let up = p.direction == Direction::Up;
                let cols = &self.columns;
                self.particles.retain(|d| (t - d.born) * fall - TRAIL <= cols[d.lane].len() as f32);
                trim(&mut self.particles);
                for d in &self.particles {
                    let col = &cols[d.lane];
                    let head = (t - d.born) * fall;
                    for (pos, &idx) in col.iter().enumerate() {
                        let pos = if up { col.len() - 1 - pos } else { pos } as f32;
                        let dist = head - pos;
                        if (0.0..TRAIL).contains(&dist) {
                            f[idx] = if dist < 0.7 {
                                scale(lerp(d.color, [255; 3], 0.6), b)
                            } else {
                                scale(lerp(f[idx], d.color, (1.0 - dist / TRAIL).powf(1.6)), b)
                            };
                        }
                    }
                }
            }
            Kind::Ripple | Kind::Splash | Kind::Cross | Kind::Sparks | Kind::Afterglow | Kind::Heatmap | Kind::Lightning => {
                self.render_reactive(t, &mut f)
            }
            Kind::Striation => {
                for &(idx, ..) in &self.keys {
                    f[idx] = bg;
                }
                if t - self.last_spawn >= (0.45 - s * 0.36).max(0.07) {
                    self.last_spawn = t;
                    let lane = fastrand::usize(..self.rows.len());
                    let color = self.pick();
                    self.particles.push(Particle { born: t, lane, color });
                }
                let vx = 0.7 + s * 2.2;
                let trail = 0.30;
                let rtl = p.direction == Direction::Left;
                self.particles.retain(|st| (t - st.born) * vx - trail <= 1.1);
                trim(&mut self.particles);
                for st in &self.particles {
                    let head = (t - st.born) * vx;
                    for &idx in &self.rows[st.lane] {
                        let x = self.nx(idx);
                        let d = head - if rtl { 1.0 - x } else { x };
                        if (0.0..trail).contains(&d) {
                            f[idx] = scale(lerp(f[idx], st.color, (1.0 - d / trail).powi(2)), b);
                        }
                    }
                }
            }
        }
        f
    }

    /// Seconds an impulse of the current kind stays visible.
    fn life(&self) -> f32 {
        let s = self.params.speed;
        match self.params.kind {
            Kind::Ripple => 1.1 / ripple_speed(s),
            Kind::Cross => 1.0 / ripple_speed(s),
            Kind::Splash => 1.5 - s,
            Kind::Sparks => 0.9 - s * 0.4,
            Kind::Lightning => (0.6 - s * 0.3).max(0.2),
            _ => 0.0,
        }
    }

    fn render_reactive(&mut self, t: f32, f: &mut Frame) {
        let p = self.params.clone();
        let (s, b) = (p.speed, p.brightness);
        let bg = scale(p.bg, b);
        let dt = (t - self.last_t).max(0.0);
        self.last_t = t;
        for &(idx, ..) in &self.keys {
            f[idx] = bg;
        }

        for idx in std::mem::take(&mut self.pending) {
            let Some(&(_, x, y)) = self.pos.iter().find(|k| k.0 == idx) else { continue };
            let color = self.palette[self.next_color % self.palette.len()];
            self.next_color += 1;
            match p.kind {
                Kind::Afterglow => self.twinkles[idx] = Some((t, color)),
                Kind::Heatmap => self.heat[idx] = (self.heat[idx] + 0.2).min(1.0),
                _ => {
                    let sparks = if p.kind == Kind::Sparks {
                        (0..8)
                            .map(|_| {
                                let (a, v) = (fastrand::f32() * std::f32::consts::TAU, (0.25 + fastrand::f32() * 0.5) * (0.5 + s));
                                (a.cos() * v, a.sin() * v)
                            })
                            .collect()
                    } else {
                        Vec::new()
                    };
                    let path = if p.kind == Kind::Lightning { self.bolt(x, y) } else { Vec::new() };
                    self.impulses.push(Impulse { born: t, idx, x, y, color, sparks, path });
                }
            }
        }
        let life = self.life();
        self.impulses.retain(|i| t - i.born < life);
        if self.impulses.len() > MAX_IMPULSES {
            self.impulses.drain(..self.impulses.len() - MAX_IMPULSES);
        }

        // How strongly impulse `i` lights the key at (kx, ky), 0..1.
        let v = ripple_speed(s);
        let glow = |f: &mut Frame, idx: usize, c: Rgb, a: f32| {
            if a > 0.004 {
                f[idx] = lerp(f[idx], scale(c, b), a.min(1.0));
            }
        };
        match p.kind {
            Kind::Afterglow => {
                let k = 0.5 + s * 3.0;
                for &(idx, ..) in &self.keys {
                    if let Some((born, c)) = self.twinkles[idx] {
                        let a = (-(t - born) * k).exp();
                        if a < 0.004 {
                            self.twinkles[idx] = None;
                        } else {
                            glow(f, idx, c, a);
                        }
                    }
                }
            }
            Kind::Heatmap => {
                let decay = (-dt / (30.0 - s * 25.0)).exp();
                let top = (self.palette.len() - 1) as f32;
                for &(idx, ..) in &self.keys {
                    self.heat[idx] *= decay;
                    let h = self.heat[idx];
                    if h > 0.004 {
                        glow(f, idx, palette_at(&self.palette, h * top), (h * 4.0).min(1.0));
                    }
                }
            }
            _ => {
                for imp in &self.impulses {
                    let age = t - imp.born;
                    let fade = (1.0 - age / life).max(0.0);
                    let flash = (-age * 8.0).exp();
                    glow(f, imp.idx, lerp(imp.color, [255; 3], 0.4), flash);
                    match p.kind {
                        Kind::Lightning => {
                            let flicker = 0.55 + 0.45 * fastrand::f32();
                            for (n, &idx) in imp.path.iter().enumerate() {
                                // the bolt reaches each step in a few ms, then flickers out
                                if age * 60.0 >= n as f32 {
                                    glow(f, idx, lerp(imp.color, [255; 3], 0.5 * fade), fade * flicker);
                                }
                            }
                        }
                        _ => {
                            for &(idx, kx, ky) in &self.pos {
                                let (dx, dy) = (kx - imp.x, ky - imp.y);
                                let a = match p.kind {
                                    Kind::Ripple => {
                                        let d = (dx * dx + dy * dy).sqrt();
                                        (-((d - age * v) / 0.05).powi(2)).exp() * fade
                                    }
                                    Kind::Splash => {
                                        let r = 0.14 * (age / 0.12).min(1.0);
                                        let d = (dx * dx + dy * dy).sqrt();
                                        let edge = if d < r { 1.0 } else { (-((d - r) / 0.04).powi(2)).exp() };
                                        edge * fade.powf(1.5)
                                    }
                                    Kind::Cross => {
                                        let along = if dy.abs() < 0.03 {
                                            dx.abs()
                                        } else if dx.abs() < 0.035 {
                                            dy.abs()
                                        } else {
                                            continue;
                                        };
                                        (-((along - age * v) / 0.05).powi(2)).exp() * fade
                                    }
                                    Kind::Sparks => imp
                                        .sparks
                                        .iter()
                                        .map(|(vx, vy)| {
                                            let (ex, ey) = (dx - vx * age, dy - vy * age);
                                            (-(ex * ex + ey * ey) / (0.04 * 0.04)).exp()
                                        })
                                        .fold(0.0, f32::max)
                                        * fade,
                                    _ => 0.0,
                                };
                                glow(f, idx, imp.color, a);
                            }
                        }
                    }
                }
            }
        }
    }

    /// A jagged path of keys from (x, y) to the board edge, in a random direction.
    fn bolt(&self, x: f32, y: f32) -> Vec<usize> {
        let (dx, dy) = [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)][fastrand::usize(..4)];
        let (mut cx, mut cy) = (x, y);
        let mut path = Vec::new();
        for _ in 0..24 {
            let next: Vec<&(usize, f32, f32)> = self
                .pos
                .iter()
                .filter(|(_, kx, ky)| {
                    let (fwd, side) = ((kx - cx) * dx + (ky - cy) * dy, ((kx - cx) * dy - (ky - cy) * dx).abs());
                    (0.03..0.12).contains(&fwd) && side < 0.09
                })
                .collect();
            if next.is_empty() {
                break;
            }
            let &&(idx, kx, ky) = &next[fastrand::usize(..next.len())];
            path.push(idx);
            (cx, cy) = (kx, ky);
        }
        path
    }
}

/// Ripple/cross front speed in board widths per second.
fn ripple_speed(s: f32) -> f32 {
    0.35 + s * 1.1
}

fn trim(v: &mut Vec<Particle>) {
    if v.len() > MAX_PARTICLES {
        v.drain(..v.len() - MAX_PARTICLES);
    }
}

fn minmax(it: impl Iterator<Item = f32>) -> (f32, f32) {
    it.fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)))
}

/// Paces `engine` at `fps`, handing each frame to `send` until `keep_going`
/// returns false or `send` fails. `presses` is polled before every frame for
/// keys that went down. Frame time is subtracted from the sleep so slow
/// writes don't slow the animation.
pub fn run<E>(
    engine: &mut Engine,
    fps: f32,
    mut keep_going: impl FnMut() -> bool,
    mut presses: impl FnMut() -> Vec<usize>,
    mut send: impl FnMut(&Frame) -> Result<(), E>,
) -> Result<(), E> {
    let interval = Duration::from_secs_f32(1.0 / fps.max(1.0));
    let t0 = Instant::now();
    while keep_going() {
        let start = Instant::now();
        for idx in presses() {
            engine.press(idx);
        }
        let frame = engine.render(t0.elapsed().as_secs_f32());
        send(&frame)?;
        if let Some(rest) = interval.checked_sub(start.elapsed()) {
            std::thread::sleep(rest);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> Vec<KeyPos> {
        (0..80).map(|i| KeyPos { index: (i / 16) * 16 + i % 16, x: (i % 16) as f32 * 55.0, y: (i / 16) as f32 * 55.0 }).collect()
    }

    #[test]
    fn every_effect_renders_without_panicking_and_lights_keys() {
        for kind in Kind::ALL {
            let mut e = Engine::new(&grid(), Params { kind, ..Params::default() }, 28.0);
            let mut lit = 0;
            for i in 0..200 {
                if i % 20 == 0 {
                    e.press(16 * 2 + 7);
                }
                let f = e.render(i as f32 / 28.0);
                lit = lit.max(f.iter().filter(|c| **c != [0; 3]).count());
            }
            assert!(lit > 0, "{kind:?} never lit a key");
        }
    }

    #[test]
    fn only_layout_keys_are_ever_lit() {
        let keys = grid();
        let mut e = Engine::new(&keys, Params::default(), 28.0);
        let f = e.render(1.0);
        for (i, c) in f.iter().enumerate() {
            if !keys.iter().any(|k| k.index == i) {
                assert_eq!(*c, [0; 3], "slot {i}");
            }
        }
    }

    #[test]
    fn reactive_effects_stay_dark_until_a_key_is_pressed() {
        for kind in Kind::ALL.into_iter().filter(|k| k.reactive()) {
            let mut e = Engine::new(&grid(), Params { kind, ..Params::default() }, 28.0);
            for i in 0..60 {
                assert!(e.render(i as f32 / 28.0).iter().all(|c| *c == [0; 3]), "{kind:?}");
            }
        }
        let mut e = Engine::new(&grid(), Params::default(), 28.0);
        e.press(5); // ignored by non-reactive kinds
        assert!(e.pending.is_empty());
    }

    #[test]
    fn ripple_ring_grows_outward() {
        // grid keys are 55 apart over a 825-wide board: one key = 1/15 board width
        let mut e = Engine::new(&grid(), Params { kind: Kind::Ripple, speed: 0.5, ..Params::default() }, 28.0);
        let centre = 2 * 16 + 7;
        let (near, far) = (2 * 16 + 8, 2 * 16 + 13); // 1 and 6 keys to the right
        e.press(centre);
        let lum = |f: &Frame, i: usize| f[i].iter().map(|&v| v as u32).sum::<u32>();
        let early = e.render(0.05);
        // front speed 0.9 board widths/s: 6 keys = 0.4 widths, reached at ~0.44 s
        let late = e.render(0.44);
        assert!(lum(&early, near) > lum(&early, far), "starts near the key");
        assert!(lum(&late, far) > lum(&late, near), "then moves out");
        assert!(lum(&late, far) > lum(&early, far));
        assert!(e.render(5.0).iter().all(|c| *c == [0; 3]), "and fades out");
    }

    #[test]
    fn names_round_trip() {
        for k in Kind::ALL {
            assert_eq!(Kind::from_name(k.name()), Some(k));
        }
    }

    #[test]
    fn helpers_match_python() {
        assert_eq!(gamma([255, 165, 0]), [255, 127, 0]);
        assert_eq!(lerp([0, 0, 0], [255, 255, 255], 0.5), [127, 127, 127]);
        assert_eq!(palette_at(&[[0, 0, 0], [200, 0, 0]], 1.5), [100, 0, 0]);
    }
}
