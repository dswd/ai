use ansi_color_constants::*;
use log::{debug, warn};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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

impl Action {
    /// Spelling the policy-file parser accepts for this action.
    pub fn policy_name(&self) -> &'static str {
        match self {
            Action::Read => "read",
            Action::Write => "write",
            Action::Execute => "execute",
            Action::WebFetch => "web-fetch",
            Action::WebSearch => "web-search",
        }
    }
}

#[derive(Debug, Clone)]
pub enum PolicyRule {
    Allow(Action, String),
    Deny(Action, String),
}

/// Session-scoped approval memory. Shared by every clone of a [`Policy`] (all
/// tools hold clones), so a decision the user makes while one tool runs is
/// visible to the next. Rules live only for the process/session unless the user
/// chooses to persist them to the policy file.
#[derive(Debug, Default)]
pub struct ApprovalState {
    rules: Mutex<Vec<PolicyRule>>,
    /// Effective policy file that persisted rules are appended to. `None`
    /// disables the "create rule" option (e.g. `ai setup`).
    persist_path: Option<PathBuf>,
}

/// Sentinel [`crate::io::read_user_input`] returns on Ctrl-C/Ctrl-D.
const CANCEL_SENTINEL: &str = "/exit";

impl ApprovalState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Approval state that can persist rules to `path` (the effective policy
    /// file). Used by interactive sessions and one-off `--ask` runs.
    pub fn with_policy_file(path: PathBuf) -> Self {
        Self {
            persist_path: Some(path),
            ..Self::default()
        }
    }

    fn rules(&self) -> Vec<PolicyRule> {
        self.rules.lock().unwrap().clone()
    }

    fn add(&self, rule: PolicyRule) {
        self.rules.lock().unwrap().push(rule);
    }

    /// Ask the user to approve an unmatched action. Returns whether it is now
    /// allowed. `y` allows once; `r` opens the rule builder (allow/deny,
    /// editable subject, optional persistence); anything else denies.
    fn request(&self, action: &Action, target: &str) -> bool {
        self.request_with(action, target, &real_ask)
    }

    fn request_with(
        &self,
        action: &Action,
        target: &str,
        ask: &dyn Fn(&str, Option<&str>) -> Option<String>,
    ) -> bool {
        loop {
            let prompt = if self.persist_path.is_some() {
                format!("Allow {action} for {target}? [y=once, r=rule, N=deny] ")
            } else {
                format!("Allow {action} for {target}? [y=once, N=deny] ")
            };
            let answer = ask(&prompt, None).unwrap_or_default().to_lowercase();
            match answer.as_str() {
                "y" | "yes" => return true,
                "r" | "rule" if self.persist_path.is_some() => {
                    if let Some(allowed) = self.build_rule(action, target, ask) {
                        return allowed;
                    }
                }
                _ => return false,
            }
        }
    }

    /// Walk the rule builder: pick allow/deny, edit the pre-filled subject, then
    /// decide whether to persist it. Returns `Some(allowed)` when a committed
    /// rule covers `target`, or `None` to re-show the original prompt (the user
    /// cancelled, or the rule does not match this request).
    fn build_rule(
        &self,
        action: &Action,
        target: &str,
        ask: &dyn Fn(&str, Option<&str>) -> Option<String>,
    ) -> Option<bool> {
        let direction = ask("Create an allow or deny rule? [a=allow, d=deny] ", None)
            .unwrap_or_default()
            .to_lowercase();
        let allow = match direction.as_str() {
            "a" | "allow" => true,
            "d" | "deny" => false,
            _ => return None,
        };

        let subject = match ask("Rule subject (edit as needed): ", Some(target)) {
            Some(s) if s != CANCEL_SENTINEL => s,
            _ => return None,
        };
        let subject = normalize_rule_subject(action, &subject);

        let rule = if allow {
            PolicyRule::Allow(action.clone(), subject.clone())
        } else {
            PolicyRule::Deny(action.clone(), subject.clone())
        };

        let persist = if let Some(path) = &self.persist_path {
            let line = format_rule_line(&rule);
            let answer = ask(
                &format!("Persist \"{line}\" to {}? [y/N] ", path.display()),
                None,
            );
            match answer.as_deref() {
                Some(CANCEL_SENTINEL) => return None,
                Some(a) => matches!(a.to_lowercase().as_str(), "y" | "yes"),
                None => false,
            }
        } else {
            false
        };

        self.add(rule.clone());
        if persist
            && let Some(path) = &self.persist_path
            && let Err(err) = append_rule(path, &rule)
        {
            warn!(
                "{YELLOW}\u{26A0} could not persist rule to {}: {err}; keeping it for this session only{RESET}",
                path.display()
            );
        }

        if matches_pattern(target, &subject) {
            Some(allow)
        } else {
            warn!(
                "{YELLOW}\u{26A0} rule \"{}\" does not cover {target}; asking again{RESET}",
                format_rule_line(&rule)
            );
            None
        }
    }
}

