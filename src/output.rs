use crate::format::MarkdownFormatter;
use regex::Regex;
use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// Pending output state: 0 = None, 1 = Stdout, 2 = Stderr
static PENDING: AtomicU8 = AtomicU8::new(0);

/// Set by `--no-color` before any output is produced.
static NO_COLOR: AtomicBool = AtomicBool::new(false);

static FORMATTER: OnceLock<Mutex<MarkdownFormatter>> = OnceLock::new();

fn formatter() -> &'static Mutex<MarkdownFormatter> {
    FORMATTER.get_or_init(|| Mutex::new(MarkdownFormatter::new(tty_enabled())))
}

/// Disable colored output (and ANSI styling in tool logs) for the rest of the
/// process. Also implied by the `NO_COLOR` environment variable.
pub fn set_no_color(enabled: bool) {
    NO_COLOR.store(enabled, Ordering::SeqCst);
}

fn color_disabled() -> bool {
    NO_COLOR.load(Ordering::SeqCst) || std::env::var_os("NO_COLOR").is_some()
}

fn tty_enabled() -> bool {
    if color_disabled() {
        return false;
    }
    io::stdout().is_terminal()
}

/// Remove ANSI SGR escape sequences so tool logs stay readable when colors are
/// disabled.
fn strip_ansi(s: &str) -> String {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    match RE.get_or_init(|| Regex::new("\x1b\\[[0-9;]*m").ok()) {
        Some(re) => re.replace_all(s, "").into_owned(),
        None => s.to_string(),
    }
}

struct StdoutWriter;

impl Write for StdoutWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let old = PENDING.swap(1, Ordering::SeqCst);
        if old == 2 {
            let _ = writeln!(io::stderr());
        }
        let _ = io::stdout().write_all(buf);
        let _ = io::stdout().flush();
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        io::stdout().flush()
    }
}

/// Emit a text token to stdout. Finalizes pending stderr line first if needed.
pub fn stdout_push(s: &str) {
    if let Ok(mut f) = formatter().lock() {
        f.push(s, &mut StdoutWriter);
    }
}

/// Emit a thinking token to stderr (no trailing newline).
/// Continues on same stderr line if previous was also stderr token.
pub fn stderr_push(s: &str) {
    flush_stdout_partial();
    let old = PENDING.swap(2, Ordering::SeqCst);
    if old == 1 {
        let _ = writeln!(io::stderr());
    }
    if color_disabled() {
        let _ = write!(io::stderr(), "{}", strip_ansi(s));
    } else {
        let _ = write!(io::stderr(), "{s}");
    }
    let _ = io::stderr().flush();
}

/// Emit a complete line to stderr (trailing newline).
/// Finalizes any pending stdout/stderr line first.
pub fn stderr_line(s: &str) {
    flush_stdout_partial();
    let old = PENDING.swap(0, Ordering::SeqCst);
    if old != 0 {
        let _ = writeln!(io::stderr());
    }
    if color_disabled() {
        let _ = writeln!(io::stderr(), "{}", strip_ansi(s));
    } else {
        let _ = writeln!(io::stderr(), "{s}");
    }
}

/// A TTY-only progress spinner on stderr. It stops and clears itself on drop,
/// so wrap the wait immediately around it and drop before emitting output.
pub struct Spinner {
    shutdown: Option<Arc<AtomicBool>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Spinner {
    /// Start a spinner. Returns an inert guard when stderr is not a TTY or the
    /// process is quiet.
    pub fn start(label: &str) -> Spinner {
        if !io::stderr().is_terminal() || crate::logging::is_quiet() {
            return Spinner {
                shutdown: None,
                handle: None,
            };
        }
        let running = Arc::new(AtomicBool::new(true));
        let alive = Arc::clone(&running);
        let label = label.to_string();
        let handle = std::thread::spawn(move || {
            const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
            let mut out = io::stderr();
            let mut i = 0usize;
            let mut width = 0usize;
            while alive.load(Ordering::Relaxed) {
                let line = format!("{} {}", FRAMES[i % FRAMES.len()], label);
                let _ = write!(out, "\r{line}");
                let _ = out.flush();
                width = line.chars().count();
                i += 1;
                std::thread::sleep(Duration::from_millis(80));
            }
            if width > 0 {
                let _ = write!(out, "\r{}\r", " ".repeat(width));
                let _ = out.flush();
            }
        });
        Spinner {
            shutdown: Some(running),
            handle: Some(handle),
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        if let Some(flag) = self.shutdown.take() {
            flag.store(false, Ordering::Relaxed);
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Finalize stdout with a newline. Used before returning the final response.
pub fn stdout_finish() {
    if let Ok(mut f) = formatter().lock() {
        f.finish(&mut StdoutWriter);
    }
    PENDING.store(0, Ordering::SeqCst);
}

/// Emit any buffered partial stdout line before switching to stderr,
/// so reasoning output stays in order relative to the streamed text.
fn flush_stdout_partial() {
    if let Some(f) = FORMATTER.get()
        && let Ok(mut f) = f.lock()
    {
        f.flush_partial(&mut StdoutWriter);
    }
}
