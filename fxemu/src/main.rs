//! fxemu: CASIO fx-9860GII-2 emulator running a real firmware dump.
//!
//! Double-click (no arguments): opens the calculator in its own window,
//! resumes where it was last closed, and saves everything on close.
//! Debugging: `fxemu --headless 10 [--trace-io] [--press 3.0:EXE] ...`

// No console window for the release build on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod bus;
mod cpu;
#[cfg(windows)]
mod gui;
mod flash;
mod keys;
mod lcd;
mod state;
mod timers;
mod web;

use std::collections::{HashMap, VecDeque};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bus::Bus;
use cpu::Cpu;
use flash::Flash;
use state::Snap;
use timers::CPU_HZ;

/// The firmware dumped from the calculator, built into the executable.
static BUILTIN_ROM: &[u8] = include_bytes!("../../dump/fx9860gii2_full_4MB.bin");

/// Fixed port: also makes sure only one emulator runs at a time.
const APP_PORT: u16 = 47860;

pub struct Shared {
    pub lcd: Mutex<Vec<u8>>,
    /// Key presses/releases from the window, applied one at a time.
    pub key_events: Mutex<VecDeque<(u8, bool)>>,
    pub status: Mutex<String>,
    pub reset: AtomicBool,
    pub turbo: AtomicBool,
    pub quit: AtomicBool,
    /// The calculator is switched off (blank screen, CPU asleep).
    pub off: AtomicBool,
    /// Milliseconds since start of the last request from the window.
    pub last_seen_ms: AtomicU64,
    /// When the window said goodbye (0 = never).
    pub bye_ms: AtomicU64,
    pub started: Instant,
}

impl Shared {
    pub fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }
}

struct Args {
    rom: Option<String>,
    flash: Option<String>,
    data_dir: Option<String>,
    port: u16,
    no_window: bool,
    web: bool,
    headless: Option<f64>,
    trace_io: bool,
    trace_exc: bool,
    trace_irq: bool,
    trace_flash: bool,
    fast: bool,
    presses: Vec<(f64, u8)>,
    screen_every: Option<f64>,
    breakpoints: Vec<u32>,
    profile: bool,
    trace_all_after: Option<f64>,
}

fn parse_args() -> Args {
    let mut a = Args {
        rom: None,
        flash: None,
        data_dir: None,
        port: APP_PORT,
        no_window: false,
        web: false,
        headless: None,
        trace_io: false,
        trace_exc: false,
        trace_irq: false,
        trace_flash: false,
        fast: false,
        presses: Vec::new(),
        screen_every: None,
        breakpoints: Vec::new(),
        profile: false,
        trace_all_after: None,
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    let val = |i: &mut usize| -> String {
        *i += 1;
        argv.get(*i).cloned().unwrap_or_else(|| {
            eprintln!("missing value for {}", argv[*i - 1]);
            std::process::exit(2)
        })
    };
    while i < argv.len() {
        match argv[i].as_str() {
            "--rom" => a.rom = Some(val(&mut i)),
            "--flash" => a.flash = Some(val(&mut i)),
            "--data-dir" => a.data_dir = Some(val(&mut i)),
            "--port" => a.port = val(&mut i).parse().expect("port"),
            "--no-window" => a.no_window = true,
            "--web" => a.web = true,
            "--headless" => a.headless = Some(val(&mut i).parse().expect("seconds")),
            "--trace-io" => a.trace_io = true,
            "--trace-exc" => a.trace_exc = true,
            "--trace-irq" => a.trace_irq = true,
            "--trace-flash" => a.trace_flash = true,
            "--fast" => a.fast = true,
            "--profile" => a.profile = true,
            "--trace-all-after" => a.trace_all_after = Some(val(&mut i).parse().expect("seconds")),
            "--screen-every" => a.screen_every = Some(val(&mut i).parse().expect("seconds")),
            "--break" => {
                let v = val(&mut i);
                a.breakpoints.push(u32::from_str_radix(v.trim_start_matches("0x"), 16).expect("hex address"));
            }
            "--press" => {
                // --press 3.5:EXE  (key held for 150 ms)
                let v = val(&mut i);
                let (t, k) = v.split_once(':').expect("--press TIME:KEY");
                let code = keys::by_name(k).unwrap_or_else(|| {
                    eprintln!("unknown key {}", k);
                    std::process::exit(2)
                });
                a.presses.push((t.parse().expect("time"), code));
            }
            "-h" | "--help" => {
                eprintln!("{}", include_str!("../README.md"));
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown argument {}", other);
                std::process::exit(2);
            }
        }
        i += 1;
    }
    a
}

fn ascii_screen(frame: &[u8]) -> String {
    let px = |x: usize, y: usize| (frame[y * 16 + x / 8] >> (7 - x % 8)) & 1 != 0;
    let border = format!("+{}+", "-".repeat(128));
    let mut s = border.clone() + "\n";
    for y in (0..64).step_by(2) {
        s.push('|');
        for x in 0..128 {
            s.push(match (px(x, y), px(x, y + 1)) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                _ => ' ',
            });
        }
        s += "|\n";
    }
    s + &border
}

