use crate::cli::Cli;
use crate::config::Config;
use crate::memory;
use crate::policy::{Action, Policy};
use crate::skills;
use std::sync::Arc;

pub(crate) const DEFAULT_SYSTEM_PROMPT: &str = "You are a CLI assistant. Keep responses concise. \
     For multi-step tasks, work methodically and report progress.";

#[derive(Debug, Clone, Copy)]
pub(crate) struct Permissions {
    pub(crate) read: bool,
    pub(crate) write: bool,
    pub(crate) web_fetch: bool,
    pub(crate) web_search: bool,
    pub(crate) execute: bool,
}

pub(crate) fn permissions_available(policy: &Policy) -> Permissions {
    Permissions {
        read: policy.ask || policy.has_any_allow(&Action::Read),
        write: policy.ask || policy.has_any_allow(&Action::Write),
        web_fetch: policy.ask || policy.has_any_allow(&Action::WebFetch),
        web_search: policy.ask || policy.has_any_allow(&Action::WebSearch),
        execute: policy.ask || policy.has_any_allow(&Action::Execute),
    }
}

pub(crate) fn missing_permissions_note(policy: &Policy) -> Option<String> {
    let p = permissions_available(policy);
    let mut lines: Vec<&str> = Vec::new();
    if !(p.web_fetch && p.web_search) {
        lines.push("Web access not allowed. If required, ask the user to add `--web` to the call.");
    }
    if !p.read {
        lines.push(
            "Read access not allowed. If required, ask the user to add `-r <PATH>` (e.g. `-r=.`) to the call.",
        );
    }
    if !p.write {
        lines.push(
            "Write access not allowed. If required, ask the user to add `-w <PATH>` (e.g. `-w=./src`) to the call.",
        );
    }
    if !p.execute {
        lines.push(
            "External command execution not allowed. If required, ask the user to add `-x <PATTERN>` (e.g. `-x=cargo,git`) to the call.",
        );
    }
    if lines.is_empty() {
        return None;
    }
    let mut out = String::from(
        "\n\n### Missing permissions\n\
         The following tools are unavailable. If a task needs them, ask the user to re-run with the corresponding flag:\n",
    );
    for l in lines {
        out.push_str("- ");
        out.push_str(l);
        out.push('\n');
    }
    Some(out)
}

pub(crate) fn assemble_system_prompt(
    cli: &Cli,
    config: &Config,
    policy: &Policy,
    skills: &[skills::Skill],
) -> anyhow::Result<(String, Option<Arc<memory::Memory>>)> {
    let mut system_prompt = cli
        .system
        .clone()
        .or_else(|| config.system_prompt.clone())
        .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string());

    let memory = if let Some(memory_path) = &cli.memory {
        let path = if memory_path.is_empty() {
            config.memory_path_resolved()
        } else {
            std::path::PathBuf::from(memory_path)
        };
        let mem = Arc::new(memory::Memory::load(&path)?);
        let md = mem.summary();
        system_prompt = format!("{system_prompt}\n\n{md}");
        Some(mem)
    } else {
        None
    };

    system_prompt = format!("{system_prompt}\n\n{}", policy.summary());
    if let Some(note) = missing_permissions_note(policy) {
        system_prompt.push_str(&note);
    }

    if !skills.is_empty() {
        system_prompt = format!("{system_prompt}\n\n{}", skills::summary(skills));
    }

    Ok((system_prompt, memory))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::PolicyRule;

    fn policy_with(actions: &[Action]) -> Policy {
        let mut policy = Policy::default();
        for action in actions {
            policy.add_cli_rule(PolicyRule::Allow(action.clone(), "**".to_string()));
        }
        policy
    }

    #[test]
    fn test_permissions_all_allowed() {
        let policy = policy_with(&[
            Action::Read,
            Action::Write,
            Action::WebFetch,
            Action::WebSearch,
            Action::Execute,
        ]);
        let p = permissions_available(&policy);
        assert!(p.read && p.write && p.web_fetch && p.web_search && p.execute);
    }

    #[test]
    fn test_permissions_none_allowed() {
        let policy = Policy::default();
        let p = permissions_available(&policy);
        assert!(!p.read && !p.write && !p.web_fetch && !p.web_search && !p.execute);
    }

    #[test]
    fn test_missing_permissions_note_default() {
        let policy = Policy::default();
        let note = missing_permissions_note(&policy).expect("note should be present");
        assert!(note.contains("--web"));
        assert!(note.contains("-r <PATH>"));
        assert!(note.contains("-w <PATH>"));
        assert!(note.contains("-x <PATTERN>"));
    }

    #[test]
    fn test_missing_permissions_note_web_only() {
        let policy = policy_with(&[Action::Read, Action::Write, Action::Execute]);
        let note = missing_permissions_note(&policy).expect("note should be present");
        assert!(note.contains("--web"));
        assert!(!note.contains("-r <PATH>"));
        assert!(!note.contains("-w <PATH>"));
        assert!(!note.contains("-x <PATTERN>"));
    }

    #[test]
    fn test_missing_permissions_note_ask_mode() {
        let mut policy = Policy::default();
        policy.ask = true;
        assert!(missing_permissions_note(&policy).is_none());
    }

    #[test]
    fn test_missing_permissions_note_all_allowed() {
        let policy = policy_with(&[
            Action::Read,
            Action::Write,
            Action::WebFetch,
            Action::WebSearch,
            Action::Execute,
        ]);
        assert!(missing_permissions_note(&policy).is_none());
    }
}
