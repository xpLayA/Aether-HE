//! `aether`: control an Aether-supported keyboard from the terminal.
//! Every command that changes the board saves a backup first.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use aether_device::{Advanced, BUILTINS, describe, Backup, KeyPresses, Keyboard, PRESETS, plan_advanced, usage_for_code, Profile, Registry, backups_dir, open_first, profiles_dir};
use aether_effects::{Direction, Engine, KeyPos, Kind, Params};
use aether_proto::{Actuation, DksSlot, Lighting, SocdMode, raw_to_mm};
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
    Backups {
        /// Keep only each board's original backup (missing sections merged in
        /// from the oldest backup that has them) and delete the rest.
        #[arg(long)]
        prune: bool,
    },
    /// Firmware lighting.
    #[command(subcommand)]
    Light(LightCmd),
    /// Actuation point and rapid trigger.
    #[command(subcommand)]
    Actuation(ActCmd),
    /// Saved setups (lighting + actuation, optionally an effect for the GUI).
    #[command(subcommand)]
    Profile(ProfileCmd),
    /// Advanced keys: DKS, mod-tap, toggle, SOCD, rapid snap (RS).
    #[command(subcommand)]
    Keys(KeysCmd),
    /// Run a host-driven effect until Ctrl+C (or --seconds), then restore the lighting.
    Effect {
        /// wave, breathe, aurora, comet, tide, twinkle, rain, striation, static;
        /// reactive: ripple, splash, cross, sparks, afterglow, heatmap, lightning
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
        /// Comma-separated key names, or `all`.
        #[arg(long)]
        keys: String,
        /// Actuation point in mm.
        #[arg(long)]
        travel: f64,
        /// Rapid trigger sensitivity: PRESS or PRESS,RELEASE in mm.
        #[arg(long)]
        rt: Option<String>,
    },
    /// Put keys back on the firmware default actuation (as shipped).
    Reset {
        /// Comma-separated key names, or `all`.
        #[arg(long, default_value = "all")]
        keys: String,
    },
}

#[derive(Subcommand)]
enum KeysCmd {
    /// Show the keys that have an advanced function.
    List,
    /// Back to an ordinary key (a pair's other key too).
    Off { key: String },
    /// Toggle: press once to hold the key down, again to release.
    Tgl { key: String },
    /// Mod-tap: tap sends TAP, holding longer than --ms sends HOLD.
    Mt {
        key: String,
        #[arg(long)]
        tap: String,
        #[arg(long)]
        hold: String,
        /// 10..1000 ms, in steps of 10
        #[arg(long, default_value_t = 200)]
        ms: u16,
    },
    /// Rapid snap: of the two keys, the one pressed deeper wins.
    Rs { key1: String, key2: String },
    /// SOCD: what holding both keys sends.
    Socd {
        key1: String,
        key2: String,
        /// last, key1, key2 or neutral
        #[arg(long, default_value = "last")]
        mode: String,
    },
    /// Dynamic keystroke: up to four keys sent at press / bottom-out / release points.
    Dks {
        key: String,
        /// press point in mm
        #[arg(long, default_value_t = 1.0)]
        press: f64,
        /// bottom-out point in mm
        #[arg(long, default_value_t = 3.0)]
        bottom: f64,
        /// KEY:pbrr — one letter per point (press, bottom-out, release from bottom, full release):
        /// t = tap, h = hold, - = nothing. E.g. --act W:t--t --act Space:-t--. Up to four.
        #[arg(long = "act", required = true)]
        acts: Vec<String>,
    },
}

#[derive(Subcommand)]
enum ProfileCmd {
    /// List the built-in and custom profiles.
    List,
    /// Save the board's current lighting and actuation as a new custom profile.
    Save { name: String },
    /// Overwrite a custom profile with the board's current lighting and actuation (keeps its effect).
    Update { name: String },
    /// Apply a built-in or custom profile (backs up first, verifies actuation).
    Apply { name: String },
    /// Copy a profile (built-ins too) to a new custom profile.
    Duplicate { name: String, new_name: String },
    /// Rename a custom profile.
    Rename { name: String, new_name: String },
    /// Delete a custom profile.
    Delete { name: String },
}

