use ansi_color_constants::*;
use log::{debug, warn};
use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Read,
    Write,
    Execute,
    WebFetch,
    WebSearch,
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Action::Read => write!(f, "read"),
            Action::Write => write!(f, "write"),
            Action::Execute => write!(f, "execute"),
            Action::WebFetch => write!(f, "web fetch"),
            Action::WebSearch => write!(f, "web search"),
        }
    }
}

#[derive(Debug, Clone)]
pub enum PolicyRule {
    Allow(Action, String),
    Deny(Action, String),
}

#[derive(Debug, Clone, Default)]
pub struct Policy {
    rules: Vec<PolicyRule>,
    cli_rules: Vec<PolicyRule>,
    pub ask: bool,
}

impl Policy {
    pub fn from_file(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Ok(Self::parse(&content))
    }

    pub fn parse(input: &str) -> Self {
        let rules: Vec<PolicyRule> = input
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .filter_map(parse_line)
            .collect();

        Self {
            rules,
            cli_rules: Vec::new(),
            ask: false,
        }
    }

    pub fn add_cli_rule(&mut self, rule: PolicyRule) {
        self.cli_rules.push(rule);
    }

    pub fn is_allowed(&self, action: &Action, target: &str) -> bool {
        let target_norm = normalize_path_separators(target);
        let combined: Vec<&PolicyRule> = self.cli_rules.iter().chain(self.rules.iter()).collect();

        for rule in &combined {
            match rule {
                PolicyRule::Allow(a, pattern)
                    if a == action && matches_pattern(&target_norm, pattern) =>
                {
                    debug!(
                        "{DIM}\u{2705} {:?} for {:?} (matched rule: allow {}){RESET}",
                        action, target_norm, pattern
                    );
                    return true;
                }
                PolicyRule::Deny(a, pattern)
                    if a == action && matches_pattern(&target_norm, pattern) =>
                {
                    warn!(
                        "{RED}\u{274C} {:?} for {:?} (matched rule: deny {}){RESET}",
                        action, target_norm, pattern
                    );
                    return false;
                }
                _ => {}
            }
        }

        if self.ask {
            let mut stderr = io::stderr().lock();
            let _ = stderr.write_all(
                format!("\u{2753} Allow {:?} for {}? [y/N] ", action, target_norm).as_bytes(),
            );
            let _ = stderr.flush();
            let mut answer = String::new();
            if io::stdin().read_line(&mut answer).is_ok() {
                let trimmed = answer.trim().to_lowercase();
                if trimmed == "y" || trimmed == "yes" {
                    return true;
                }
            }
            false
        } else {
            warn!(
                "{RED}\u{274C} {:?} for {:?} (no matching rule){RESET}",
                action, target_norm
            );
            false
        }
    }

    #[allow(dead_code)]
    pub fn effective_allow_list(&self, action: &Action) -> Vec<String> {
        let mut allowed = Vec::new();
        let mut denied: HashMap<&str, bool> = HashMap::new();

        for rule in self.cli_rules.iter().chain(self.rules.iter()) {
            match rule {
                PolicyRule::Allow(a, pattern) if a == action => {
                    allowed.push(pattern.clone());
                }
                PolicyRule::Deny(a, pattern) if a == action => {
                    denied.insert(pattern, true);
                }
                _ => {}
            }
        }

        allowed
    }

    pub fn has_any_allow(&self, action: &Action) -> bool {
        self.cli_rules
            .iter()
            .chain(self.rules.iter())
            .any(|rule| matches!(rule, PolicyRule::Allow(a, _) if a == action))
    }

    pub fn summary(&self) -> String {
        let mut lines = vec!["## Policy".to_string()];

        for rule in self.cli_rules.iter().chain(self.rules.iter()) {
            match rule {
                PolicyRule::Allow(action, pattern) => {
                    lines.push(format!("- allow {} {}", action, pattern));
                }
                PolicyRule::Deny(action, pattern) => {
                    lines.push(format!("- deny {} {}", action, pattern));
                }
            }
        }

        lines.push(String::new());
        lines.push("### Available Built-in Commands".to_string());
        lines.push(String::new());
        lines.push(
            "These commands are available inside the `execute` tool without needing `-x` permissions. Filesystem access is governed by read/write policy."
                .to_string(),
        );

        for chunk in crate::tools::shared::BASHKIT_BUILTINS.chunks(12) {
            lines.push(format!("  - {}", chunk.join(", ")));
        }

        lines.push(String::new());
        if self.ask {
            lines.push("You may ask for more permissions — the user will be asked to approve each request.".to_string());
        } else {
            lines.push("Do not try additional permissions.".to_string());
        }

        lines.join("\n")
    }
}

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn normalize_path_separators(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        out.push(if ch == '\\' { '/' } else { ch });
    }
    out
}

