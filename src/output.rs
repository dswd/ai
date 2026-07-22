use std::io::{self, Write};
use std::sync::atomic::{AtomicU8, Ordering};

/// Pending output state: 0 = None, 1 = Stdout, 2 = Stderr
static PENDING: AtomicU8 = AtomicU8::new(0);

/// Emit a text token to stdout. Finalizes pending stderr line first if needed.
pub fn stdout_push(s: &str) {
    let old = PENDING.swap(1, Ordering::SeqCst);
    if old == 2 {
        let _ = writeln!(io::stderr());
    }
    let _ = write!(io::stdout(), "{s}");
    let _ = io::stdout().flush();
}

/// Emit a thinking token to stderr (no trailing newline).
/// Continues on same stderr line if previous was also stderr token.
pub fn stderr_push(s: &str) {
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
    let old = PENDING.swap(0, Ordering::SeqCst);
    if old != 0 {
        let _ = writeln!(io::stderr());
    }
    let _ = writeln!(io::stderr(), "{s}");
}

/// Finalize stdout with a newline. Used before returning the final response.
pub fn stdout_finish() {
    if PENDING.swap(0, Ordering::SeqCst) == 2 {
        let _ = writeln!(io::stderr());
    }
    println!();
}
