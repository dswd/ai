use crate::format::MarkdownFormatter;
use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

/// Pending output state: 0 = None, 1 = Stdout, 2 = Stderr
static PENDING: AtomicU8 = AtomicU8::new(0);

static FORMATTER: OnceLock<Mutex<MarkdownFormatter>> = OnceLock::new();

fn formatter() -> &'static Mutex<MarkdownFormatter> {
    FORMATTER.get_or_init(|| Mutex::new(MarkdownFormatter::new(tty_enabled())))
}

fn tty_enabled() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    io::stdout().is_terminal()
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
    let _ = write!(io::stderr(), "{s}");
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
    let _ = writeln!(io::stderr(), "{s}");
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
