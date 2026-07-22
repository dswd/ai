use std::io::{self, BufRead, Write};
use std::sync::{Mutex, OnceLock};
use rustyline::{Editor, history::DefaultHistory};

fn editor() -> &'static Mutex<Editor<(), DefaultHistory>> {
    static EDITOR: OnceLock<Mutex<Editor<(), DefaultHistory>>> = OnceLock::new();
    EDITOR.get_or_init(|| {
        let ed = Editor::<(), DefaultHistory>::new().expect("failed to create line editor");
        Mutex::new(ed)
    })
}

pub fn load_session_history(lines: &[String]) {
    let mut ed = editor().lock().unwrap();
    for line in lines {
        let _ = ed.add_history_entry(line);
    }
}

pub fn read_stdin() -> Option<String> {
    let stdin = io::stdin();
    if std::io::IsTerminal::is_terminal(&stdin) {
        return None;
    }
    let mut lines = Vec::new();
    let reader = io::BufReader::new(stdin.lock());
    for line in reader.lines().map_while(Result::ok) {
        lines.push(line);
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

pub fn print_stderr(text: &str) {
    let mut stderr = io::stderr().lock();
    let _ = stderr.write_all(text.as_bytes());
    let _ = stderr.flush();
}

pub fn stderr_line(text: &str) {
    print_stderr(text);
    print_stderr("\n");
}

pub fn read_user_input(prompt: &str) -> Option<String> {
    let mut ed = editor().lock().unwrap();
    match ed.readline(prompt) {
        Ok(line) => {
            let trimmed = line.trim().to_string();
            if trimmed.is_empty() {
                return None;
            }
            let _ = ed.add_history_entry(&trimmed);
            Some(trimmed)
        }
        Err(rustyline::error::ReadlineError::Interrupted)
        | Err(rustyline::error::ReadlineError::Eof) => {
            Some("/exit".to_string())
        }
        Err(_) => None,
    }
}
