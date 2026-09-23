use rustyline::config::BellStyle;
use rustyline::{ColorMode, Config, Editor, Prompt, history::DefaultHistory};
use std::io;
use std::sync::{Mutex, OnceLock};

fn editor() -> &'static Mutex<Editor<(), DefaultHistory>> {
    static EDITOR: OnceLock<Mutex<Editor<(), DefaultHistory>>> = OnceLock::new();
    EDITOR.get_or_init(|| {
        let color_mode = if crate::output::color_disabled() {
            ColorMode::Disabled
        } else {
            ColorMode::Enabled
        };
        let config = Config::builder()
            .color_mode(color_mode)
            .bell_style(BellStyle::None)
            .build();
        let mut ed = Editor::<(), DefaultHistory>::with_config(config)
            .expect("failed to create line editor");
        // rustyline only renders the styled prompt when a Highlighter is
        // installed; `()` is the identity highlighter.
        ed.set_helper(Some(()));
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

pub fn read_user_input<P: Prompt + ?Sized>(prompt: &P) -> Option<String> {
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

/// Like [`read_user_input`], but pre-populates the buffer with `initial` so the
/// user can edit it (cursor placed at the end). Used by the permission rule
/// builder.
pub fn read_user_input_with_initial<P: Prompt + ?Sized>(
    prompt: &P,
    initial: &str,
) -> Option<String> {
    let mut ed = editor().lock().unwrap();
    match ed.readline_with_initial(prompt, (initial, "")) {
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
