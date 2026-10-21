//! Host-driven per-key effects, ported from `legacy/python/effects.py`.
//!
//! The firmware effects hold one colour; these animate a whole palette by
//! rendering a colour per key every frame and streaming it to the board.
//! Motion is time-based, so effects look the same at any frame rate; spawners
//! are normalised so particle density is rate-independent too.

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
}

impl Kind {
    pub const ALL: [Kind; 9] = [
        Kind::Wave,
        Kind::Breathe,
        Kind::Aurora,
        Kind::Comet,
        Kind::Tide,
        Kind::Twinkle,
        Kind::Rain,
        Kind::Striation,
        Kind::Static,
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
        }
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
/// returns false or `send` fails. Frame time is subtracted from the sleep so
/// slow writes don't slow the animation.
pub fn run<E>(
    engine: &mut Engine,
    fps: f32,
    mut keep_going: impl FnMut() -> bool,
    mut send: impl FnMut(&Frame) -> Result<(), E>,
) -> Result<(), E> {
    let interval = Duration::from_secs_f32(1.0 / fps.max(1.0));
    let t0 = Instant::now();
    while keep_going() {
        let start = Instant::now();
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
