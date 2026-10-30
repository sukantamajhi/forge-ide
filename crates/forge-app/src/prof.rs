//! A profiling aid. With `FORGE_PROFILE=1` the editor prints, every five seconds, how many frames it built and where
//! the time went (`prof::time("name", || ...)` adds up the calls under each name). With the variable unset a timed
//! call costs one load of a flag.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("FORGE_PROFILE").is_some())
}

#[derive(Default)]
struct Totals {
    since: Option<Instant>,
    frames: u64,
    frame_time: Duration,
    spans: BTreeMap<&'static str, (u64, Duration)>,
}

static TOTALS: Mutex<Totals> = Mutex::new(Totals { since: None, frames: 0, frame_time: Duration::ZERO, spans: BTreeMap::new() });

/// With profiling on, print how long after the first call to `mark` this point was reached (startup phases).
pub fn mark(what: &str) {
    static START: OnceLock<Instant> = OnceLock::new();
    let start = *START.get_or_init(Instant::now);
    if enabled() {
        eprintln!("[prof] startup: {what} at {:.0} ms", start.elapsed().as_secs_f64() * 1000.0);
    }
}

/// Run `f`, adding its time to the total for `name`.
pub fn time<T>(name: &'static str, f: impl FnOnce() -> T) -> T {
    if !enabled() {
        return f();
    }
    let start = Instant::now();
    let out = f();
    let took = start.elapsed();
    let mut totals = TOTALS.lock().unwrap_or_else(|e| e.into_inner());
    let entry = totals.spans.entry(name).or_default();
    entry.0 += 1;
    entry.1 += took;
    out
}

/// One frame's element tree was built in `took`. Prints the report when five seconds have passed.
pub fn frame(took: Duration) {
    if !enabled() {
        return;
    }
    static FIRST: OnceLock<()> = OnceLock::new();
    if FIRST.set(()).is_ok() {
        mark("first frame built");
    }
    let mut totals = TOTALS.lock().unwrap_or_else(|e| e.into_inner());
    totals.frames += 1;
    totals.frame_time += took;
    let start = *totals.since.get_or_insert_with(Instant::now);
    if start.elapsed() < Duration::from_secs(5) {
        return;
    }
    let secs = start.elapsed().as_secs_f64();
    let frames = totals.frames.max(1);
    let mut line = format!("[prof] {:.1} frames/s, {:.2} ms to build each", frames as f64 / secs, totals.frame_time.as_secs_f64() * 1000.0 / frames as f64);
    for (name, (calls, total)) in &totals.spans {
        line.push_str(&format!(" | {name} x{calls} {:.2} ms each", total.as_secs_f64() * 1000.0 / *calls as f64));
    }
    eprintln!("{line}");
    *totals = Totals { since: Some(Instant::now()), ..Totals::default() };
}
