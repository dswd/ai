use crate::cli::Cli;
use crate::config::Config;
use crate::memory;
use crate::policy::{Action, Policy};
use crate::skills;
use std::sync::Arc;

pub(crate) const DEFAULT_SYSTEM_PROMPT: &str = "You are a CLI assistant. Keep responses concise. \
     For multi-step tasks, work methodically and report progress.";

/// The resolved memory database path. Memory is enabled by default; `--no-memory`
/// turns it off and the config `memory:` key points at a different database.
pub(crate) fn memory_path(cli: &Cli, config: &Config) -> Option<std::path::PathBuf> {
    if cli.no_memory {
        None
    } else {
        Some(config.memory_path_resolved())
    }
}

/// Open the memory database, running the one-time session backfill. `None` when
/// memory is disabled with `--no-memory`.
pub(crate) fn open_memory(
    cli: &Cli,
    config: &Config,
) -> anyhow::Result<Option<Arc<memory::Memory>>> {
    let Some(path) = memory_path(cli, config) else {
        return Ok(None);
    };
    let embedder: Arc<dyn crate::embed::Embedder> = {
        #[cfg(feature = "embed")]
        {
            Arc::new(crate::embed::FastembedEmbedder::new(
                &config.embedding_model_resolved(),
                config.memory_max_distance,
            ))
        }
        #[cfg(not(feature = "embed"))]
        {
            log::info!("semantic memory is unavailable in this build (no embedding support)");
            Arc::new(crate::embed::DisabledEmbedder)
        }
    };
    let memory = memory::Memory::open(&path, embedder)?;
    if let Err(e) = memory.backfill_sessions(&config.session_dir_resolved()) {
        log::warn!("transcript backfill failed: {e}");
    }
    Ok(Some(Arc::new(memory)))
}

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

pub(crate) fn missing_permissions_note(policy: &Policy, containerized: bool) -> Option<String> {
    let p = permissions_available(policy);
    let mut lines: Vec<&str> = Vec::new();
    if !(p.web_fetch && p.web_search) {
        lines.push("Web access not allowed. If required, ask the user to add `--web` to the call.");
    }
    if !p.read {
        lines.push(
            "Read access not allowed. If required, ask the user to add `-r <PATH>` (e.g. `-r .`) to the call.",
        );
    }
    if !p.write {
        lines.push(
            "Write access not allowed. If required, ask the user to add `-w <PATH>` (e.g. `-w ./src`) to the call.",
        );
    }
    if !p.execute && !containerized {
        lines.push(
            "External command execution not allowed. If required, ask the user to add `-x <PATTERN>` (e.g. `-x cargo,git`) to the call.",
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
    containerized: bool,
) -> anyhow::Result<(String, Option<Arc<memory::Memory>>)> {
    let mut system_prompt = cli
        .system
        .clone()
        .or_else(|| config.system_prompt.clone())
        .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string());

    let memory = if let Some(mem) = open_memory(cli, config)? {
        let md = mem.summary();
        system_prompt = format!("{system_prompt}\n\n{md}");
        Some(mem)
    } else {
        None
    };

    system_prompt = format!("{system_prompt}\n\n{}", policy.summary());
    if let Some(note) = missing_permissions_note(policy, containerized) {
        system_prompt.push_str(&note);
    }

    if cli.is_interactive() {
        system_prompt.push_str(
            "\n\nIf the user asks to quit or exit the program, call the `exit_program` tool.",
        );
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
        let note = missing_permissions_note(&policy, false).expect("note should be present");
        assert!(note.contains("--web"));
        assert!(note.contains("-r <PATH>"));
        assert!(note.contains("-w <PATH>"));
        assert!(note.contains("-x <PATTERN>"));
    }

    #[test]
    fn test_missing_permissions_note_web_only() {
        let policy = policy_with(&[Action::Read, Action::Write, Action::Execute]);
        let note = missing_permissions_note(&policy, false).expect("note should be present");
        assert!(note.contains("--web"));
        assert!(!note.contains("-r <PATH>"));
        assert!(!note.contains("-w <PATH>"));
        assert!(!note.contains("-x <PATTERN>"));
    }

    #[test]
    fn test_missing_permissions_note_ask_mode() {
        let mut policy = Policy::default();
        policy.ask = true;
        assert!(missing_permissions_note(&policy, false).is_none());
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
        assert!(missing_permissions_note(&policy, false).is_none());
    }

    #[test]
    fn test_exit_guidance_only_interactive() {
        use clap::Parser;

        let mut interactive = Cli::parse_from(["ai", "--no-memory"]);
        interactive.interactive = true;
        let (prompt, _) = assemble_system_prompt(
            &interactive,
            &Config::default(),
            &Policy::default(),
            &[],
            false,
        )
        .unwrap();
        assert!(prompt.contains("exit_program"));

        let oneshot = Cli::parse_from(["ai", "run", "hi", "--no-memory"]);
        let (prompt, _) =
            assemble_system_prompt(&oneshot, &Config::default(), &Policy::default(), &[], false)
                .unwrap();
        assert!(!prompt.contains("exit_program"));
    }

    #[test]
    fn test_memory_default_on_and_relocatable() {
        use clap::Parser;

        let plain = Cli::parse_from(["ai"]);
        let config = Config::default();
        assert_eq!(
            memory_path(&plain, &config),
            Some(config.memory_path_resolved())
        );

        let relocated = Config {
            memory: Some(std::path::PathBuf::from("/tmp/x.db")),
            ..Config::default()
        };
        assert_eq!(
            memory_path(&plain, &relocated),
            Some(std::path::PathBuf::from("/tmp/x.db"))
        );

        let off = Cli::parse_from(["ai", "--no-memory"]);
        assert_eq!(memory_path(&off, &config), None);
    }
}
