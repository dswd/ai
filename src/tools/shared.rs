use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use log::warn;
use regex::Regex;

use rand::RngExt;

/// Realistic browser user-agent used for all web requests.
pub(crate) const DEFAULT_UA: &str =
    "Mozilla/5.0 (X11; Linux x86_64; rv:132.0) Gecko/20100101 Firefox/132.0";

/// A small pool of realistic user-agents, rotated per request to avoid
/// fingerprinting via a single fixed UA.
pub(crate) const USER_AGENTS: &[&str] = &[
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:132.0) Gecko/20100101 Firefox/132.0",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Safari/605.1.15",
];

/// Pick a random user-agent from the pool.
pub(crate) fn random_ua() -> &'static str {
    let idx = rand::rng().random_range(0..USER_AGENTS.len());
    USER_AGENTS[idx]
}

/// Shared HTTP client with connection pooling, built once per proxy
/// configuration and reused across tools. A cookie jar is enabled so
/// consent/verification cookies carry across requests within a process run.
///
/// When `proxy` is `None`, the client honors the standard environment
/// variables (`HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY`) via
/// reqwest's default system-proxy handling. When `Some(url)`, requests are
/// routed through that proxy (HTTP, HTTPS, or SOCKS5 such as
/// `socks5h://127.0.0.1:1080`).
pub(crate) fn http_client(proxy: Option<&str>) -> reqwest::Client {
    static CLIENTS: OnceLock<Mutex<HashMap<Option<String>, reqwest::Client>>> = OnceLock::new();
    let cache = CLIENTS.get_or_init(|| Mutex::new(HashMap::new()));
    let key = proxy.map(str::to_string);
    if let Some(client) = cache.lock().unwrap().get(&key) {
        return client.clone();
    }

    let mut builder = reqwest::Client::builder()
        .user_agent(DEFAULT_UA)
        .cookie_store(true)
        .timeout(Duration::from_secs(30));
    if let Some(url) = proxy {
        match reqwest::Proxy::all(url) {
            Ok(proxy) => {
                builder = builder.proxy(proxy);
            }
            Err(e) => {
                warn!("invalid proxy URL {url:?}: {e}; continuing without explicit proxy");
            }
        }
    }
    let client = builder.build().expect("failed to build HTTP client");
    cache.lock().unwrap().insert(key, client.clone());
    client
}

/// Browser-like request headers to set per request on top of the shared client
/// defaults. UA is chosen per request so consecutive requests differ.
pub(crate) fn browser_headers(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    request
        .header(reqwest::header::USER_AGENT, random_ua())
        .header(
            reqwest::header::ACCEPT,
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
        )
        .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9")
        .header(reqwest::header::REFERER, "https://www.google.com/")
        .header("Sec-Fetch-Dest", "document")
        .header("Sec-Fetch-Mode", "navigate")
        .header("Sec-Fetch-Site", "cross-site")
        .header("Upgrade-Insecure-Requests", "1")
        .header("DNT", "1")
}

/// Emit a JS string literal (JSON strings are valid JS string literals; `serde_json`
/// escapes `\`, `"`, newlines, and unicode line separators). Safe to splice into
/// `page.evaluate(...)` templates.
#[cfg(feature = "browser")]
pub(crate) fn js_literal(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("{0}")]
    Message(String),
}

pub fn commands_in_string(command: &str) -> Vec<String> {
    let re = Regex::new(r"&&|\|\||[|;&]").unwrap();
    re.split(command)
        .filter_map(|seg| {
            let word = seg.split_whitespace().next()?;
            if word.is_empty() {
                None
            } else {
                Some(word.to_string())
            }
        })
        .collect()
}

pub(crate) static BASHKIT_BUILTINS: &[&str] = &[
    "echo",
    "printf",
    "cat",
    "nl",
    "read",
    "mapfile",
    "readarray",
    "log",
    "cd",
    "pwd",
    "ls",
    "find",
    "tree",
    "pushd",
    "popd",
    "dirs",
    "true",
    "false",
    "exit",
    "return",
    "break",
    "continue",
    "test",
    "[",
    "assert",
    "export",
    "set",
    "unset",
    "local",
    "shift",
    "source",
    ".",
    "eval",
    "readonly",
    "times",
    "declare",
    "typeset",
    "let",
    "dotenv",
    "envsubst",
    "bash",
    "sh",
    "exec",
    ":",
    "trap",
    "caller",
    "getopts",
    "shopt",
    "command",
    "type",
    "which",
    "hash",
    "alias",
    "unalias",
    "compgen",
    "fc",
    "help",
    "grep",
    "rg",
    "sed",
    "awk",
    "head",
    "tail",
    "sort",
    "uniq",
    "cut",
    "tr",
    "wc",
    "paste",
    "column",
    "diff",
    "comm",
    "strings",
    "tac",
    "rev",
    "seq",
    "expr",
    "fold",
    "expand",
    "unexpand",
    "join",
    "split",
    "iconv",
    "shuf",
    "template",
    "mkdir",
    "mktemp",
    "mkfifo",
    "rm",
    "cp",
    "mv",
    "touch",
    "chmod",
    "chown",
    "ln",
    "rmdir",
    "realpath",
    "readlink",
    "truncate",
    "glob",
    "patch",
    "file",
    "stat",
    "less",
    "tar",
    "gzip",
    "gunzip",
    "zip",
    "unzip",
    "od",
    "xxd",
    "hexdump",
    "base64",
    "md5sum",
    "sha1sum",
    "sha256sum",
    "verify",
    "sleep",
    "date",
    "basename",
    "dirname",
    "timeout",
    "wait",
    "watch",
    "yes",
    "kill",
    "clear",
    "numfmt",
    "retry",
    "parallel",
    "df",
    "du",
    "xargs",
    "tee",
    "whoami",
    "hostname",
    "uname",
    "id",
    "env",
    "printenv",
    "history",
    "json",
    "csv",
    "yaml",
    "tomlq",
    "semver",
    "bc",
];