fn default_data_dir() -> PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA") {
        return Path::new(&appdata).join("fxemu");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    Path::new(&home).join(".fxemu")
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Fingerprint of the boot code + OS, so a snapshot is only resumed on the
/// same firmware it was taken with.
fn rom_id(rom: &[u8]) -> u32 {
    let mut h: u32 = 0x811C_9DC5;
    for &b in &rom[..rom.len().min(0x25_0000)] {
        h = (h ^ b as u32).wrapping_mul(0x0100_0193);
    }
    h
}

fn save_snapshot(cpu: &Cpu, rom_id: u32, path: &Path) -> std::io::Result<()> {
    let mut w = state::W::default();
    w.0.extend_from_slice(state::MAGIC);
    w.u32(rom_id);
    w.u64(unix_now());
    cpu.save(&mut w);
    // Write to a temporary file first so a crash never leaves a broken snapshot.
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &w.0)?;
    std::fs::rename(&tmp, path)
}

fn load_snapshot(cpu: &mut Cpu, rom_id: u32, path: &Path) -> bool {
    let Ok(data) = std::fs::read(path) else { return false };
    if data.len() < 20 || &data[..8] != state::MAGIC {
        return false;
    }
    let mut r = state::R::new(&data[8..]);
    let (Ok(id), Ok(saved_at)) = (r.u32(), r.u64()) else { return false };
    if id != rom_id {
        return false;
    }
    if cpu.load(&mut r).is_err() {
        return false;
    }
    // The real clock keeps running while the calculator is off.
    cpu.bus.rtc.add_seconds(unix_now().saturating_sub(saved_at));
    true
}

/// Open the calculator in its own app-style window (Edge app mode on Windows).
fn open_window(url: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut edges: Vec<PathBuf> = Vec::new();
        for var in ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"] {
            if let Ok(base) = std::env::var(var) {
                edges.push(Path::new(&base).join(r"Microsoft\Edge\Application\msedge.exe"));
            }
        }
        for e in edges {
            if e.exists() {
                let ok = std::process::Command::new(&e)
                    .arg(format!("--app={}", url))
                    .arg("--window-size=430,860")
                    .spawn()
                    .is_ok();
                if ok {
                    return;
                }
            }
        }
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    #[cfg(not(windows))]
    {
        if std::process::Command::new("xdg-open").arg(url).spawn().is_err() {
            eprintln!("open {} in a browser", url);
        }
    }
}