/// Read a line for the rule builder. `initial` pre-fills an editable subject.
fn real_ask(prompt: &str, initial: Option<&str>) -> Option<String> {
    match initial {
        Some(text) => crate::io::read_user_input_with_initial(prompt, text),
        None => crate::io::read_user_input(prompt),
    }
}

/// Normalize an edited rule subject the same way `-r`/`-w` do: paths get `~`
/// expanded, are resolved against the cwd, and have `.`/`..` collapsed while
/// wildcards are preserved. Other actions are stored trimmed, as typed.
fn normalize_rule_subject(action: &Action, subject: &str) -> String {
    match action {
        Action::Read | Action::Write => {
            let cwd = std::env::current_dir().unwrap_or_default();
            resolve_policy_pattern(subject, &cwd)
        }
        _ => subject.trim().to_string(),
    }
}

fn format_rule_line(rule: &PolicyRule) -> String {
    match rule {
        PolicyRule::Allow(action, pattern) => {
            format!("allow {} {}", action.policy_name(), pattern)
        }
        PolicyRule::Deny(action, pattern) => {
            format!("deny {} {}", action.policy_name(), pattern)
        }
    }
}

/// Append one rule line to the policy file, creating the file and its parent
/// directories if needed. Existing content and formatting are preserved.
fn append_rule(path: &Path, rule: &PolicyRule) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(&format_rule_line(rule));
    content.push('\n');
    std::fs::write(path, content)
}

