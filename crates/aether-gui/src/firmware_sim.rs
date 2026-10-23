//! On-screen approximation of the keyboard's built-in (firmware) effects.
//!
//! The F75 can't report the colours its firmware is showing (`0x33` returns
//! black, tested 2026-10-21), so the Lighting page animates a look-alike of
//! each mode from its name and parameters. It's a guide to the motion, not a
//! pixel-exact copy of the firmware.

use std::f32::consts::TAU;
use std::time::Instant;

use aether_effects::Frame;
use aether_proto::{KEY_SLOTS, Lighting};

#[derive(Clone, Copy)]
struct Key {
    index: usize,
    x: f32,
    y: f32,
}

struct Drop {
    born: f32,
    lane: usize,
    hue: f32,
}

pub struct Sim {
    keys: Vec<Key>,
    columns: Vec<Vec<usize>>,
    t0: Instant,
    /// (key index, time pressed): demo presses plus clicks on the on-screen keys.
    presses: Vec<(usize, f32)>,
    next_demo: f32,
    drops: Vec<Drop>,
    next_drop: f32,
    stars: Vec<(usize, f32, f32)>,
}

fn hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = h.rem_euclid(1.0) * 6.0;
    let c = v * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r + m, g + m, b + m]
}

fn to8(c: [f32; 3], level: f32) -> [u8; 3] {
    c.map(|v| (v.clamp(0.0, 1.0) * level * 255.0).round() as u8)
}

impl Sim {
    /// `keys`: (firmware index, x, y) in layout units.
    pub fn new(keys: &[(usize, f32, f32)]) -> Sim {
        let (minx, maxx) = keys.iter().fold((f32::MAX, f32::MIN), |(a, b), k| (a.min(k.1), b.max(k.1)));
        let (miny, maxy) = keys.iter().fold((f32::MAX, f32::MIN), |(a, b), k| (a.min(k.2), b.max(k.2)));
        let norm = |v: f32, lo: f32, hi: f32| if hi > lo { (v - lo) / (hi - lo) } else { 0.0 };
        let keys: Vec<Key> = keys.iter().map(|&(index, x, y)| Key { index, x: norm(x, minx, maxx), y: norm(y, miny, maxy) }).collect();
        let mut columns = vec![Vec::new(); 18];
        let mut sorted = keys.clone();
        sorted.sort_by(|a, b| a.y.total_cmp(&b.y));
        for k in &sorted {
            columns[(k.x * 17.0).round() as usize].push(k.index);
        }
        columns.retain(|c| !c.is_empty());
        Sim { keys, columns, t0: Instant::now(), presses: Vec::new(), next_demo: 0.0, drops: Vec::new(), next_drop: 0.0, stars: Vec::new() }
    }

    fn now(&self) -> f32 {
        self.t0.elapsed().as_secs_f32()
    }

    /// A click on the on-screen key `index` (drives the reactive effects).
    pub fn press(&mut self, index: usize) {
        let t = self.now();
        self.presses.push((index, t));
    }

    fn key(&self, index: usize) -> Option<Key> {
        self.keys.iter().copied().find(|k| k.index == index)
    }

