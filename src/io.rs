use std::io::{self, BufRead, Write};

pub fn read_stdin() -> Option<String> {
    let stdin = io::stdin();
    if std::io::IsTerminal::is_terminal(&stdin) {
        return None;
    }
    let mut lines = Vec::new();
    let reader = io::BufReader::new(stdin.lock());
    for line in reader.lines() {
        if let Ok(line) = line {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

pub fn print_stdout(text: &str) {
    let mut stdout = io::stdout().lock();
    let _ = stdout.write_all(text.as_bytes());
    let _ = stdout.flush();
}

pub fn print_stderr(text: &str) {
    let mut stderr = io::stderr().lock();
    let _ = stderr.write_all(text.as_bytes());
    let _ = stderr.flush();
}

pub fn stdout_line(text: &str) {
    print_stdout(text);
    print_stdout("\n");
}

pub fn stderr_line(text: &str) {
    print_stderr(text);
    print_stderr("\n");
}

pub fn read_user_input(prompt: &str) -> Option<String> {
    let mut stdout = io::stdout().lock();
    let _ = stdout.write_all(prompt.as_bytes());
    let _ = stdout.flush();

    let stdin = io::stdin();
    let mut line = String::new();
    match stdin.read_line(&mut line) {
        Ok(0) => None,
        Ok(_) => {
            let trimmed = line.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        }
        Err(_) => None,
    }
}