fn main() {
    let args = parse_args();
    let app_mode = args.headless.is_none();

    let rom: Vec<u8> = match &args.rom {
        Some(p) => std::fs::read(p).unwrap_or_else(|e| {
            eprintln!("cannot read ROM {}: {}", p, e);
            std::process::exit(1)
        }),
        None => BUILTIN_ROM.to_vec(),
    };
    let rom_id = rom_id(&rom);

    // Where the emulated flash (your files) and the resume snapshot live.
    let (flash_path, snap_path): (Option<PathBuf>, Option<PathBuf>) = if app_mode {
        let dir = args.data_dir.as_ref().map(PathBuf::from).unwrap_or_else(default_data_dir);
        let _ = std::fs::create_dir_all(&dir);
        let flash = args.flash.as_ref().map(PathBuf::from).unwrap_or_else(|| dir.join("flash.bin"));
        (Some(flash), Some(dir.join("state.bin")))
    } else {
        (args.flash.as_ref().map(PathBuf::from), None)
    };

    // Native window on Windows; the browser page elsewhere or with --web.
    let use_gui = app_mode && cfg!(windows) && !args.web && !args.no_window;

    // Only one emulator at a time (the port doubles as a lock): if it is
    // taken, just bring the running one to the front.
    let listener = if app_mode {
        match TcpListener::bind(("127.0.0.1", args.port)) {
            Ok(l) => Some(l),
            Err(_) => {
                #[cfg(windows)]
                if use_gui && gui::focus_existing() {
                    return;
                }
                if !args.no_window {
                    open_window(&format!("http://127.0.0.1:{}/", args.port));
                }
                return;
            }
        }
    } else {
        None
    };

    let image = match &flash_path {
        Some(p) if p.exists() => std::fs::read(p).unwrap_or_else(|_| rom.clone()),
        _ => rom.clone(),
    };
    let mut flash = Flash::new(image);
    flash.log = args.trace_flash;
    let mut bus = Bus::new(flash);
    bus.trace_io = args.trace_io;
    bus.trace_exc = args.trace_exc;
    bus.trace_irq = args.trace_irq;
    bus.lcd.log = args.trace_io;
    let mut cpu = Cpu::new(bus);
    if let Some(sp) = &snap_path {
        if load_snapshot(&mut cpu, rom_id, sp) {
            eprintln!("[main] resumed from {}", sp.display());
            // Start with all keys up, whatever was held when it was saved.
            if cpu.bus.keys.iter().any(|&b| b != 0) {
                cpu.bus.set_keys([0; 12]);
            }
        }
    }

    let shared = Arc::new(Shared {
        lcd: Mutex::new(cpu.bus.lcd.frame()),
        key_events: Mutex::new(VecDeque::new()),
        status: Mutex::new(String::new()),
        reset: AtomicBool::new(false),
        turbo: AtomicBool::new(false),
        quit: AtomicBool::new(false),
        off: AtomicBool::new(false),
        last_seen_ms: AtomicU64::new(0),
        bye_ms: AtomicU64::new(0),
        started: Instant::now(),
    });
    // Keep the lock while running; serve the page unless the native window is used.
    let (_lock, listener) = if use_gui { (listener, None) } else { (None, listener) };
    if !use_gui {
        if let Some(l) = listener {
            let s = shared.clone();
            std::thread::spawn(move || web::serve(s, l));
            if !args.no_window && app_mode {
                open_window(&format!("http://127.0.0.1:{}/", args.port));
            }
        }
    }

    let emu_shared = shared.clone();
    let emulate = move || run_emulator(cpu, emu_shared, args, flash_path, snap_path, rom_id, app_mode);
    if use_gui {
        let worker = std::thread::spawn(emulate);
        #[cfg(windows)]
        gui::run(shared.clone());
        shared.quit.store(true, Ordering::SeqCst);
        let _ = worker.join();
    } else {
        emulate();
    }
}

