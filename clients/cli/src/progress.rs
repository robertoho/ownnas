//! Telling what is going on as it happens, so a long run never looks stuck. All of it goes to
//! stderr, leaving stdout to the results scripts read. On a terminal, what is under way is one
//! line, rewritten in place, with what happened to each file printed above it; otherwise each
//! goes on a line of its own, with running counts now and then. `--quiet` leaves only results
//! and failures.
use std::fmt::Display;
use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use crate::human_size;

/// Files at least this large get their own percentage on the progress line.
pub const LARGE: u64 = 4 * 1024 * 1024;
/// How often the progress line is redrawn on a terminal.
const TICK: Duration = Duration::from_millis(250);
/// Without a terminal, how often running counts are written out.
const PLAIN_EVERY: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Screen {
    /// What is under way, shown on the last line of a terminal.
    status: String,
    drawn: bool,
    last_plain: Option<Instant>,
}

struct Reporter {
    quiet: bool,
    tty: bool,
    screen: Mutex<Screen>,
}

static REPORTER: OnceLock<Reporter> = OnceLock::new();

/// Called once at the start. Until then (as in tests), only results and failures are told.
pub fn init(quiet: bool) {
    let _ = REPORTER.set(Reporter { quiet, tty: !quiet && io::stderr().is_terminal(), screen: Mutex::default() });
}

fn reporter() -> &'static Reporter {
    REPORTER.get_or_init(|| Reporter { quiet: true, tty: false, screen: Mutex::default() })
}

fn screen() -> MutexGuard<'static, Screen> {
    reporter().screen.lock().unwrap_or_else(|poison| poison.into_inner())
}

#[cfg(unix)]
fn width() -> usize {
    // SAFETY: TIOCGWINSZ only fills in the struct passed, which lives for the call.
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    let got = unsafe { libc::ioctl(libc::STDERR_FILENO, libc::TIOCGWINSZ, &mut size) };
    if got == 0 && size.ws_col > 0 {
        usize::from(size.ws_col)
    } else {
        80
    }
}

#[cfg(not(unix))]
fn width() -> usize {
    80
}

/// Short enough for one line of the terminal, so rewriting it in place works.
fn fit(text: &str, width: usize) -> String {
    let room = width.saturating_sub(1).max(10);
    if text.chars().count() <= room {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(room - 1).collect();
    cut.push('…');
    cut
}

fn wipe(screen: &mut Screen, err: &mut impl Write) {
    if screen.drawn {
        let _ = write!(err, "\r\x1b[K");
        screen.drawn = false;
    }
}

fn draw(screen: &mut Screen, err: &mut impl Write) {
    if !screen.status.is_empty() {
        let _ = write!(err, "\r\x1b[K{}", fit(&screen.status, width()));
        screen.drawn = true;
    }
    let _ = err.flush();
}

/// Prints a line above the progress line, which is then drawn again.
fn above(line: &dyn Display, to_stdout: bool) {
    let tty = reporter().tty;
    let mut screen = screen();
    let mut err = io::stderr().lock();
    if tty {
        wipe(&mut screen, &mut err);
    }
    if to_stdout {
        let _ = err.flush();
        println!("{line}");
    } else {
        let _ = writeln!(err, "{line}");
    }
    if tty {
        draw(&mut screen, &mut err);
    }
}

/// A result, on stdout, as before: told even with --quiet.
pub fn out(line: impl Display) {
    above(&line, true);
}

/// A failure, on stderr: told even with --quiet.
pub fn error(line: impl Display) {
    above(&line, false);
}

/// What happened to one file, or what a step found.
pub fn event(line: impl Display) {
    if !reporter().quiet {
        above(&line, false);
    }
}

/// A step starting: shown as the progress line on a terminal, and as a line of its own otherwise.
pub fn phase(text: impl Into<String>) {
    let reporter = reporter();
    if reporter.quiet {
        return;
    }
    let text = text.into();
    let mut screen = screen();
    let mut err = io::stderr().lock();
    if reporter.tty {
        screen.status = text;
        draw(&mut screen, &mut err);
    } else {
        let _ = writeln!(err, "{text}");
        screen.last_plain = Some(Instant::now());
    }
}

/// How the step under way is getting on: the progress line on a terminal, and otherwise a line
/// every few seconds.
pub fn status(text: impl Into<String>) {
    let reporter = reporter();
    if reporter.quiet {
        return;
    }
    let text = text.into();
    let mut screen = screen();
    let mut err = io::stderr().lock();
    if reporter.tty {
        if text != screen.status || !screen.drawn {
            screen.status = text;
            draw(&mut screen, &mut err);
        }
    } else if screen.last_plain.is_none_or(|last| last.elapsed() >= PLAIN_EVERY) {
        let _ = writeln!(err, "{text}");
        screen.last_plain = Some(Instant::now());
    }
}

/// The step is over: the progress line goes.
pub fn clear() {
    if reporter().quiet {
        return;
    }
    let mut screen = screen();
    let mut err = io::stderr().lock();
    wipe(&mut screen, &mut err);
    screen.status.clear();
    let _ = err.flush();
}

/// Runs `work`, with `tick` called every so often meanwhile to tell how it is getting on.
fn ticking<T>(tick: &(dyn Fn(Duration) + Sync), work: impl FnOnce() -> T) -> T {
    if reporter().quiet {
        return work();
    }
    let done = AtomicBool::new(false);
    let started = Instant::now();
    let result = std::thread::scope(|scope| {
        scope.spawn(|| {
            let mut last = Instant::now();
            while !done.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(50));
                if last.elapsed() >= TICK && !done.load(Ordering::Relaxed) {
                    tick(started.elapsed());
                    last = Instant::now();
                }
            }
        });
        let result = work();
        done.store(true, Ordering::Relaxed);
        result
    });
    clear();
    result
}

