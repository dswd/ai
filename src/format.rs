use ansi_color_constants::*;
use regex::Regex;
use std::io::Write;
use std::sync::OnceLock;

static INLINE_RE: OnceLock<Regex> = OnceLock::new();

fn inline_re() -> &'static Regex {
    INLINE_RE.get_or_init(|| {
        Regex::new(
            r"(?P<triple>\*\*\*.+?\*\*\*)|(?P<bold>\*\*.+?\*\*)|(?P<ital>\*.+?\*)|(?P<code>`[^`]*`)|(?P<strike>~~.+?~~)",
        )
        .expect("valid inline regex")
    })
}

pub struct MarkdownFormatter {
    buf: String,
    in_code_block: bool,
    tty: bool,
    ends_with_newline: bool,
}

impl MarkdownFormatter {
    pub fn new(tty: bool) -> Self {
        Self {
            buf: String::new(),
            in_code_block: false,
            tty,
            ends_with_newline: false,
        }
    }

    pub fn push<W: Write>(&mut self, chunk: &str, writer: &mut W) {
        self.buf.push_str(chunk);
        while let Some(pos) = self.buf.find('\n') {
            let line = self.buf[..pos].to_string();
            self.buf.drain(..=pos);
            let rendered = self.render_line(&line);
            let _ = writer.write_all(rendered.as_bytes());
            let _ = writer.write_all(b"\n");
            self.ends_with_newline = true;
        }
        if !self.buf.is_empty() {
            self.ends_with_newline = false;
        }
    }

    pub fn flush_partial<W: Write>(&mut self, writer: &mut W) {
        if self.buf.is_empty() {
            return;
        }
        let line = std::mem::take(&mut self.buf);
        let rendered = self.render_line(&line);
        let _ = writer.write_all(rendered.as_bytes());
        self.ends_with_newline = false;
    }

    pub fn finish<W: Write>(&mut self, writer: &mut W) {
        self.flush_partial(writer);
        if !self.ends_with_newline {
            let _ = writer.write_all(b"\n");
        }
        self.buf.clear();
        self.in_code_block = false;
        self.ends_with_newline = false;
    }

    fn render_line(&mut self, line: &str) -> String {
        if !self.tty {
            return line.to_string();
        }

        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            let rendered = format!("{DIM}{line}{RESET}");
            self.in_code_block = !self.in_code_block;
            return rendered;
        }

        if self.in_code_block {
            return format!("{DIM}{line}{RESET}");
        }

        if let Some(stripped) = header_text(line) {
            return format!("{BOLD}{CYAN}{stripped}{RESET}");
        }

        render_inline(line)
    }
}

fn header_text(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('#')?;
    let mut hashes = 1;
    let mut rest = rest;
    while rest.starts_with('#') && hashes < 6 {
        hashes += 1;
        rest = &rest[1..];
    }
    if rest.starts_with(' ') {
        Some(rest.strip_prefix(' ').unwrap_or(rest))
    } else {
        None
    }
}

fn render_inline(line: &str) -> String {
    let mut out = String::with_capacity(line.len() + 32);
    let mut last = 0;
    for caps in inline_re().captures_iter(line) {
        let m = caps.get(0).unwrap();
        out.push_str(&line[last..m.start()]);
        let full = m.as_str();
        if caps.name("code").is_some() {
            let inner = &full[1..full.len() - 1];
            out.push_str(&format!("{L_BLUE}{inner}{RESET}"));
        } else if caps.name("strike").is_some() {
            let inner = &full[2..full.len() - 2];
            out.push_str(&format!("{STRIKETHRU}{inner}{RESET}"));
        } else if caps.name("triple").is_some() {
            let inner = &full[3..full.len() - 3];
            out.push_str(&format!("{BOLD}{ITALICS}{inner}{RESET}"));
        } else if caps.name("bold").is_some() {
            let inner = &full[2..full.len() - 2];
            out.push_str(&format!("{BOLD}{inner}{RESET}"));
        } else {
            let inner = &full[1..full.len() - 1];
            out.push_str(&format!("{ITALICS}{inner}{RESET}"));
        }
        last = m.end();
    }
    out.push_str(&line[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(tty: bool, input: &[&str]) -> String {
        let mut f = MarkdownFormatter::new(tty);
        let mut out = Vec::new();
        for chunk in input {
            f.push(chunk, &mut out);
        }
        f.finish(&mut out);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn test_bold() {
        let out = fmt(true, &["**bold**\n"]);
        assert_eq!(out, "\u{1b}[1mbold\u{1b}[0m\n");
    }

    #[test]
    fn test_italic() {
        let out = fmt(true, &["*italic*\n"]);
        assert_eq!(out, "\u{1b}[3mitalic\u{1b}[0m\n");
    }

    #[test]
    fn test_bold_italic() {
        let out = fmt(true, &["***both***\n"]);
        assert_eq!(out, "\u{1b}[1m\u{1b}[3mboth\u{1b}[0m\n");
    }

    #[test]
    fn test_inline_code() {
        let out = fmt(true, &["`code`\n"]);
        assert_eq!(out, "\u{1b}[36mcode\u{1b}[0m\n");
    }

    #[test]
    fn test_strike() {
        let out = fmt(true, &["~~gone~~\n"]);
        assert_eq!(out, "\u{1b}[9mgone\u{1b}[0m\n");
    }

    #[test]
    fn test_markers_split_across_chunks() {
        let out = fmt(true, &["**bo", "ld**\n"]);
        assert_eq!(out, "\u{1b}[1mbold\u{1b}[0m\n");
    }

    #[test]
    fn test_unmatched_markers_literal() {
        let out = fmt(true, &["**unclosed\n"]);
        assert_eq!(out, "**unclosed\n");
    }

    #[test]
    fn test_code_block() {
        let out = fmt(true, &["```rs\n", "fn main() {}\n", "```\n"]);
        assert_eq!(
            out,
            "\u{1b}[2m```rs\u{1b}[0m\n\u{1b}[2mfn main() {}\u{1b}[0m\n\u{1b}[2m```\u{1b}[0m\n"
        );
    }

    #[test]
    fn test_header() {
        let out = fmt(true, &["# Title\n", "## Sub\n"]);
        assert_eq!(
            out,
            "\u{1b}[1m\u{1b}[96mTitle\u{1b}[0m\n\u{1b}[1m\u{1b}[96mSub\u{1b}[0m\n"
        );
    }

    #[test]
    fn test_non_tty_passthrough() {
        let out = fmt(false, &["**bold** `code`\n", "```rs\n", "x\n", "```\n"]);
        assert_eq!(out, "**bold** `code`\n```rs\nx\n```\n");
    }
}
