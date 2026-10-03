//! `gbtest`: headless test-ROM runner and project scoreboard.

mod exec;
mod extra;
mod image;
mod suites;
mod trace;

use exec::{run_case, Outcome, TestResult};
use gb_core::{GameBoy, CLOCK_HZ, CYCLES_PER_FRAME, SCREEN_HEIGHT, SCREEN_WIDTH};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use suites::{is_scored, TestCase, SUITES};

const USAGE: &str = "\
usage:
  gbtest [--roms DIR] [--suite NAME]... [--filter SUBSTR] [-j N] [--long] [--wall SECS]
         [--markdown] [--json] [--screenshots DIR] [--list]
  gbtest run <rom> [--seconds N] [--frames N] [--screenshot out.png] [--wav out.wav] [--until-ldbb]
  gbtest trace <rom> [--steps N] [--doctor]

suites: blargg, mooneye, mooneye-mbc, acid2, mbc3 (scored); every other suite is not scored; see --list
(`*-cgb` suites and gbmicrotest-manual are listed but not run)
default suites: the scored ones. Exit code is 0 for any scoreboard run.

trace: one line per executed instruction (default 1000000). --doctor emits Gameboy Doctor
lines (state before each instruction) and makes LY (0xFF44) read 0x90 to the CPU, as the
Doctor reference logs assume.";

struct Opts {
    roms: PathBuf,
    suites: Vec<String>,
    filter: Option<String>,
    jobs: usize,
    long: bool,
    wall: f64,
    markdown: bool,
    json: bool,
    screenshots: Option<PathBuf>,
    list: bool,
}

fn usage_error(msg: &str) -> ! {
    eprintln!("gbtest: {msg}\n{USAGE}");
    std::process::exit(2);
}

fn value(args: &mut impl Iterator<Item = String>, flag: &str) -> String {
    args.next()
        .unwrap_or_else(|| usage_error(&format!("{flag} needs a value")))
}

fn parse<T: std::str::FromStr>(s: &str, flag: &str) -> T {
    s.parse()
        .unwrap_or_else(|_| usage_error(&format!("bad value '{s}' for {flag}")))
}

fn parse_opts(args: impl Iterator<Item = String>) -> Opts {
    let mut o = Opts {
        roms: PathBuf::from("roms"),
        suites: vec![],
        filter: None,
        jobs: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4),
        long: false,
        wall: 120.0,
        markdown: false,
        json: false,
        screenshots: None,
        list: false,
    };
    let mut args = args;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--roms" => o.roms = value(&mut args, &a).into(),
            "--suite" => {
                let s = value(&mut args, &a);
                if !SUITES.iter().any(|x| x.name == s) {
                    usage_error(&format!("unknown suite '{s}'"));
                }
                o.suites.push(s);
            }
            "--filter" => o.filter = Some(value(&mut args, &a)),
            "-j" | "--jobs" => o.jobs = parse::<usize>(&value(&mut args, &a), &a).max(1),
            "--long" => o.long = true,
            "--wall" => o.wall = parse(&value(&mut args, &a), &a),
            "--markdown" => o.markdown = true,
            "--json" => o.json = true,
            "--screenshots" => o.screenshots = Some(value(&mut args, &a).into()),
            "--list" => o.list = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            _ => usage_error(&format!("unknown argument '{a}'")),
        }
    }
    o
}

fn main() {
    let mut args = std::env::args().skip(1).peekable();
    if args.peek().map(String::as_str) == Some("run") {
        args.next();
        run_single(args);
        return;
    }
    if args.peek().map(String::as_str) == Some("trace") {
        args.next();
        trace::run(args);
        return;
    }
    let o = parse_opts(args);
    if !o.roms.is_dir() {
        usage_error(&format!(
            "ROM directory '{}' not found (use --roms DIR)",
            o.roms.display()
        ));
    }
    let mut cases = suites::discover(&o.roms, o.long);
    if o.suites.is_empty() {
        cases.retain(|c| is_scored(c.suite));
    } else {
        cases.retain(|c| o.suites.iter().any(|s| s == c.suite));
    }
    if let Some(f) = &o.filter {
        cases.retain(|c| c.name.contains(f.as_str()) || c.suite.contains(f.as_str()));
    }
    if o.list {
        list(&cases);
        return;
    }
    let skipped = cases.len();
    cases.retain(|c| SUITES.iter().any(|s| s.name == c.suite && s.runnable));
    if cases.len() != skipped {
        eprintln!(
            "note: skipped {} ROMs of suites that need a CGB model",
            skipped - cases.len()
        );
    }
    exec::install_quiet_panic_hook();
    let started = Instant::now();
    let results = run_all(&cases, &o);
    if let Some(dir) = &o.screenshots {
        save_screenshots(dir, &cases, &results);
    }
    let wall = started.elapsed();
    if o.json {
        print_json(&cases, &results);
    } else if o.markdown {
        print_markdown(&cases, &results);
    } else {
        print_table(&cases, &results, wall);
    }
}

