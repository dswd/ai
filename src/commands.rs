use crate::config::Config;
use crate::memory::{self, Hit, HitKind, Memory, MemoryEntry};
use crate::output;
use crate::session::Session;
use crate::tools;
use ansi_color_constants::*;
#[cfg(feature = "browser")]
use std::sync::Arc;

/// Wrap `text` in an ANSI `code` only when colors are enabled.
fn paint(color: bool, code: &str, text: &str) -> String {
    if color {
        format!("{code}{text}{RESET}")
    } else {
        text.to_string()
    }
}

/// Date part of an ISO-8601 timestamp.
fn day(ts: &str) -> &str {
    ts.get(..10).unwrap_or(ts)
}

/// Format one memory entry as a fact line plus an indented metadata line.
fn format_memory_entry(e: &MemoryEntry, color: bool) -> String {
    let mut meta: Vec<String> = Vec::new();
    if !e.tags.is_empty() {
        let tags = paint(color, CYAN, &e.tags.join(", "));
        meta.push(format!("{}: {tags}", paint(color, GREY, "tags")));
    }
    let origin_color = match e.origin.as_str() {
        "user" => GREEN,
        "agent" => BLUE,
        _ => GREY,
    };
    meta.push(format!(
        "{}: {}",
        paint(color, GREY, "origin"),
        paint(color, origin_color, &e.origin)
    ));
    if let Some(session) = &e.source_session {
        meta.push(format!(
            "{}: {}",
            paint(color, GREY, "session"),
            paint(color, BLUE, session)
        ));
    }
    meta.push(paint(color, GREY, &format!("created {}", day(&e.created))));
    meta.push(paint(color, GREY, &format!("used {}", day(&e.last_used))));

    let id = paint(color, GREY, &e.id);
    let text = e.text.replace('\n', " ");
    format!("{id}  {text}\n  {}", meta.join(&paint(color, GREY, " · ")))
}

/// Format one search hit as a kind badge plus an indented metadata line.
fn format_memory_hit(h: &Hit, query: &str, color: bool) -> String {
    let (badge, mut meta) = match h.kind {
        HitKind::Memory => {
            let mut meta = vec![format!(
                "{} {}",
                paint(color, GREY, "id"),
                paint(color, GREY, &h.key)
            )];
            if !h.tags.is_empty() {
                meta.push(format!(
                    "{}: {}",
                    paint(color, GREY, "tags"),
                    paint(color, CYAN, &h.tags.join(", "))
                ));
            }
            (paint(color, CYAN, "memory"), meta)
        }
        HitKind::Transcript => (
            paint(color, PURPLE, "transcript"),
            vec![format!(
                "{} {}",
                paint(color, GREY, "session"),
                paint(color, BLUE, h.session.as_deref().unwrap_or("?"))
            )],
        ),
    };
    meta.push(paint(color, GREY, &format!("created {}", day(&h.created))));
    meta.push(paint(color, GREY, &format!("score {:.2}", h.score)));

    let text = memory::fragment(&h.text, query);
    format!(
        "{badge}  {text}\n  {}",
        meta.join(&paint(color, GREY, " · "))
    )
}

pub(crate) async fn cmd_probe_web(query: &str, config: &Config) -> anyhow::Result<()> {
    #[cfg(feature = "browser")]
    let browser = match tools::BrowserState::new().await {
        Ok(bs) => Some(Arc::new(bs)),
        Err(e) => {
            output::stderr_line(&format!("warning: browser unavailable: {e}"));
            None
        }
    };

    println!("Probing web search engines for query: {query}");
    println!();

    #[cfg(feature = "browser")]
    let results =
        tools::search_probe::probe_web_search(query, &config.search, config.proxy.clone(), browser)
            .await;
    #[cfg(not(feature = "browser"))]
    let results =
        tools::search_probe::probe_web_search(query, &config.search, config.proxy.clone()).await;

    let mut any_ok = false;
    for r in &results {
        let status = if r.ok { "✅ OK " } else { "❌ FAIL" };
        let bytes = if r.ok {
            format!("{} bytes", r.bytes)
        } else {
            String::new()
        };
        println!(
            "{status}  {:<12} {}{:>8} ms  {}",
            r.engine,
            if r.ok { "" } else { " " },
            r.latency_ms,
            bytes
        );
        if !r.ok {
            println!("            reason: {}", r.detail);
        }
        if r.ok {
            any_ok = true;
        }
    }

    println!();
    println!("--- Results as returned to the AI ---");
    for r in &results {
        println!();
        println!("=== {} ===", r.engine);
        if r.ok {
            println!("{}", r.output);
        } else {
            println!("(no results — rejected: {})", r.detail);
        }
    }

    println!();
    if any_ok {
        println!("Result: at least one engine works.");
    } else {
        println!("Result: all engines failed. Check network / proxy / blocked markers above.");
    }
    Ok(())
}