/// Something that takes as long as it takes, such as a question to the server: told as it
/// starts, with the seconds counted while it lasts.
pub fn waiting<T>(text: &str, work: impl FnOnce() -> T) -> T {
    phase(format!("{text}…"));
    ticking(
        &|took| {
            if took >= Duration::from_secs(1) {
                status(format!("{text}… {} s", took.as_secs()));
            }
        },
        work,
    )
}

/// One file being worked on.
pub struct Slot {
    name: String,
    size: u64,
    /// Bytes done of what it is doing now.
    pub moved: AtomicU64,
    doing: Mutex<&'static str>,
}

impl Slot {
    /// On to something else, such as sending after hashing.
    pub fn doing(&self, what: &'static str) {
        *self.doing.lock().unwrap_or_else(|poison| poison.into_inner()) = what;
        self.moved.store(0, Ordering::Relaxed);
    }
}

/// How a batch of files is getting on, for the progress line.
pub struct Meter {
    noun: &'static str,
    files: usize,
    /// For copying: the bytes in all, to tell how many have gone.
    bytes: Option<u64>,
    finished: AtomicU64,
    done_bytes: AtomicU64,
    active: Mutex<Vec<Arc<Slot>>>,
    started: Instant,
}

impl Meter {
    pub fn new(noun: &'static str, files: usize, bytes: Option<u64>) -> Self {
        Meter { noun, files, bytes, finished: AtomicU64::new(0), done_bytes: AtomicU64::new(0), active: Mutex::default(), started: Instant::now() }
    }

    fn active(&self) -> MutexGuard<'_, Vec<Arc<Slot>>> {
        self.active.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    pub fn begin(&self, name: &str, size: u64, doing: &'static str) -> Arc<Slot> {
        let slot = Arc::new(Slot { name: name.to_string(), size, moved: AtomicU64::new(0), doing: Mutex::new(doing) });
        self.active().push(slot.clone());
        slot
    }

    /// That file is done with, whichever way it went; `sent` adds its size to what has gone.
    pub fn end(&self, slot: &Arc<Slot>, sent: bool) {
        self.active().retain(|other| !Arc::ptr_eq(other, slot));
        self.finished.fetch_add(1, Ordering::Relaxed);
        if sent {
            self.done_bytes.fetch_add(slot.size, Ordering::Relaxed);
        }
    }

    pub fn line(&self) -> String {
        let active = self.active();
        let mut line = format!("{}/{} {}", self.finished.load(Ordering::Relaxed), self.files, self.noun);
        if let Some(total) = self.bytes {
            let moved = self.done_bytes.load(Ordering::Relaxed) + active.iter().map(|slot| slot.moved.load(Ordering::Relaxed)).sum::<u64>();
            let rate = (moved as f64 / self.started.elapsed().as_secs_f64().max(0.001)) as u64;
            line.push_str(&format!(", {} of {}, {}/s", human_size(moved), human_size(total), human_size(rate)));
        }
        let mut large: Vec<&Arc<Slot>> = active.iter().filter(|slot| slot.size >= LARGE).collect();
        large.sort_by_key(|slot| std::cmp::Reverse(slot.size));
        let shown: Vec<&Arc<Slot>> = if large.is_empty() { active.iter().take(1).collect() } else { large.into_iter().take(2).collect() };
        for slot in shown {
            let doing = *slot.doing.lock().unwrap_or_else(|poison| poison.into_inner());
            line.push_str(&format!(" · {doing} {}", slot.name));
            if slot.size >= LARGE {
                let moved = slot.moved.load(Ordering::Relaxed).min(slot.size);
                line.push_str(&format!(" {}% of {}", moved * 100 / slot.size.max(1), human_size(slot.size)));
            }
        }
        if active.len() > 1 {
            line.push_str(&format!(" (+{} more at once)", active.len() - 1));
        }
        line
    }

    /// Runs `work`, keeping the progress line up to date meanwhile.
    pub fn run<T>(&self, work: impl FnOnce() -> T) -> T {
        ticking(&|_| status(self.line()), work)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_line_is_cut_to_fit() {
        assert_eq!(fit("short", 80), "short");
        let cut = fit(&"x".repeat(200), 40);
        assert_eq!(cut.chars().count(), 39);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn the_progress_line_tells_large_files_by_percent() {
        let meter = Meter::new("files", 3, Some(20 * 1024 * 1024));
        let small = meter.begin("small.txt", 10, "sending");
        meter.end(&small, true);
        let big = meter.begin("big.mov", 10 * 1024 * 1024, "sending");
        big.moved.store(5 * 1024 * 1024, Ordering::Relaxed);
        let line = meter.line();
        assert!(line.starts_with("1/3 files, 5.0 MB of 20.0 MB"), "{line}");
        assert!(line.contains("sending big.mov 50% of 10.0 MB"), "{line}");
        big.doing("checking");
        assert!(meter.line().contains("checking big.mov 0%"), "{}", meter.line());
    }
}
