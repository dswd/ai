use crate::config::Config;
use crate::output;
use crate::session::Session;
use crate::tools;
#[cfg(feature = "browser")]
use std::sync::Arc;

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
    let path = dir.join(format!("{name}.json"));
    if path.exists() {
        std::fs::remove_file(&path)?;
        output::stderr_line(&format!("Deleted session: {name}"));
    } else {
        anyhow::bail!("Session not found: {name}");
    }
    Ok(())
}