/// Print every stored memory entry.
pub(crate) fn cmd_memory_list(mem: &Memory) -> anyhow::Result<()> {
    let entries = mem.list();
    let color = output::tty_enabled();
    if entries.is_empty() {
        println!("{}", paint(color, GREY, "No memory entries."));
        return Ok(());
    }
    println!(
        "{}",
        paint(
            color,
            BOLD,
            &format!(
                "🧠  {} memories · {} transcript excerpts",
                mem.count_memory(),
                mem.count_transcripts()
            )
        )
    );
    for e in &entries {
        println!();
        println!("{}", format_memory_entry(e, color));
    }
    Ok(())
}

/// Search memory and transcripts and print the fused ranking.
pub(crate) fn cmd_memory_search(mem: &Memory, query: &str) -> anyhow::Result<()> {
    let hits = mem.retrieve(query, 20);
    let color = output::tty_enabled();
    if hits.is_empty() {
        println!("{}", paint(color, GREY, "No matches."));
        return Ok(());
    }
    println!(
        "{}",
        paint(
            color,
            BOLD,
            &format!("🧠  {} matches for \"{query}\"", hits.len())
        )
    );
    for h in &hits {
        println!();
        println!("{}", format_memory_hit(h, query, color));
    }
    Ok(())
}

pub(crate) fn cmd_list_sessions(dir: &std::path::Path) -> anyhow::Result<()> {
    let names = Session::list(dir)?;
    if names.is_empty() {
        println!("No saved sessions.");
        return Ok(());
    }
    let current = crate::session::newest(dir).map(|(name, _)| name);
    for name in &names {
        let marker = if current.as_deref() == Some(name.as_str()) {
            "*"
        } else {
            " "
        };
        match Session::load(name, dir) {
            Ok(s) => {
                let provider = if s.provider.is_empty() {
                    "unknown"
                } else {
                    &s.provider
                };
                println!(
                    "{marker} {}  — {} messages, {} / {}, created {}",
                    name,
                    s.log.len(),
                    provider,
                    s.model,
                    s.created
                );
            }
            Err(e) => println!("{marker} {name}  — (unreadable: {e})"),
        }
    }

    Ok(())
}

