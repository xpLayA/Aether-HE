//! `aether`: control an Aether-supported keyboard from the terminal.
//! Every command that changes the board saves a backup first.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use aether_device::{Backup, Keyboard, Registry, backups_dir, open_first};
use aether_effects::{Direction, Engine, KeyPos, Kind, Params};
use aether_proto::{Actuation, Lighting, raw_to_mm};
use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "aether", version, about = "Control AULA Hall-effect keyboards (AULA F75 HE)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show supported boards and whether one is connected.
    List,
    /// Read the connected board's identity and firmware version.
    Info,
    /// Save lighting + actuation to a backup file.
    Backup {
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Restore a backup (default: the newest one for this board).
    Restore {
        file: Option<PathBuf>,
        /// Restore the oldest backup: the board as it was before Aether first changed it.
        #[arg(long, conflicts_with = "file")]
        original: bool,
    },
    /// List saved backups, oldest first (unix time, board, file).
    Backups,
    /// Firmware lighting.
    #[command(subcommand)]
    Light(LightCmd),
    /// Actuation point and rapid trigger.
    #[command(subcommand)]
    Actuation(ActCmd),
    /// Run a host-driven effect until Ctrl+C (or --seconds), then restore the lighting.
    Effect {
        /// wave, breathe, aurora, comet, tide, twinkle, rain, striation, static
        name: String,
        /// Comma-separated hex colours, e.g. ff0000,00ff00,0000ff
        #[arg(long)]
        colors: Option<String>,
        #[arg(long, default_value_t = 0.5)]
        speed: f32,
        #[arg(long, default_value_t = 1.0)]
        brightness: f32,
        /// right, left, up, down
        #[arg(long, default_value = "right")]
        direction: String,
        #[arg(long)]
        seconds: Option<f32>,
    },
}

#[derive(Subcommand)]
enum LightCmd {
    /// Show the active firmware effect.
    Get,
    /// List this board's firmware effects.
    Modes,
    /// Change the firmware effect; unspecified settings are kept.
    Set {
        /// Mode name (as in `light modes`) or number.
        #[arg(long)]
        mode: Option<String>,
        /// Hex colour, e.g. ff0000.
        #[arg(long)]
        color: Option<String>,
        #[arg(long)]
        brightness: Option<u8>,
        #[arg(long)]
        speed: Option<u8>,
    },
}

