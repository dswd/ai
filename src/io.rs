use rustyline::{Editor, history::DefaultHistory};
use std::io;
use std::sync::{Mutex, OnceLock};

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

pub async fn read_stdin_async() -> Option<String> {
    let stdin = io::stdin();
    if std::io::IsTerminal::is_terminal(&stdin) {
        return None;
    }
    use tokio::io::AsyncReadExt;
    let mut buffer = Vec::new();
    match tokio::io::stdin().read_to_end(&mut buffer).await {
        Ok(_) if !buffer.is_empty() => {
            let text = String::from_utf8_lossy(&buffer).trim().to_string();
            if text.is_empty() { None } else { Some(text) }
        }
        _ => None,
    }
}

pub fn stderr_line(text: &str) {
    crate::output::stderr_line(text);
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
        | Err(rustyline::error::ReadlineError::Eof) => Some("/exit".to_string()),
        Err(_) => None,
    }
}