#[derive(Debug, Clone, Default)]
pub struct Policy {
    rules: Vec<PolicyRule>,
    cli_rules: Vec<PolicyRule>,
    pub ask: bool,
    pub approval: Option<Arc<ApprovalState>>,
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
            approval: None,
        }
    }

    pub fn add_cli_rule(&mut self, rule: PolicyRule) {
        self.cli_rules.push(rule);
    }

    /// Append an implicit default rule, evaluated after the policy file's own
    /// rules so an explicit `deny` still wins.
    pub fn add_default_rule(&mut self, rule: PolicyRule) {
        self.rules.push(rule);
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

        // Session-scoped decisions made earlier in this run.
        if let Some(approval) = &self.approval {
            for rule in &approval.rules() {
                match rule {
                    PolicyRule::Allow(a, pattern)
                        if a == action && matches_pattern(&target_norm, pattern) =>
                    {
                        return true;
                    }
                    PolicyRule::Deny(a, pattern)
                        if a == action && matches_pattern(&target_norm, pattern) =>
                    {
                        return false;
                    }
                    _ => {}
                }
            }
        }

        if self.ask
            && let Some(approval) = &self.approval
        {
            let allowed = approval.request(action, &target_norm);
            if allowed {
                debug!(
                    "{DIM}\u{2705} approved {:?} for {:?}{RESET}",
                    action, target_norm
                );
            } else {
                warn!(
                    "{RED}\u{274C} {:?} for {:?} (denied by user){RESET}",
                    action, target_norm
                );
            }
            return allowed;
        }

        warn!(
            "{RED}\u{274C} {:?} for {:?} (no matching rule){RESET}",
            action, target_norm
        );
        false
    }

    pub fn has_any_allow(&self, action: &Action) -> bool {
        self.cli_rules
            .iter()
            .chain(self.rules.iter())
            .any(|rule| matches!(rule, PolicyRule::Allow(a, _) if a == action))
    }

    /// All `allow` patterns for an action, CLI rules first.
    pub fn allow_patterns(&self, action: &Action) -> Vec<String> {
        self.cli_rules
            .iter()
            .chain(self.rules.iter())
            .filter_map(|rule| match rule {
                PolicyRule::Allow(a, pattern) if a == action => Some(pattern.clone()),
                _ => None,
            })
            .collect()
    }

    /// All `deny` patterns for an action, CLI rules first.
    pub fn deny_patterns(&self, action: &Action) -> Vec<String> {
        self.cli_rules
            .iter()
            .chain(self.rules.iter())
            .filter_map(|rule| match rule {
                PolicyRule::Deny(a, pattern) if a == action => Some(pattern.clone()),
                _ => None,
            })
            .collect()
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

        for chunk in crate::tools::shared::advertised_builtins().chunks(12) {
            lines.push(format!("  - {}", chunk.join(", ")));
        }

        lines.push(String::new());
        if self.ask {
            lines.push("You may ask for more permissions — the user will be asked to approve each request, and can turn an approval into a reusable policy rule.".to_string());
        } else {
            lines.push("Do not attempt actions beyond granted permissions; you may suggest the user re-run with the appropriate flag.".to_string());
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

    // A pattern that starts with a wildcard (e.g. `*.rs`, `**/*.rs`) has an
    // empty prefix. Resolve it against the base directory as a whole; joining
    // an empty prefix would otherwise produce `base*.rs`, which matches nothing
    // under the base.
    if prefix.is_empty() {
        let relative_str = normalize_path_separators(&relative_to.to_string_lossy());
        let base = if relative_str.is_empty() {
            String::from(".")
        } else {
            relative_str
        };
        return normalize_path_segments(&format!("{base}/{pattern}"));
    }

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

pub(crate) fn matches_pattern(target: &str, pattern: &str) -> bool {
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
    fn test_default_rule_allows_when_no_file_rule() {
        let mut policy = Policy::parse("");
        policy.add_default_rule(PolicyRule::Allow(Action::Read, "/skills".to_string()));
        assert!(policy.is_allowed(&Action::Read, "/skills/a/SKILL.md"));
        assert!(!policy.is_allowed(&Action::Read, "/etc/passwd"));
        assert!(policy.has_any_allow(&Action::Read));
    }

    #[test]
    fn test_explicit_deny_overrides_default_rule() {
        let mut policy = Policy::parse("deny read /skills");
        policy.add_default_rule(PolicyRule::Allow(Action::Read, "/skills".to_string()));
        assert!(!policy.is_allowed(&Action::Read, "/skills/a/SKILL.md"));
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

    #[test]
    fn test_resolve_policy_pattern_relative() {
        let cwd = std::path::Path::new("/work");
        assert_eq!(resolve_policy_pattern("src/**", cwd), "/work/src/**");
        // Bare-glob patterns keep the wildcard relative to the resolved base:
        // `*.rs` must match files under /work, not `/work*.rs`.
        assert_eq!(resolve_policy_pattern("*.rs", cwd), "/work/*.rs");
        assert_eq!(resolve_policy_pattern("**/*.rs", cwd), "/work/**/*.rs");
        assert_eq!(resolve_policy_pattern("**/test", cwd), "/work/**/test");
    }

    #[test]
    fn test_resolve_policy_pattern_absolute_and_wildcards() {
        let cwd = std::path::Path::new("/work");
        assert_eq!(resolve_policy_pattern("/etc/passwd", cwd), "/etc/passwd");
        assert_eq!(resolve_policy_pattern("**", cwd), "**");
        assert_eq!(resolve_policy_pattern("*", cwd), "*");
        assert_eq!(
            resolve_policy_pattern("/tmp/**/*.log", cwd),
            "/tmp/**/*.log"
        );
    }

    #[test]
    fn test_resolve_policy_pattern_home() {
        let home = home_dir();
        let home_str = home.to_string_lossy();
        let cwd = std::path::Path::new("/work");
        assert_eq!(
            resolve_policy_pattern("~/projects/**", cwd),
            format!("{home_str}/projects/**")
        );
    }

    #[test]
    fn test_resolve_policy_pattern_dot_segments() {
        let cwd = std::path::Path::new("/work");
        assert_eq!(
            resolve_policy_pattern("./src/../lib/**", cwd),
            "/work/lib/**"
        );
    }

    #[test]
    fn test_rule_line_round_trips_through_parser() {
        for (action, pattern) in [
            (Action::Read, "/tmp/x"),
            (Action::Write, "/tmp/y"),
            (Action::Execute, "cargo"),
            (Action::WebFetch, "https://example.com"),
            (Action::WebSearch, "rust async"),
        ] {
            let rule = PolicyRule::Allow(action.clone(), pattern.to_string());
            let line = format_rule_line(&rule);
            let parsed = Policy::parse(&line);
            assert!(
                parsed.is_allowed(&action, pattern),
                "line {line:?} did not round-trip"
            );
        }
    }

    #[test]
    fn test_append_rule_creates_and_appends() {
        let dir = std::env::temp_dir().join(format!("ai-policy-append-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("policy");
        append_rule(
            &path,
            &PolicyRule::Allow(Action::Read, "/tmp/a".to_string()),
        )
        .unwrap();
        append_rule(
            &path,
            &PolicyRule::Deny(Action::Write, "/tmp/b".to_string()),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "allow read /tmp/a\ndeny write /tmp/b\n"
        );
        let parsed = Policy::from_file(&path).unwrap();
        assert!(parsed.is_allowed(&Action::Read, "/tmp/a"));
        assert!(!parsed.is_allowed(&Action::Write, "/tmp/b"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_normalize_rule_subject() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(
            normalize_rule_subject(&Action::Read, "src/**"),
            resolve_policy_pattern("src/**", &cwd)
        );
        assert_eq!(
            normalize_rule_subject(&Action::WebFetch, " https://example.com "),
            "https://example.com"
        );
    }

    /// Scripted `ask`: returns the queued answers in order, ignoring the prompt.
    fn scripted<'a>(answers: &'a [&'a str]) -> impl Fn(&str, Option<&str>) -> Option<String> + 'a {
        let queue = std::cell::RefCell::new(answers.iter());
        move |_, _| queue.borrow_mut().next().map(|s| s.to_string())
    }

    fn temp_policy(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("ai-rule-builder-{tag}-{}", std::process::id()))
    }

    #[test]
    fn test_rule_builder_allows_and_persists() {
        let path = temp_policy("persist");
        let _ = std::fs::remove_file(&path);
        let state = ApprovalState::with_policy_file(path.clone());

        let ask = scripted(&["r", "a", "/tmp/**", "y"]);
        assert!(state.request_with(&Action::Read, "/tmp/file.txt", &ask));

        let loaded = Policy::from_file(&path).unwrap();
        assert!(loaded.is_allowed(&Action::Read, "/tmp/file.txt"));
        assert!(loaded.is_allowed(&Action::Read, "/tmp/deep/file.txt"));
        assert!(!loaded.is_allowed(&Action::Write, "/tmp/file.txt"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_rule_builder_deny_session_only() {
        let path = temp_policy("session");
        let _ = std::fs::remove_file(&path);
        let state = ApprovalState::with_policy_file(path.clone());

        let ask = scripted(&["r", "d", "/tmp/secret", "n"]);
        assert!(!state.request_with(&Action::Read, "/tmp/secret/key", &ask));
        assert!(
            !path.exists(),
            "declining persistence must not write a file"
        );
        assert!(matches!(
            state.rules().as_slice(),
            [PolicyRule::Deny(Action::Read, p)] if p == "/tmp/secret"
        ));
    }

    #[test]
    fn test_rule_builder_remembers_session_decision() {
        let path = temp_policy("remember");
        let _ = std::fs::remove_file(&path);
        let state = std::sync::Arc::new(ApprovalState::with_policy_file(path.clone()));

        // Create a session-only allow rule...
        let ask = scripted(&["r", "a", "/tmp/**", "n"]);
        assert!(state.request_with(&Action::Read, "/tmp/file.txt", &ask));

        // ...then a different target under it is allowed without prompting.
        let policy = Policy {
            approval: Some(state.clone()),
            ..Policy::default()
        };
        assert!(policy.is_allowed(&Action::Read, "/tmp/other.txt"));
        assert!(!path.exists());
    }

    #[test]
    fn test_rule_builder_non_matching_rule_reprompts() {
        let path = temp_policy("reprompt");
        let _ = std::fs::remove_file(&path);
        let state = ApprovalState::with_policy_file(path.clone());

        // The rule does not cover the target, so the original prompt is shown
        // again; the follow-up `y` allows this one call.
        let ask = scripted(&["r", "a", "/elsewhere/**", "n", "y"]);
        assert!(state.request_with(&Action::Read, "/tmp/file.txt", &ask));
        assert!(!path.exists());
    }

    #[test]
    fn test_rule_builder_cancel_reprompts_then_denies() {
        let path = temp_policy("cancel");
        let _ = std::fs::remove_file(&path);
        let state = ApprovalState::with_policy_file(path.clone());

        // Cancelling the direction step returns to the original prompt; `N` denies.
        let ask = scripted(&["r", "", "N"]);
        assert!(!state.request_with(&Action::Read, "/tmp/file.txt", &ask));
        assert!(state.rules().is_empty());

        // The Ctrl-C sentinel in the subject editor cancels the same way.
        let ask = scripted(&["r", "a", "/exit", "N"]);
        assert!(!state.request_with(&Action::Read, "/tmp/file.txt", &ask));
        assert!(state.rules().is_empty());
    }

    #[test]
    fn test_rule_builder_not_offered_without_policy_file() {
        let state = ApprovalState::new();
        // `r` is ignored when there is nowhere to persist, falling through to deny.
        let ask = scripted(&["r"]);
        assert!(!state.request_with(&Action::Read, "/tmp/file.txt", &ask));
        assert!(state.rules().is_empty());
    }
}
