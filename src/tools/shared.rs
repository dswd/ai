use std::path::{Path, PathBuf};

use regex::Regex;

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
    "echo","printf","cat","nl","read","mapfile","readarray","log","cd","pwd","ls",
    "find","tree","pushd","popd","dirs","true","false","exit","return","break","continue",
    "test","[","assert","export","set","unset","local","shift","source",".","eval",
    "readonly","times","declare","typeset","let","dotenv","envsubst",
    "bash","sh","exec",":","trap","caller","getopts","shopt",
    "command","type","which","hash","alias","unalias","compgen","fc","help",
    "grep","rg","sed","awk","head","tail","sort","uniq","cut","tr","wc",
    "paste","column","diff","comm","strings","tac","rev","seq","expr",
    "fold","expand","unexpand","join","split","iconv","shuf","template",
    "mkdir","mktemp","mkfifo","rm","cp","mv","touch",
    "chmod","chown","ln","rmdir","realpath","readlink","truncate","glob","patch",
    "file","stat","less",
    "tar","gzip","gunzip","zip","unzip",
    "od","xxd","hexdump","base64",
    "md5sum","sha1sum","sha256sum","verify",
    "sleep","date","basename","dirname","timeout","wait","watch","yes","kill","clear",
    "numfmt","retry","parallel",
    "df","du","xargs","tee",
    "whoami","hostname","uname","id","env","printenv","history",
    "json","csv","yaml","tomlq","semver","bc",
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

        if name.starts_with('.') && name != "." && name != ".." && name != ".cargo" {
            continue;
        }

        if name == "node_modules" || name == "target" || name == ".git" {
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