fn main() {
    // Exit quietly when output is cut off (`aether backups | head`) instead of
    // panicking on a broken pipe, as command-line tools are expected to.
    // SAFETY: restores the default disposition before any other thread exists.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
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

/// Key names separated by commas, or `all` for every key on the board.
fn key_indices(kb: &dyn Keyboard, list: &str) -> Result<Vec<usize>> {
    if list.trim().eq_ignore_ascii_case("all") {
        return Ok(kb.layout().keys.iter().map(|k| k.index).collect());
    }
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
            let mut kb = match aether_device::connect() {
                Err(aether_device::Error::BluetoothOnly(bt)) => {
                    println!("{} ({})", bt.board.name, bt.name);
                    println!("connection: Bluetooth (typing only; settings need the USB-C cable or the 2.4 GHz receiver)");
                    if let Some(b) = bt.battery {
                        println!("battery: {b}%");
                    }
                    return Ok(());
                }
                r => r.context("could not connect to the keyboard")?.0,
            };
            let i = kb.device_info()?;
            println!("{} ({:04X}:{:04X}), firmware V{:X}.{:02X}", kb.board().name, i.vid, i.pid, i.firmware >> 8, i.firmware & 0xFF);
            if kb.board().wireless {
                println!("connection: 2.4 GHz receiver, battery {}%{}", i.battery, if i.charging() { " (charging)" } else { "" });
            }
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
        Cmd::Backups { prune: true } => {
            for board in Registry::load()?.boards {
                if let Some((keep, n)) = Backup::prune(&board.slug)? {
                    println!("{}: kept {}, deleted {n}", board.slug, keep.display());
                }
            }
        }
        Cmd::Backups { prune: false } => {
            for (t, p) in Backup::list()? {
                let board = p.file_stem().and_then(|s| s.to_str()).and_then(|s| s.rsplit_once('-')).map_or("?", |(b, _)| b);
                println!("{t}\t{board}\t{}", p.display());
            }
        }
        Cmd::Light(c) => light(c)?,
        Cmd::Actuation(c) => actuation(c)?,
        Cmd::Profile(c) => profile(c)?,
        Cmd::Keys(c) => keys(c)?,
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
            write_actuation(kb.as_mut(), &indices, setting)?;
            println!("set {} key(s) and verified by read-back", indices.len());
        }
        ActCmd::Reset { keys } => {
            let indices = key_indices(kb.as_ref(), &keys)?;
            write_actuation(kb.as_mut(), &indices, Actuation::Default)?;
            println!("{} key(s) back on the firmware default, verified by read-back", indices.len());
        }
    }
    Ok(())
}

fn profile(c: ProfileCmd) -> Result<()> {
    let dir = profiles_dir()?;
    Profile::migrate_in(&dir);
    match c {
        ProfileCmd::List => {
            for (name, preset) in BUILTINS {
                println!("{name:<20} built-in: {} on every key, keeps lighting", PRESETS[preset].1);
            }
            for p in Profile::list_in(&dir, "") {
                let t = p.actuation_table()?;
                let set = (0..aether_proto::KEY_SLOTS).filter(|&i| !t.record(i).is_empty()).count();
                let fx = p.effect.as_ref().map_or(String::new(), |e| format!(", effect {}", e.kind));
                let light = p.lighting.map_or("keeps lighting".into(), |l| format!("lighting mode {}", l.mode));
                println!("{:<20} {light}, {set} keys configured{fx}", p.name);
            }
        }
        ProfileCmd::Save { name } => {
            let mut kb = connect()?;
            let path = Profile::capture(kb.as_mut(), &name)?.save_new_in(&dir)?;
            println!("saved {}", path.display());
        }
        ProfileCmd::Update { name } => {
            let old = Profile::find_in(&dir, &name).ok_or_else(|| anyhow!("no custom profile named {name:?}"))?;
            let mut kb = connect()?;
            let p = Profile { effect: old.effect, ..Profile::capture(kb.as_mut(), &old.name)? };
            println!("updated {}", p.save_in(&dir)?.display());
        }
        ProfileCmd::Apply { name } => {
            let mut kb = connect()?;
            let p = find_any(&dir, kb.as_ref(), &name)?;
            let backup = p.apply(kb.as_mut())?;
            eprintln!("backup: {}", backup.display());
            println!("applied {:?} (actuation verified)", p.name);
            if let Some(e) = &p.effect {
                println!("note: its {} effect runs only in the GUI", e.kind);
            }
        }
        ProfileCmd::Duplicate { name, new_name } => {
            let p = match Profile::find_in(&dir, &name) {
                Some(p) => p,
                None => find_any(&dir, connect()?.as_ref(), &name)?,
            };
            let copy = Profile { name: new_name.trim().into(), builtin: false, ..p };
            println!("saved {}", copy.save_new_in(&dir)?.display());
        }
        ProfileCmd::Rename { name, new_name } => {
            println!("renamed to {}", Profile::rename_in(&dir, &name, &new_name)?.display());
        }
        ProfileCmd::Delete { name } => {
            Profile::delete_in(&dir, &name)?;
            println!("deleted {name:?}");
        }
    }
    Ok(())
}

fn usage_of(kb: &dyn Keyboard, name: &str) -> Result<u8> {
    let k = kb.layout().find(name).ok_or_else(|| anyhow!("no key named {name:?}"))?;
    usage_for_code(&k.code).ok_or_else(|| anyhow!("{} can't be sent by another key", k.name))
}

fn one_key(kb: &dyn Keyboard, name: &str) -> Result<usize> {
    kb.layout().find(name).map(|k| k.index).ok_or_else(|| anyhow!("no key named {name:?}"))
}