pub(crate) fn is_bashkit_builtin(cmd: &str) -> bool {
    BASHKIT_BUILTINS.contains(&cmd)
}

pub fn search_file(
    path: &Path,
    pattern: &Regex,
    exts: &[&str],
    max_file_size: usize,
    results: &mut Vec<String>,
    count: &mut usize,
    max_matches: usize,
) -> Result<(), ToolError> {
    if !exts.is_empty() {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let matches_ext = exts
            .iter()
            .any(|ext| name.to_lowercase().ends_with(&ext.to_lowercase()));
        if !matches_ext {
            return Ok(());
        }
    }

    if is_binary_filename(&path.to_string_lossy()) {
        return Ok(());
    }

    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Ok(()),
    };

    if content.len() > max_file_size {
        return Ok(());
    }

    if content.contains('\0') {
        return Ok(());
    }

    for (line_num, line) in content.lines().enumerate() {
        if *count >= max_matches {
            results.push("[... results truncated ...]".to_string());
            return Ok(());
        }
        if pattern.is_match(line) {
            results.push(format!("{}:{}: {}", path.display(), line_num + 1, line));
            *count += 1;
        }
    }

    Ok(())
}

/// Returns true for directory entries that recursive file walks should skip:
/// hidden dirs (except `.cargo`) and common dependency/build directories.
pub fn should_skip_walk_entry(name: &str) -> bool {
    if name.starts_with('.') && name != "." && name != ".." && name != ".cargo" {
        return true;
    }
    matches!(name, "node_modules" | "target" | ".git")
}

#[allow(clippy::too_many_arguments, clippy::only_used_in_recursion)]
pub fn walk_dir(
    root: &Path,
    dir: &Path,
    pattern: &Regex,
    exts: &[&str],
    max_file_size: usize,
    results: &mut Vec<String>,
    count: &mut usize,
    max_matches: usize,
) -> Result<(), ToolError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();

        if should_skip_walk_entry(&name) {
            continue;
        }

        if path.is_dir() {
            walk_dir(
                root,
                &path,
                pattern,
                exts,
                max_file_size,
                results,
                count,
                max_matches,
            )?;
        } else if path.is_file() {
            search_file(
                &path,
                pattern,
                exts,
                max_file_size,
                results,
                count,
                max_matches,
            )?;
        }
    }

    Ok(())
}

fn is_binary_filename(name: &str) -> bool {
    const BINARY_EXTS: &[&str] = &[
        ".png", ".jpg", ".jpeg", ".gif", ".bmp", ".ico", ".webp", ".pdf", ".doc", ".docx", ".xls",
        ".xlsx", ".ppt", ".pptx", ".zip", ".tar", ".gz", ".bz2", ".xz", ".7z", ".rar", ".exe",
        ".dll", ".so", ".dylib", ".o", ".a", ".lib", ".bin", ".dat", ".class", ".pyc", ".pyo",
        ".wasm", ".mp3", ".mp4", ".avi", ".mov", ".mkv", ".wav", ".flac", ".ttf", ".otf", ".woff",
        ".woff2", ".eot", ".db", ".sqlite", ".sqlite3", ".mdb",
    ];
    let lower = name.to_lowercase();
    BINARY_EXTS.iter().any(|ext| lower.ends_with(ext))
}

pub fn find_git_dir() -> Result<PathBuf, ToolError> {
    let cwd = std::env::current_dir()
        .map_err(|e| ToolError::Message(format!("cannot get current directory: {e}")))?;
    for ancestor in cwd.ancestors() {
        let dot_git = ancestor.join(".git");
        if dot_git.exists() {
            return dot_git
                .canonicalize()
                .map_err(|e| ToolError::Message(format!("cannot resolve .git path: {e}")));
        }
    }
    Err(ToolError::Message(
        "not in a git repository (no .git directory found)".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_random_ua_is_member_of_pool() {
        for _ in 0..50 {
            let ua = random_ua();
            assert!(USER_AGENTS.contains(&ua), "UA not in pool: {ua}");
        }
    }

    #[cfg(feature = "browser")]
    #[test]
    fn test_js_literal_escapes_metacharacters() {
        assert_eq!(js_literal("plain"), "\"plain\"");
        assert_eq!(js_literal("a\\b"), r#""a\\b""#);
        assert_eq!(js_literal("a\"b"), r#""a\"b""#);
        assert_eq!(js_literal("a'b"), "\"a'b\""); // single quotes need no escaping in JSON
        assert_eq!(js_literal("a\nb"), r#""a\nb""#);
    }

    #[cfg(feature = "browser")]
    #[test]
    fn test_js_literal_roundtrips() {
        let s = "sel\\ector's\"with\nnewline";
        let lit = js_literal(s);
        let parsed: String = serde_json::from_str(&lit).unwrap();
        assert_eq!(parsed, s);
    }
}