    pub fn render(&mut self, l: &Lighting, multicolor: bool) -> Frame {
        let t = self.now();
        let mut f: Frame = [[0; 3]; KEY_SLOTS];
        if l.mode == 0 {
            return f;
        }
        let level = 0.12 + 0.88 * (l.brightness as f32 / 5.0);
        let rate = 0.35 + 0.33 * l.speed as f32;
        let base = l.rgb.map(|v| v as f32 / 255.0);
        // The colour for a key: the chosen colour, or a rainbow hue when the
        // effect picks its own colours.
        let colour = |hue: f32| if multicolor { hsv(hue, 1.0, 1.0) } else { base };

        // Demo presses so the reactive modes show something without typing.
        if t >= self.next_demo && !self.keys.is_empty() {
            let k = self.keys[fastrand::usize(..self.keys.len())].index;
            self.presses.push((k, t));
            self.next_demo = t + 0.45 + fastrand::f32() * 0.5;
        }
        self.presses.retain(|&(_, at)| t - at < 3.0);

        let presses: Vec<(Key, f32)> = self.presses.iter().filter_map(|&(i, at)| self.key(i).map(|k| (k, t - at))).collect();
        let mut put = |i: usize, c: [f32; 3], a: f32| {
            let cur = f[i].map(|v| v as f32 / 255.0 / level.max(0.01));
            let mixed = [0, 1, 2].map(|j| cur[j] + (c[j] - cur[j]) * a.clamp(0.0, 1.0));
            f[i] = to8(mixed, level);
        };

        match l.mode {
            // Static
            1 => {
                for k in &self.keys {
                    put(k.index, colour(k.x * 0.85), 1.0);
                }
            }
            // Reactive Single / Reactive / Double Tap: pressed keys light and fade
            2 | 13 | 14 => {
                for (k, age) in &presses {
                    let fade = (1.0 - age * rate * 0.8).max(0.0);
                    put(k.index, colour(k.x + age * 0.2), fade);
                    if l.mode == 14 {
                        // a ring spreading from the press
                        let r = age * rate * 0.45;
                        for o in &self.keys {
                            let d = ((o.x - k.x).powi(2) + ((o.y - k.y) * 0.4).powi(2)).sqrt();
                            if (d - r).abs() < 0.035 {
                                put(o.index, colour(k.x + d), fade * 0.9);
                            }
                        }
                    }
                }
            }
            // Extinguish Single: everything lit, pressed keys go dark and recover
            3 => {
                for k in &self.keys {
                    put(k.index, colour(k.x * 0.85 + t * 0.05), 1.0);
                }
                for (k, age) in &presses {
                    put(k.index, [0.0; 3], (1.0 - age * rate * 0.7).max(0.0));
                }
            }
            // Starry Sky: random twinkles
            4 => {
                if fastrand::f32() < 0.25 * rate {
                    let k = self.keys[fastrand::usize(..self.keys.len())].index;
                    self.stars.push((k, t, fastrand::f32()));
                }
                self.stars.retain(|s| t - s.1 < 1.6);
                for &(i, at, hue) in &self.stars {
                    put(i, colour(hue), ((t - at) / 1.6 * std::f32::consts::PI).sin());
                }
            }
            // Snowfall / Gentle Rain: drops falling down columns
            5 | 18 => {
                if t >= self.next_drop && !self.columns.is_empty() {
                    self.drops.push(Drop { born: t, lane: fastrand::usize(..self.columns.len()), hue: fastrand::f32() });
                    self.next_drop = t + 0.18 / rate;
                }
                let speed = 2.5 * rate;
                let cols = &self.columns;
                self.drops.retain(|d| (t - d.born) * speed < cols[d.lane].len() as f32 + 3.0);
                for d in &self.drops {
                    let col = &self.columns[d.lane];
                    let head = (t - d.born) * speed;
                    for (row, &i) in col.iter().enumerate() {
                        let dist = head - row as f32;
                        if (0.0..2.5).contains(&dist) {
                            let c = if l.mode == 5 && !multicolor { [1.0, 1.0, 1.0] } else { colour(d.hue) };
                            put(i, c, 1.0 - dist / 2.5);
                        }
                    }
                }
            }
            // Blossoming / Ripple Spread: rings from random points or presses
            6 | 15 => {
                for (k, age) in &presses {
                    let r = age * rate * 0.35;
                    let fade = (1.0 - age / 2.5).max(0.0);
                    for o in &self.keys {
                        let d = ((o.x - k.x).powi(2) + ((o.y - k.y) * 0.4).powi(2)).sqrt();
                        let band = 1.0 - ((d - r).abs() / 0.06).min(1.0);
                        if band > 0.0 {
                            put(o.index, colour(k.x + age * 0.15), band * fade);
                        }
                    }
                }
            }
            // Dynamic Breathing
            7 => {
                let env = 0.5 - 0.5 * (t * rate * 1.6).cos();
                let hue = (t * rate * 0.8 / TAU).floor() * 0.17;
                for k in &self.keys {
                    put(k.index, colour(hue), env);
                }
            }
            // Spectrum Cycle: the whole board through the rainbow
            8 => {
                for k in &self.keys {
                    put(k.index, hsv(t * rate * 0.12, 1.0, 1.0), 1.0);
                }
            }
            // Color Spring: rings from the centre
            9 => {
                for k in &self.keys {
                    let d = ((k.x - 0.5).powi(2) + ((k.y - 0.5) * 0.45).powi(2)).sqrt();
                    put(k.index, colour(d * 1.6 - t * rate * 0.35), 1.0);
                }
            }
            // Colorful Cross: lines along the pressed key's row and column
            10 => {
                for (k, age) in &presses {
                    let fade = (1.0 - age * rate * 0.6).max(0.0);
                    for o in &self.keys {
                        if (o.y - k.y).abs() < 0.05 || (o.x - k.x).abs() < 0.03 {
                            put(o.index, colour(k.x + o.x * 0.3), fade);
                        }
                    }
                }
            }
            // Drifting: horizontal rainbow drift
            11 => {
                for k in &self.keys {
                    put(k.index, colour(k.x - t * rate * 0.18), 1.0);
                }
            }
            // Twisting Path: diagonal swirl
            12 => {
                for k in &self.keys {
                    put(k.index, colour(k.x * 0.7 + k.y * 0.5 + (k.x * 3.0 + t * rate).sin() * 0.08 - t * rate * 0.15), 1.0);
                }
            }
            // Flowing: vertical flow
            16 => {
                for k in &self.keys {
                    put(k.index, colour(k.y * 0.8 - t * rate * 0.2), 1.0);
                }
            }
            // Mountain Peaks: columns rising and falling
            17 => {
                for (ci, col) in self.columns.iter().enumerate() {
                    let h = 0.5 + 0.5 * (t * rate * 1.3 + ci as f32 * 0.7).sin() * (t * rate * 0.6 + ci as f32 * 0.3).cos();
                    let lit = (h * col.len() as f32).round() as usize;
                    for (row, &i) in col.iter().rev().enumerate() {
                        if row < lit {
                            put(i, colour(row as f32 / 6.0), 1.0);
                        }
                    }
                }
            }
            // Shuttle: a band sweeping back and forth
            19 => {
                let pos = 0.5 + 0.5 * (t * rate * 0.9).sin();
                for k in &self.keys {
                    let d = (k.x - pos).abs();
                    if d < 0.14 {
                        put(k.index, colour(pos), 1.0 - d / 0.14);
                    }
                }
            }
            // Custom (per-key table) and anything unknown: a plain lit board
            _ => {
                for k in &self.keys {
                    put(k.index, [0.55, 0.6, 0.7], 0.6);
                }
            }
        }
        f
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> Vec<(usize, f32, f32)> {
        (0..80).map(|i| ((i / 16) * 16 + i % 16, (i % 16) as f32 * 55.0, (i / 16) as f32 * 55.0)).collect()
    }

    fn light(mode: u8) -> Lighting {
        Lighting { mode, rgb: [255, 0, 0], alpha: 255, color_mode: 0, brightness: 5, speed: 3, reserved: [0; 3] }
    }

    #[test]
    fn every_firmware_mode_animates_only_real_keys() {
        let keys = grid();
        for mode in 0..=20u8 {
            let mut sim = Sim::new(&keys);
            sim.press(34);
            let mut lit_ever = false;
            for _ in 0..60 {
                sim.t0 -= std::time::Duration::from_millis(50); // advance time without sleeping
                let f = sim.render(&light(mode), mode % 2 == 0);
                for (i, c) in f.iter().enumerate() {
                    if *c != [0; 3] {
                        lit_ever = true;
                        assert!(keys.iter().any(|k| k.0 == i), "mode {mode} lit slot {i} that isn't a key");
                    }
                }
            }
            assert_eq!(lit_ever, mode != 0, "mode {mode}");
        }
    }

    #[test]
    fn brightness_scales_output() {
        let keys = grid();
        let mut sim = Sim::new(&keys);
        let hi = sim.render(&light(1), false)[keys[0].0];
        let lo = sim.render(&Lighting { brightness: 0, ..light(1) }, false)[keys[0].0];
        assert!(hi[0] > lo[0] && lo[0] > 0, "{hi:?} vs {lo:?}");
    }
}