fn keys(c: KeysCmd) -> Result<()> {
    let mut kb = connect()?;
    if !kb.board().capabilities.advanced_keys {
        bail!("advanced keys are not verified on {}", kb.board().name);
    }
    let (table, dks) = (kb.key_table()?, kb.dks_table()?);
    let adv = match c {
        KeysCmd::List => {
            let mut any = false;
            for k in &kb.layout().keys {
                let a = table.action(k.index);
                if a.is_advanced() {
                    any = true;
                    println!("{:<10} {}", k.name, describe(kb.layout(), a, &dks));
                }
            }
            if !any {
                println!("no advanced keys set");
            }
            return Ok(());
        }
        KeysCmd::Off { ref key } => (one_key(kb.as_ref(), key)?, Advanced::Off),
        KeysCmd::Tgl { ref key } => (one_key(kb.as_ref(), key)?, Advanced::Toggle),
        KeysCmd::Mt { ref key, ref tap, ref hold, ms } => (
            one_key(kb.as_ref(), key)?,
            Advanced::ModTap { tap: usage_of(kb.as_ref(), tap)?, hold: usage_of(kb.as_ref(), hold)?, delay_ms: ms },
        ),
        KeysCmd::Rs { ref key1, ref key2 } => (one_key(kb.as_ref(), key1)?, Advanced::Rs { partner: one_key(kb.as_ref(), key2)? }),
        KeysCmd::Socd { ref key1, ref key2, ref mode } => {
            let mode = match mode.to_lowercase().as_str() {
                "last" => SocdMode::LastInput,
                "key1" => SocdMode::Key1,
                "key2" => SocdMode::Key2,
                "neutral" => SocdMode::Neutral,
                m => bail!("unknown SOCD mode {m:?} (last, key1, key2, neutral)"),
            };
            (one_key(kb.as_ref(), key1)?, Advanced::Socd { partner: one_key(kb.as_ref(), key2)?, mode })
        }
        KeysCmd::Dks { ref key, press, bottom, ref acts } => {
            if acts.len() > 4 {
                bail!("at most four --act");
            }
            let (mut actions, mut masks) = ([0u8; 4], [0u8; 4]);
            for (i, a) in acts.iter().enumerate() {
                let (k, pts) = a.rsplit_once(':').ok_or_else(|| anyhow!("--act {a:?}: expected KEY:pbrr"))?;
                if pts.chars().count() != 4 {
                    bail!("--act {a:?}: give one of t/h/- for each of the four points");
                }
                actions[i] = usage_of(kb.as_ref(), k)?;
                for (p, ch) in pts.chars().enumerate() {
                    masks[p] |= match ch {
                        't' => 1 << i,
                        'h' => 0x10 << i,
                        '-' => 0,
                        c => bail!("--act {a:?}: {c:?} is not t, h or -"),
                    };
                }
            }
            (one_key(kb.as_ref(), key)?, Advanced::Dks(DksSlot::new(press, bottom, actions, masks)))
        }
    };
    let (idx, adv) = adv;
    let slug = kb.board().slug.clone();
    let original = Backup::original_key_table(&slug)?.unwrap_or_else(|| table.clone());
    let layout = kb.layout().clone();
    let (new_keys, new_dks) = plan_advanced(&table, &dks, &layout, &original, idx, adv)?;
    backup_first(kb.as_mut())?;
    if new_dks != dks {
        kb.write_dks_table(&new_dks)?;
    }
    kb.write_key_table(&new_keys)?;
    let name = layout.key(idx).map_or(String::new(), |k| k.name.clone());
    let what = match new_keys.action(idx) {
        a if a.is_advanced() => describe(kb.layout(), a, &new_dks),
        _ => "an ordinary key".into(),
    };
    println!("{name} is now {what} (verified by read-back)");
    Ok(())
}

/// A built-in (needs the board's layout) or custom profile by name.
fn find_any(dir: &std::path::Path, kb: &dyn Keyboard, name: &str) -> Result<Profile> {
    Profile::builtins(&kb.board().slug, kb.layout())
        .into_iter()
        .find(|p| p.name.eq_ignore_ascii_case(name.trim()))
        .or_else(|| Profile::find_in(dir, name))
        .ok_or_else(|| anyhow!("no profile named {name:?}"))
}

/// Validate, back up, write and verify.
fn write_actuation(kb: &mut dyn Keyboard, indices: &[usize], setting: Actuation) -> Result<()> {
    kb.validate_actuation(indices, setting)?;
    backup_first(kb)?;
    kb.set_actuation(indices, setting)?;
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
    let listener = if params.kind.reactive() {
        let l = KeyPresses::open(kb.board(), kb.layout()).context("reactive effects need the keyboard's key presses")?;
        eprintln!("reacting to key presses on the keyboard");
        Some(l)
    } else {
        None
    };
    let mut engine = Engine::new(&keys, params, fps);
    kb.stream_begin()?;
    let result = aether_effects::run(
        &mut engine,
        fps,
        || running.load(Ordering::SeqCst) && deadline.is_none_or(|d| Instant::now() < d),
        || listener.as_ref().map(|l| l.drain().collect()).unwrap_or_default(),
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
