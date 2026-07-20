use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Read,
    Write,
    Execute,
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
            .filter_map(|l| parse_line(l))
            .collect();

        Self {
            rules,
            cli_rules: Vec::new(),
        }
    }

    pub fn add_cli_rule(&mut self, rule: PolicyRule) {
        self.cli_rules.push(rule);
    }

    pub fn is_allowed(&self, action: &Action, target: &str) -> bool {
        let combined: Vec<&PolicyRule> = self.cli_rules.iter().chain(self.rules.iter()).collect();

        if combined.is_empty() {
            return false;
        }

        for rule in &combined {
            match rule {
                PolicyRule::Allow(a, pattern) if a == action => {
                    if matches_pattern(target, pattern) {
                        return true;
                    }
                }
                PolicyRule::Deny(a, pattern) if a == action => {
                    if matches_pattern(target, pattern) {
                        return false;
                    }
                }
                _ => {}
            }
        }

        false
    }

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
}

fn parse_line(line: &str) -> Option<PolicyRule> {
    let parts: Vec<&str> = line.splitn(3, ' ').collect();

    if parts.len() < 3 {
        return None;
    }

    let directive = parts[0].to_lowercase();
    let action_str = parts[1].to_lowercase();
    let pattern = parts[2].to_string();

    let is_allow = match directive.as_str() {
        "allow" => true,
        "deny" => false,
        _ => return None,
    };

    let action = match action_str.as_str() {
        "read" => Action::Read,
        "write" => Action::Write,
        "execute" => Action::Execute,
        _ => return None,
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

    for sub_pattern in pattern.split(',') {
        let sub_pattern = sub_pattern.trim();
        if sub_pattern.is_empty() {
            continue;
        }

        if sub_pattern.contains('*') {
            if let Ok(matcher) = glob::Pattern::new(sub_pattern) {
                if matcher.matches(target) {
                    return true;
                }
            }
            continue;
        }

        if target == sub_pattern || target.starts_with(sub_pattern) {
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
        policy.add_cli_rule(PolicyRule::Allow(Action::Read, "/tmp/allowed.txt".to_string()));
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
}