#[derive(Subcommand)]
enum ActCmd {
    /// Show actuation per key (keys never configured use the firmware default).
    Get {
        /// Comma-separated key names, e.g. W,A,S,D (default: all configured keys).
        #[arg(long)]
        keys: Option<String>,
    },
    /// Set actuation for keys; with --rt also enables rapid trigger.
    Set {
        #[arg(long)]
        keys: String,
        /// Actuation point in mm.
        #[arg(long)]
        travel: f64,
        /// Rapid trigger sensitivity: PRESS or PRESS,RELEASE in mm.
        #[arg(long)]
        rt: Option<String>,
    },
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    if let Err(e) = run(Cli::parse()) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn connect() -> Result<Box<dyn Keyboard>> {
    Ok(aether_device::connect().context("could not connect to the keyboard")?.0)
}

/// Save a backup before any write; refuse to write if that fails.
fn backup_first(kb: &mut dyn Keyboard) -> Result<()> {
    let path = Backup::snapshot(kb).context("could not save a backup, nothing was changed")?;
    eprintln!("backup: {}", path.display());
    Ok(())
}

fn parse_color(s: &str) -> Result<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    let v = u32::from_str_radix(s, 16).ok().filter(|_| s.len() == 6).ok_or_else(|| anyhow!("bad colour {s:?}"))?;
    Ok([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

fn key_indices(kb: &dyn Keyboard, list: &str) -> Result<Vec<usize>> {
    list.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|n| kb.layout().find(n).map(|k| k.index).ok_or_else(|| anyhow!("no key named {n:?}")))
        .collect()
}

fn run(cli: Cli) -> Result<()> {
    match cli.cmd {
        Cmd::List => {
            let reg = Registry::load()?;
            let connected = open_first(&reg).ok().map(|kb| kb.board().slug.clone());
            for b in &reg.boards {
                let mark = if connected.as_deref() == Some(&b.slug) { "connected" } else { "-" };
                println!("{:<14} {:04X}:{:04X}  {:<10} {}", b.slug, b.vid, b.pid, mark, b.name);
            }
        }
        Cmd::Info => {
            let mut kb = connect()?;
            let i = kb.device_info()?;
            println!("{} ({:04X}:{:04X}), firmware V{:X}.{:02X}", kb.board().name, i.vid, i.pid, i.firmware >> 8, i.firmware & 0xFF);
            println!("keys: {}  backups: {}", kb.layout().keys.len(), backups_dir()?.display());
        }
        Cmd::Backup { out } => {
            let mut kb = connect()?;
            let b = Backup::capture(kb.as_mut())?;
            let path = match out {
                Some(p) => {
                    b.save_to(&p)?;
                    p
                }
                None => b.save_in(&backups_dir()?)?,
            };
            println!("saved {}", path.display());
        }
        Cmd::Restore { file, original } => {
            let mut kb = connect()?;
            let slug = kb.board().slug.clone();
            let path = match file {
                Some(p) => p,
                None if original => Backup::oldest(&slug)?.ok_or_else(|| anyhow!("no backups yet"))?,
                None => Backup::latest(&slug)?.ok_or_else(|| anyhow!("no backups yet"))?,
            };
            Backup::load(&path)?.restore(kb.as_mut())?;
            println!("restored {} (actuation verified)", path.display());
        }
        Cmd::Backups => {
            for (t, p) in Backup::list()? {
                let board = p.file_stem().and_then(|s| s.to_str()).and_then(|s| s.rsplit_once('-')).map_or("?", |(b, _)| b);
                println!("{t}\t{board}\t{}", p.display());
            }
        }
        Cmd::Light(c) => light(c)?,
        Cmd::Actuation(c) => actuation(c)?,
        Cmd::Effect { name, colors, speed, brightness, direction, seconds } => {
            let kind = Kind::from_name(&name).ok_or_else(|| anyhow!("unknown effect {name:?}"))?;
            let direction = match direction.as_str() {
                "right" => Direction::Right,
                "left" => Direction::Left,
                "up" => Direction::Up,
                "down" => Direction::Down,
                d => bail!("unknown direction {d:?}"),
            };
            let mut params = Params { kind, speed, brightness, direction, ..Params::default() };
            if let Some(c) = colors {
                params.palette = c.split(',').map(parse_color).collect::<Result<_>>()?;
            }
            effect(params, seconds)?;
        }
    }
    Ok(())
}

fn light(c: LightCmd) -> Result<()> {
    let mut kb = connect()?;
    let info = kb.board().lighting.clone();
    match c {
        LightCmd::Get => {
            let l = kb.lighting()?;
            let name = info.mode(l.mode).map_or("custom/unknown", |m| m.name.as_str());
            println!("mode {} ({name}), brightness {}/{}, speed {}/{}", l.mode, l.brightness, info.brightness_max, l.speed, info.speed_max);
            println!("note: the board reports the stored colour, which may differ from what a fixed-colour mode shows");
        }
        LightCmd::Modes => {
            for m in &info.modes {
                println!("{:>3}  {}{}", m.byte, m.name, if m.color { "" } else { "  (firmware colours)" });
            }
        }
        LightCmd::Set { mode, color, brightness, speed } => {
            let mut l = kb.lighting()?;
            if let Some(m) = mode {
                let found = m
                    .parse::<u8>()
                    .ok()
                    .and_then(|b| info.mode(b))
                    .or_else(|| info.modes.iter().find(|x| x.name.eq_ignore_ascii_case(&m) || x.name.to_lowercase().replace(' ', "-") == m.to_lowercase()))
                    .ok_or_else(|| anyhow!("unknown mode {m:?} (see `aether light modes`)"))?;
                l.mode = found.byte;
                l.color_mode = if found.color { 0 } else { 1 };
            }
            if let Some(c) = color {
                l.rgb = parse_color(&c)?;
                l.alpha = 0xFF;
                if info.mode(l.mode).is_some_and(|m| m.color) {
                    l.color_mode = 0;
                }
            }
            if let Some(b) = brightness {
                l.brightness = b;
            }
            if let Some(s) = speed {
                l.speed = s;
            }
            backup_first(kb.as_mut())?;
            kb.set_lighting(&l)?;
            println!("lighting set: mode {}, colour {:02x}{:02x}{:02x}, brightness {}, speed {}", l.mode, l.rgb[0], l.rgb[1], l.rgb[2], l.brightness, l.speed);
        }
    }
    Ok(())
}

fn actuation(c: ActCmd) -> Result<()> {
    let mut kb = connect()?;
    match c {
        ActCmd::Get { keys } => {
            let table = kb.actuation_table()?;
            let indices = match keys {
                Some(k) => key_indices(kb.as_ref(), &k)?,
                None => kb.layout().keys.iter().map(|k| k.index).filter(|&i| !table.record(i).is_empty()).collect(),
            };
            if indices.is_empty() {
                println!("no keys configured: every key uses the firmware default");
            }
            for i in indices {
                let name = kb.layout().key(i).map_or("?", |k| k.name.as_str());
                let r = table.record(i);
                if r.is_empty() {
                    println!("{name:<8} firmware default");
                } else if r.rapid_trigger() {
                    println!("{name:<8} {:.2} mm, rapid trigger {:.2}/{:.2} mm", raw_to_mm(r.trip), raw_to_mm(r.press), raw_to_mm(r.release));
                } else {
                    println!("{name:<8} {:.2} mm", raw_to_mm(r.trip));
                }
            }
        }
        ActCmd::Set { keys, travel, rt } => {
            let indices = key_indices(kb.as_ref(), &keys)?;
            let setting = match rt {
                None => Actuation::Fixed { travel_mm: travel },
                Some(s) => {
                    let v: Vec<f64> = s.split(',').map(|x| x.trim().parse()).collect::<Result<_, _>>().context("--rt takes PRESS or PRESS,RELEASE")?;
                    let (press_mm, release_mm) = match v[..] {
                        [p] => (p, p),
                        [p, r] => (p, r),
                        _ => bail!("--rt takes PRESS or PRESS,RELEASE"),
                    };
                    Actuation::RapidTrigger { travel_mm: travel, press_mm, release_mm }
                }
            };
            kb.validate_actuation(&indices, setting)?;
            backup_first(kb.as_mut())?;
            kb.set_actuation(&indices, setting)?;
            println!("set {} key(s) and verified by read-back", indices.len());
        }
    }
    Ok(())
}

fn effect(params: Params, seconds: Option<f32>) -> Result<()> {
    let mut kb = connect()?;
    let fps = kb.board().lighting.host_fps;
    let keys: Vec<KeyPos> = kb.layout().keys.iter().map(|k| KeyPos { index: k.index, x: k.x + k.width / 2.0, y: k.y + k.height / 2.0 }).collect();
    let saved: Lighting = kb.lighting()?;
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || r.store(false, Ordering::SeqCst))?;
    let deadline = seconds.map(|s| Instant::now() + Duration::from_secs_f32(s));

    eprintln!("running {} at {fps} fps, Ctrl+C to stop", params.kind.name());
    let mut engine = Engine::new(&keys, params, fps);
    kb.stream_begin()?;
    let result = aether_effects::run(
        &mut engine,
        fps,
        || running.load(Ordering::SeqCst) && deadline.is_none_or(|d| Instant::now() < d),
        |frame| kb.stream_frame(frame),
    );
    // Always hand the LEDs back, even if streaming failed.
    let end = kb.stream_end();
    std::thread::sleep(Duration::from_millis(200));
    let restore = kb.set_lighting(&saved);
    result?;
    end?;
    restore?;
    eprintln!("stopped; lighting restored");
    Ok(())
}