fn normalize_path_segments(raw: &str) -> String {
    let normalized = normalize_path_separators(raw);
    let segments: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
    let mut out: Vec<&str> = Vec::new();
    for seg in segments {
        if seg == "." {
            continue;
        }
        if seg == ".." {
            out.pop();
            continue;
        }
        out.push(seg);
    }
    if out.is_empty() {
        if raw.starts_with("\\\\?\\") || raw.starts_with("//?/") {
            return normalize_path_separators(raw);
        }
        let has_root = raw.starts_with('/') || raw.len() >= 2 && raw.as_bytes()[1] == b':';
        return if has_root {
            "/".to_string()
        } else {
            ".".to_string()
        };
    }
    let mut result = String::with_capacity(normalized.len());
    let has_leading_slash = normalized.starts_with('/');
    if has_leading_slash || (normalized.len() >= 2 && normalized.as_bytes()[1] == b':') {
        if !has_leading_slash {
            result.push_str(&normalized[..2]);
        } else {
            result.push('/');
        }
    }
    result.push_str(&out.join("/"));
    result
}

/// Resolve a policy pattern: expand `~` and resolve relative paths
/// against `relative_to`. Wildcards (`*`, `**`) are preserved.
pub fn resolve_policy_pattern(pattern: &str, relative_to: &Path) -> String {
    if pattern == "*" || pattern == "**" {
        return pattern.to_string();
    }

    let pattern = normalize_path_separators(pattern);

    let (prefix, suffix) = split_at_wildcard(&pattern);

    let resolved = if let Some(rest) = prefix.strip_prefix('~') {
        let home = home_dir();
        let home_str = normalize_path_separators(&home.to_string_lossy());
        if rest.is_empty() {
            home_str
        } else if rest.starts_with('/') {
            format!("{}{}", home_str, rest)
        } else {
            pattern.to_string()
        }
    } else if prefix.starts_with('/') || prefix.len() >= 2 && prefix.as_bytes()[1] == b':' {
        prefix.to_string()
    } else {
        let relative_str = normalize_path_separators(&relative_to.to_string_lossy());
        let base = if relative_str.is_empty() {
            String::from(".")
        } else {
            relative_str
        };
        format!("{}/{}", base, prefix)
    };

    let resolved = normalize_path_segments(&resolved);
    if suffix.is_empty() {
        resolved
    } else if prefix.ends_with('/') && !resolved.ends_with('/') {
        format!("{}/{}", resolved, suffix)
    } else {
        format!("{}{}", resolved, suffix)
    }
}

fn split_at_wildcard(s: &str) -> (String, String) {
    for (i, ch) in s.char_indices() {
        if ch == '*' {
            return (s[..i].to_string(), s[i..].to_string());
        }
    }
    (s.to_string(), String::new())
}

fn parse_line(line: &str) -> Option<PolicyRule> {
    let parts: Vec<&str> = line.splitn(3, ' ').collect();

    if parts.len() < 3 {
        return None;
    }

    let directive = parts[0].to_lowercase();
    let action_str = parts[1].to_lowercase();
    let raw_pattern = parts[2].to_string();

    let is_allow = match directive.as_str() {
        "allow" => true,
        "deny" => false,
        _ => return None,
    };

    let action = match action_str.as_str() {
        "read" => Action::Read,
        "write" => Action::Write,
        "execute" => Action::Execute,
        "web-fetch" | "webfetch" => Action::WebFetch,
        "web-search" | "websearch" => Action::WebSearch,
        _ => return None,
    };

    let pattern = match action {
        Action::Read | Action::Write => resolve_policy_pattern(&raw_pattern, &home_dir()),
        _ => raw_pattern,
    };

    Some(if is_allow {
        PolicyRule::Allow(action, pattern)
    } else {
        PolicyRule::Deny(action, pattern)
    })
}