fn run_emulator(
    mut cpu: Cpu,
    shared: Arc<Shared>,
    args: Args,
    flash_path: Option<PathBuf>,
    snap_path: Option<PathBuf>,
    rom_id: u32,
    app_mode: bool,
) {

    // Real-time pacing is measured from this point (and re-based on turbo changes).
    let mut pace_wall = Instant::now();
    let mut pace_cycles = cpu.cycles;
    let mut was_turbo = false;
    let start_cycles = cpu.cycles;
    let mut last_flash_save = Instant::now();
    let mut last_snapshot = Instant::now();
    let mut last_status = Instant::now();
    let mut last_status_cycles = cpu.cycles;
    let mut last_instr = cpu.instructions;
    let mut next_screen = args.screen_every.unwrap_or(f64::INFINITY);
    let mut profile: HashMap<u32, u64> = HashMap::new();
    let mut bp_hits: HashMap<u32, u32> = HashMap::new();
    let slice = CPU_HZ / 250; // 4 ms of emulated time
    // Keyboard state as seen by the calculator, and when it last changed.
    let mut key_matrix = [0u8; 12];
    let mut key_changed_at = cpu.cycles;
    let key_gap = CPU_HZ / 20; // at least 50 ms between key changes
    let key_timeout = CPU_HZ / 2; // don't wait more than 0.5 s for the OS

    loop {
        let target = cpu.cycles + slice;
        if args.breakpoints.is_empty() && !args.profile {
            while cpu.cycles < target {
                cpu.cycles += cpu.step() as u64;
                if cpu.cycles >= cpu.bus.next_tick {
                    cpu.bus.tick(cpu.cycles);
                }
            }
        } else {
            while cpu.cycles < target {
                if args.breakpoints.contains(&cpu.pc) {
                    let h = bp_hits.entry(cpu.pc).or_insert(0);
                    *h += 1;
                    if *h <= 5 {
                        eprintln!("[break] hit #{} {}", h, cpu.regs_string());
                    }
                }
                if args.profile && cpu.instructions % 997 == 0 {
                    *profile.entry(cpu.pc).or_insert(0) += 1;
                }
                cpu.cycles += cpu.step() as u64;
                if cpu.cycles >= cpu.bus.next_tick {
                    cpu.bus.tick(cpu.cycles);
                }
            }
        }
        // Emulated seconds since this run started (headless scripts use this).
        let emu_secs = (cpu.cycles - start_cycles) as f64 / CPU_HZ as f64;
        if let Some(t) = args.trace_all_after {
            if emu_secs >= t && cpu.bus.trace_all == 0 && emu_secs < t + 0.01 {
                cpu.bus.trace_all = 400;
            }
        }

        // Keyboard: from the window, or from scripted presses.
        if !app_mode {
            let mut m = [0u8; 12];
            for &(t, code) in &args.presses {
                if emu_secs >= t && emu_secs < t + 0.15 {
                    keys::set_matrix(&mut m, code, true);
                }
            }
            cpu.bus.set_keys(m);
        } else {
            // Apply key events one at a time, each only after the OS has read
            // the previous change, so fast typing never merges two keys.
            let since = cpu.cycles - key_changed_at;
            if since >= key_gap && (cpu.bus.keys_seen || since >= key_timeout) {
                if let Some((code, down)) = shared.key_events.lock().unwrap().pop_front() {
                    keys::set_matrix(&mut key_matrix, code, down);
                    cpu.bus.set_keys(key_matrix);
                    key_changed_at = cpu.cycles;
                }
            }
            *shared.lcd.lock().unwrap() = cpu.bus.lcd.frame();
        }

        if shared.reset.swap(false, Ordering::SeqCst) {
            eprintln!("[main] reset requested");
            cpu.reset();
        }

        if last_status.elapsed() >= Duration::from_secs(1) {
            let wall = last_status.elapsed().as_secs_f64();
            let speed = (cpu.cycles - last_status_cycles) as f64 / CPU_HZ as f64 / wall * 100.0;
            let mips = (cpu.instructions - last_instr) as f64 / wall / 1e6;
            last_instr = cpu.instructions;
            last_status_cycles = cpu.cycles;
            last_status = Instant::now();
            if app_mode {
                let blank = cpu.sleeping && cpu.bus.lcd.frame().iter().all(|&b| b == 0);
                shared.off.store(blank, Ordering::Relaxed);
                *shared.status.lock().unwrap() = if blank {
                    "The calculator is switched off – press AC/ON (or Home) to switch it on".to_string()
                } else {
                    format!("speed {:.0}%", speed)
                };
            } else {
                eprintln!("[status] {:.1}s emulated · {:.1} MIPS · pc {:08x}{}", emu_secs, mips, cpu.pc,
                    if cpu.sleeping { " · idle" } else { "" });
            }
        }

        if emu_secs >= next_screen {
            eprintln!("[screen @ {:.1}s]\n{}", emu_secs, ascii_screen(&cpu.bus.lcd.frame()));
            next_screen += args.screen_every.unwrap();
        }

        // Persist files (flash) soon after they change, and the whole machine
        // state every minute, so nothing is lost even if the PC crashes.
        if let Some(p) = &flash_path {
            if cpu.bus.flash.dirty && last_flash_save.elapsed() >= Duration::from_secs(3) {
                if std::fs::write(p, &cpu.bus.flash.data).is_ok() {
                    cpu.bus.flash.dirty = false;
                }
                last_flash_save = Instant::now();
            }
        }
        if let Some(sp) = &snap_path {
            if last_snapshot.elapsed() >= Duration::from_secs(60) {
                let _ = save_snapshot(&cpu, rom_id, sp);
                last_snapshot = Instant::now();
            }
        }

        // Quit when the window is closed: it says goodbye, then nothing more
        // arrives for 3 seconds (a reload starts polling again immediately).
        // Also quit if the window has been gone for a long time.
        if app_mode {
            let now = shared.now_ms();
            let bye = shared.bye_ms.load(Ordering::SeqCst);
            let seen = shared.last_seen_ms.load(Ordering::SeqCst);
            let closed = bye > 0 && now > bye.max(seen) + 3000;
            let gone = seen > 0 && now > seen + 5 * 60 * 1000;
            if closed || gone || shared.quit.load(Ordering::SeqCst) {
                break;
            }
        }

        if let Some(limit) = args.headless {
            if emu_secs >= limit {
                break;
            }
        }

        let turbo = args.fast || shared.turbo.load(Ordering::Relaxed);
        if turbo != was_turbo {
            pace_wall = Instant::now();
            pace_cycles = cpu.cycles;
            was_turbo = turbo;
        }
        if !turbo {
            let emu = (cpu.cycles - pace_cycles) as f64 / CPU_HZ as f64;
            let ahead = emu - pace_wall.elapsed().as_secs_f64();
            if ahead > 0.0 {
                std::thread::sleep(Duration::from_secs_f64(ahead.min(0.05)));
            } else if ahead < -0.5 {
                // Too far behind (e.g. the PC was asleep): don't try to catch up.
                pace_wall = Instant::now();
                pace_cycles = cpu.cycles;
            }
        }
    }

    // Save everything on exit.
    if let Some(p) = &flash_path {
        if cpu.bus.flash.dirty {
            let _ = std::fs::write(p, &cpu.bus.flash.data);
        }
    }
    if let Some(sp) = &snap_path {
        let _ = save_snapshot(&cpu, rom_id, sp);
    }
    if app_mode {
        return;
    }

    // Headless report.
    eprintln!(
        "[end] {:.2}s emulated, {} instructions; {}",
        (cpu.cycles - start_cycles) as f64 / CPU_HZ as f64,
        cpu.instructions,
        cpu.regs_string()
    );
    eprintln!(
        "[end] lcd writes: {}, flash writes: {}, sleeping: {}, irq line: {:?}, IPR: {:04x?}, IMR: {:02x?}",
        cpu.bus.lcd.writes, cpu.bus.flash.writes, cpu.sleeping, cpu.bus.irq_line, cpu.bus.intc.ipr, cpu.bus.intc.imr
    );
    cpu.dump_history();
    if args.profile {
        let mut v: Vec<_> = profile.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        eprintln!("[profile] hottest PCs:");
        for (pc, n) in v.iter().take(25) {
            eprintln!("  {:08x}  {}", pc, n);
        }
    }
    eprintln!("[end] unmodelled register accesses ({}):", cpu.bus.unknown_log.len());
    for l in cpu.bus.unknown_log.iter().take(300) {
        eprintln!("  {}", l);
    }
    eprintln!("{}", ascii_screen(&cpu.bus.lcd.frame()));
}