fn list(cases: &[TestCase]) {
    for s in SUITES {
        let in_suite: Vec<_> = cases.iter().filter(|c| c.suite == s.name).collect();
        if in_suite.is_empty() {
            continue;
        }
        println!(
            "== {} ({} ROMs){}",
            s.name,
            in_suite.len(),
            if s.scored { "" } else { " [not scored]" }
        );
        for c in in_suite {
            println!("  {}  ({}s)", c.name, c.seconds);
        }
    }
    println!("TOTAL: {} ROMs", cases.len());
}

fn run_all(cases: &[TestCase], o: &Opts) -> Vec<TestResult> {
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<TestResult>>> = cases.iter().map(|_| Mutex::new(None)).collect();
    let wall = Duration::from_secs_f64(o.wall);
    std::thread::scope(|s| {
        for _ in 0..o.jobs.min(cases.len().max(1)) {
            // Each ROM runs on its own short-lived thread (fresh stack, isolated panics).
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(case) = cases.get(i) else { break };
                let res = std::thread::scope(|inner| {
                    std::thread::Builder::new()
                        .stack_size(16 << 20)
                        .spawn_scoped(inner, || run_case(case, wall))
                        .expect("spawn test thread")
                        .join()
                });
                let res = res.unwrap_or_else(|_| TestResult {
                    outcome: Outcome::Fail("runner thread died".into()),
                    elapsed: Duration::ZERO,
                    framebuffer: None,
                });
                *slots[i].lock().unwrap() = Some(res);
            });
        }
    });
    slots.into_iter().map(|m| m.into_inner().unwrap().unwrap()).collect()
}

fn safe_name(c: &TestCase) -> String {
    format!("{}__{}", c.suite, c.name.replace(['/', ' ', ','], "_"))
}

fn save_screenshots(dir: &Path, cases: &[TestCase], results: &[TestResult]) {
    for (c, r) in cases.iter().zip(results) {
        if let Some(fb) = &r.framebuffer {
            let p = dir.join(format!("{}.png", safe_name(c)));
            if let Err(e) = image::save_png(&p, fb, SCREEN_WIDTH, SCREEN_HEIGHT) {
                eprintln!("gbtest: screenshot {}: {e}", p.display());
            }
        }
    }
}

struct Tally {
    pass: usize,
    total: usize,
}

fn tally(cases: &[TestCase], results: &[TestResult], suite: &str) -> Tally {
    let mut t = Tally { pass: 0, total: 0 };
    for (c, r) in cases.iter().zip(results) {
        if c.suite == suite {
            t.total += 1;
            t.pass += (r.outcome == Outcome::Pass) as usize;
        }
    }
    t
}

fn score_line(cases: &[TestCase], results: &[TestResult]) -> String {
    let (mut p, mut t) = (0, 0);
    for s in SUITES.iter().filter(|s| s.scored) {
        let x = tally(cases, results, s.name);
        p += x.pass;
        t += x.total;
    }
    format!("SCORE: {p}/{t}")
}

fn print_table(cases: &[TestCase], results: &[TestResult], wall: Duration) {
    for s in SUITES {
        let t = tally(cases, results, s.name);
        if t.total == 0 {
            continue;
        }
        println!("== {}{} ==", s.name, if s.scored { "" } else { " (not scored)" });
        let w = cases
            .iter()
            .filter(|c| c.suite == s.name)
            .map(|c| c.name.len())
            .max()
            .unwrap_or(0);
        for (c, r) in cases.iter().zip(results).filter(|(c, _)| c.suite == s.name) {
            match &r.outcome {
                Outcome::Pass => println!("  PASS  {:w$}  {:.1}s", c.name, r.elapsed.as_secs_f64()),
                Outcome::Fail(why) => println!("  FAIL  {:w$}  {why}", c.name),
            }
        }
        println!("  -> {}/{} passed\n", t.pass, t.total);
    }
    println!("wall time: {:.1}s", wall.as_secs_f64());
    println!("{}", score_line(cases, results));
}

