use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ai"))
}

#[test]
fn version_prints() {
    let out = bin().arg("--version").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn help_lists_new_flags() {
    let out = bin().arg("--help").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("--completions"));
    assert!(stdout.contains("--no-color"));
    assert!(stdout.contains("--setup"));
    assert!(!stdout.contains("--init"));
}

#[test]
fn completions_bash_outputs_script() {
    let out = bin().arg("--completions=bash").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("_ai"));
}

#[test]
fn list_sessions_uses_temp_config() {
    let dir = std::env::temp_dir().join(format!("ai-it-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("config.yaml");
    std::fs::write(&config, format!("session_dir: {}\n", dir.display())).unwrap();

    let out = bin()
        .arg(format!("--config={}", config.display()))
        .arg("--list")
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("No saved sessions"));

    let _ = std::fs::remove_dir_all(&dir);
}