pub(crate) fn cmd_delete_session(name: &str, dir: &std::path::Path) -> anyhow::Result<()> {
    if !crate::session::is_safe_name(name) {
        anyhow::bail!("invalid session name: {name:?}");
    }
    let resolved = if crate::session::is_dated(name) {
        name.to_string()
    } else {
        match crate::session::find_named(dir, name) {
            Some(found) => found,
            None => anyhow::bail!("Session not found: {name}"),
        }
    };
    let path = dir.join(format!("{resolved}.json"));
    if path.exists() {
        std::fs::remove_file(&path)?;
        output::stderr_line(&format!("Deleted session: {resolved}"));
    } else {
        anyhow::bail!("Session not found: {resolved}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::*;

    fn set_mtime(path: &std::path::Path, when: SystemTime) {
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(when)
            .unwrap();
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ai-del-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_delete_undated_removes_latest_match() {
        let dir = temp_dir("latest");
        let old = dir.join("2026-09-15_foo.json");
        let new = dir.join("2026-01-01_foo.json");
        std::fs::write(&old, "{}").unwrap();
        std::fs::write(&new, "{}").unwrap();
        set_mtime(&old, SystemTime::now() - Duration::from_secs(3600));
        set_mtime(&new, SystemTime::now());

        cmd_delete_session("foo", &dir).unwrap();
        assert!(!new.exists());
        assert!(old.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_delete_dated_is_exact() {
        let dir = temp_dir("dated");
        let older = dir.join("2026-01-01_foo.json");
        let newer = dir.join("2026-09-15_foo.json");
        std::fs::write(&older, "{}").unwrap();
        std::fs::write(&newer, "{}").unwrap();

        cmd_delete_session("2026-01-01_foo", &dir).unwrap();
        assert!(!older.exists());
        assert!(newer.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_delete_missing_errors() {
        let dir = temp_dir("missing");
        assert!(cmd_delete_session("nope", &dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn memory_entry() -> MemoryEntry {
        MemoryEntry {
            id: "m1".to_string(),
            text: "likes rust".to_string(),
            tags: vec!["lang".to_string(), "pref".to_string()],
            created: "2026-09-16T10:00:00Z".to_string(),
            updated: "2026-09-16T10:00:00Z".to_string(),
            last_used: "2026-09-17T12:34:56Z".to_string(),
            last_judged: None,
            origin: "user".to_string(),
            source_session: Some("2026-09-16_x".to_string()),
        }
    }

    #[test]
    fn test_format_memory_entry_plain() {
        let out = format_memory_entry(&memory_entry(), false);
        assert_eq!(
            out,
            "m1  likes rust\n  tags: lang, pref · origin: user · session: 2026-09-16_x · created 2026-09-16 · used 2026-09-17"
        );
        assert!(!out.contains('\x1b'));
    }

    #[test]
    fn test_format_memory_entry_color() {
        let out = format_memory_entry(&memory_entry(), true);
        assert!(out.contains(CYAN), "tags cyan");
        assert!(out.contains(GREEN), "user origin green");
        assert!(out.contains(BLUE), "session blue");
        assert!(out.contains(GREY), "labels grey");
    }

    #[test]
    fn test_format_memory_entry_omits_tags_and_session() {
        let mut e = memory_entry();
        e.tags.clear();
        e.source_session = None;
        e.origin = "agent".to_string();
        let out = format_memory_entry(&e, false);
        assert_eq!(
            out,
            "m1  likes rust\n  origin: agent · created 2026-09-16 · used 2026-09-17"
        );
    }

    #[test]
    fn test_format_memory_hit_plain() {
        let memory = Hit {
            kind: HitKind::Memory,
            key: "m1".to_string(),
            text: "likes rust".to_string(),
            tags: vec!["lang".to_string()],
            session: None,
            created: "2026-09-16T10:00:00Z".to_string(),
            score: 0.85,
        };
        assert_eq!(
            format_memory_hit(&memory, "rust", false),
            "memory  likes rust\n  id m1 · tags: lang · created 2026-09-16 · score 0.85"
        );

        let transcript = Hit {
            kind: HitKind::Transcript,
            key: "3".to_string(),
            text: "user: hi\nagent: yo".to_string(),
            tags: Vec::new(),
            session: Some("2026-09-16_x".to_string()),
            created: "2026-09-17T08:00:00Z".to_string(),
            score: 0.62,
        };
        assert_eq!(
            format_memory_hit(&transcript, "hi", false),
            "transcript  user: hi agent: yo\n  session 2026-09-16_x · created 2026-09-17 · score 0.62"
        );
    }

    #[test]
    fn test_format_memory_hit_fragments_long_text() {
        let hit = Hit {
            kind: HitKind::Memory,
            key: "m2".to_string(),
            text: format!("{} needle {}", "x".repeat(120), "y".repeat(120)),
            tags: Vec::new(),
            session: None,
            created: "2026-09-16T10:00:00Z".to_string(),
            score: 0.5,
        };
        let out = format_memory_hit(&hit, "needle", false);
        let text_line = out.lines().next().unwrap();
        assert!(text_line.contains("needle"));
        assert!(text_line.contains('…'));
        assert!(text_line.chars().count() <= "memory  ".chars().count() + memory::FRAGMENT_CHARS);
    }
}