fn matches_pattern(target: &str, pattern: &str) -> bool {
    if pattern == "*" || pattern == "**" {
        return true;
    }

    let target_norm = normalize_path_separators(target);

    for sub_pattern in pattern.split(',') {
        let sub_pattern = sub_pattern.trim();
        if sub_pattern.is_empty() {
            continue;
        }
        let pat_norm = normalize_path_separators(sub_pattern);

        if pat_norm.contains('*') {
            if let Ok(matcher) = glob::Pattern::new(&pat_norm)
                && matcher.matches(&target_norm)
            {
                return true;
            }
            continue;
        }

        // Path-segment-aware matching: /tmp matches /tmp or /tmp/... but not /tmpfile
        if target_norm == pat_norm || target_norm.starts_with(&format!("{pat_norm}/")) {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_policy_denies() {
        let policy = Policy::parse("");
        assert!(!policy.is_allowed(&Action::Read, "/tmp/test.txt"));
        assert!(!policy.is_allowed(&Action::Write, "/tmp/test.txt"));
        assert!(!policy.is_allowed(&Action::Execute, "ls"));
    }

    #[test]
    fn test_allow_read() {
        let policy = Policy::parse("allow read /tmp/**");
        assert!(policy.is_allowed(&Action::Read, "/tmp/test.txt"));
        assert!(policy.is_allowed(&Action::Read, "/tmp/sub/file.txt"));
        assert!(!policy.is_allowed(&Action::Read, "/etc/passwd"));
        assert!(!policy.is_allowed(&Action::Write, "/tmp/test.txt"));
    }

    #[test]
    fn test_deny_overrides_in_order() {
        let policy = Policy::parse("allow read /tmp/**\ndeny read /tmp/secrets/**");
        assert!(policy.is_allowed(&Action::Read, "/tmp/data.txt"));
        assert!(
            policy.is_allowed(&Action::Read, "/tmp/secrets/key.txt"),
            "first match wins: allow /tmp/** matches before deny /tmp/secrets/**"
        );
    }

    #[test]
    fn test_deny_before_allow() {
        let policy = Policy::parse("deny read /tmp/secrets/**\nallow read /tmp/**");
        assert!(
            !policy.is_allowed(&Action::Read, "/tmp/secrets/key.txt"),
            "first match wins: deny /tmp/secrets/** matches before allow /tmp/**"
        );
        assert!(policy.is_allowed(&Action::Read, "/tmp/other.txt"));
    }

    #[test]
    fn test_execute_allow() {
        let policy = Policy::parse("allow execute cargo,git,npm,npx");
        assert!(policy.is_allowed(&Action::Execute, "cargo"));
        assert!(policy.is_allowed(&Action::Execute, "git"));
        assert!(!policy.is_allowed(&Action::Execute, "rm"));
    }

    #[test]
    fn test_cli_rules_precedence() {
        let mut policy = Policy::parse("deny read /tmp/**");
        policy.add_cli_rule(PolicyRule::Allow(
            Action::Read,
            "/tmp/allowed.txt".to_string(),
        ));
        assert!(policy.is_allowed(&Action::Read, "/tmp/allowed.txt"));
        assert!(!policy.is_allowed(&Action::Read, "/tmp/other.txt"));
    }

    #[test]
    fn test_first_match_wins_same_rule() {
        let policy = Policy::parse(
            "allow read /tmp/**\ndeny read /tmp/secret/**\nallow read /tmp/secret/public/**",
        );
        assert!(policy.is_allowed(&Action::Read, "/tmp/file.txt"));
        assert!(
            policy.is_allowed(&Action::Read, "/tmp/secret/key.txt"),
            "first match wins: allow /tmp/** matches before deny"
        );
        assert!(
            policy.is_allowed(&Action::Read, "/tmp/secret/public/readme.md"),
            "first match wins: allow /tmp/** matches before deny"
        );
    }

    #[test]
    fn test_segment_aware_matching() {
        let policy = Policy::parse("allow read /tmp");
        // /tmp matches itself
        assert!(policy.is_allowed(&Action::Read, "/tmp"));
        // /tmp matches children via path separator
        assert!(policy.is_allowed(&Action::Read, "/tmp/foo.txt"));
        assert!(policy.is_allowed(&Action::Read, "/tmp/sub/file.txt"));
        // /tmp does NOT match /tmpfile (prefix but not segment boundary)
        assert!(!policy.is_allowed(&Action::Read, "/tmpfile"));
        assert!(!policy.is_allowed(&Action::Read, "/tmp123/test.txt"));
    }

    #[test]
    fn test_home_expansion_in_policy() {
        let home = home_dir();
        let home_str = home.to_string_lossy();
        let policy = Policy::parse("deny read ~/projects/secret/**\nallow read ~/projects/**");
        let allowed_path = format!("{home_str}/projects/src/main.rs");
        let denied_path = format!("{home_str}/projects/secret/key.txt");
        assert!(policy.is_allowed(&Action::Read, &allowed_path));
        assert!(!policy.is_allowed(&Action::Read, &denied_path));
    }

    #[test]
    fn test_normalize_path_segments() {
        assert_eq!(normalize_path_segments("/"), "/");
        assert_eq!(normalize_path_segments("/foo/bar"), "/foo/bar");
        assert_eq!(normalize_path_segments("/foo/./bar"), "/foo/bar");
        assert_eq!(normalize_path_segments("/foo/../bar"), "/bar");
        assert_eq!(normalize_path_segments("/a/b/../c/./d"), "/a/c/d");
    }
}