fn print_markdown(cases: &[TestCase], results: &[TestResult]) {
    println!("| Suite | Passed | Total |\n|---|---|---|");
    for s in SUITES {
        let t = tally(cases, results, s.name);
        if t.total > 0 {
            println!(
                "| {}{} | {} | {} |",
                s.name,
                if s.scored { "" } else { " (unscored)" },
                t.pass,
                t.total
            );
        }
    }
    println!("\n**{}**\n", score_line(cases, results));
    for s in SUITES {
        if tally(cases, results, s.name).total == 0 {
            continue;
        }
        println!("### {}\n\n| Test | Result | Notes |\n|---|---|---|", s.name);
        for (c, r) in cases.iter().zip(results).filter(|(c, _)| c.suite == s.name) {
            let (res, note) = match &r.outcome {
                Outcome::Pass => ("PASS", String::new()),
                Outcome::Fail(w) => ("FAIL", w.replace('|', "\\|")),
            };
            println!("| {} | {res} | {note} |", c.name.replace('|', "\\|"));
        }
        println!();
    }
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn print_json(cases: &[TestCase], results: &[TestResult]) {
    println!("{{\n  \"tests\": [");
    for (i, (c, r)) in cases.iter().zip(results).enumerate() {
        let (pass, reason) = match &r.outcome {
            Outcome::Pass => (true, String::new()),
            Outcome::Fail(w) => (false, w.clone()),
        };
        println!(
            "    {{\"suite\": {}, \"name\": {}, \"pass\": {pass}, \"reason\": {}, \"seconds\": {:.3}}}{}",
            json_str(c.suite),
            json_str(&c.name),
            json_str(&reason),
            r.elapsed.as_secs_f64(),
            if i + 1 < cases.len() { "," } else { "" }
        );
    }
    println!("  ],\n  \"suites\": {{");
    let shown: Vec<_> = SUITES
        .iter()
        .filter(|s| tally(cases, results, s.name).total > 0)
        .collect();
    for (i, s) in shown.iter().enumerate() {
        let t = tally(cases, results, s.name);
        println!(
            "    {}: {{\"passed\": {}, \"total\": {}, \"scored\": {}}}{}",
            json_str(s.name),
            t.pass,
            t.total,
            s.scored,
            if i + 1 < shown.len() { "," } else { "" }
        );
    }
    println!(
        "  }},\n  \"score\": {}\n}}",
        json_str(score_line(cases, results).trim_start_matches("SCORE: "))
    );
}

/// 16-bit stereo PCM WAV from interleaved f32 samples.
fn write_wav(path: &Path, samples: &[f32], rate: u32) {
    let data_len = (samples.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + samples.len() * 2);
    w.extend(b"RIFF");
    w.extend((36 + data_len).to_le_bytes());
    w.extend(b"WAVEfmt ");
    w.extend(16u32.to_le_bytes());
    w.extend(1u16.to_le_bytes());
    w.extend(2u16.to_le_bytes());
    w.extend(rate.to_le_bytes());
    w.extend((rate * 4).to_le_bytes());
    w.extend(4u16.to_le_bytes());
    w.extend(16u16.to_le_bytes());
    w.extend(b"data");
    w.extend(data_len.to_le_bytes());
    for s in samples {
        w.extend(((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    if let Err(e) = std::fs::write(path, w) {
        eprintln!("gbtest: {}: {e}", path.display());
        std::process::exit(1);
    }
    println!("wav: {} ({} samples/channel)", path.display(), samples.len() / 2);
}

fn run_single(mut args: impl Iterator<Item = String>) {
    let mut rom: Option<String> = None;
    let (mut seconds, mut frames, mut shot, mut until) = (10.0f64, None::<u64>, None::<PathBuf>, false);
    let mut wav: Option<PathBuf> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--seconds" => seconds = parse(&value(&mut args, &a), &a),
            "--frames" => frames = Some(parse(&value(&mut args, &a), &a)),
            "--screenshot" => shot = Some(value(&mut args, &a).into()),
            "--wav" => wav = Some(value(&mut args, &a).into()),
            "--until-ldbb" => until = true,
            _ if !a.starts_with("--") && rom.is_none() => rom = Some(a),
            _ => usage_error(&format!("unknown argument '{a}'")),
        }
    }
    let rom = rom.unwrap_or_else(|| usage_error("run: missing <rom>"));
    let data = std::fs::read(&rom).unwrap_or_else(|e| usage_error(&format!("{rom}: {e}")));
    let mut gb = GameBoy::new(data).unwrap_or_else(|e| usage_error(&format!("{rom}: cartridge error {e:?}")));
    let limit = match frames {
        Some(f) => f * CYCLES_PER_FRAME as u64,
        None => (seconds * CLOCK_HZ as f64) as u64,
    };
    const WAV_RATE: u32 = 48_000;
    if wav.is_some() {
        gb.set_sample_rate(WAV_RATE);
    }
    let mut audio: Vec<f32> = Vec::new();
    let mut why = "time limit";
    while gb.cycles() < limit {
        if gb.step() == Some(0x40) && until {
            why = "LD B,B";
            break;
        }
        if wav.is_some() {
            gb.drain_audio(&mut audio);
        }
    }
    if let Some(p) = &wav {
        gb.drain_audio(&mut audio);
        write_wav(p, &audio, WAV_RATE);
    }
    let serial = String::from_utf8_lossy(gb.serial_output()).into_owned();
    println!(
        "stopped: {why} after {} cycles ({:.2}s emulated)",
        gb.cycles(),
        gb.cycles() as f64 / CLOCK_HZ as f64
    );
    println!("serial output ({} bytes):\n{serial}", serial.len());
    let r = gb.registers();
    println!(
        "A={:02X} F={:02X} B={:02X} C={:02X} D={:02X} E={:02X} H={:02X} L={:02X} SP={:04X} PC={:04X}",
        r.a, r.f, r.b, r.c, r.d, r.e, r.h, r.l, r.sp, r.pc
    );
    if let Some(p) = shot {
        match image::save_png(&p, gb.framebuffer(), SCREEN_WIDTH, SCREEN_HEIGHT) {
            Ok(()) => println!("screenshot: {}", p.display()),
            Err(e) => {
                eprintln!("gbtest: {e}");
                std::process::exit(1);
            }
        }
    }
}
