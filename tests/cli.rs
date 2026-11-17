use std::process::Command;

#[test]
fn help_works_without_git_or_network_tools() {
    let output = Command::new(env!("CARGO_BIN_EXE_sk"))
        .arg("--help")
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    for name in ["create", "restack", "sync", "submit", "checkout", "pr"] {
        assert!(text.contains(name), "missing command {name}");
    }
}

#[test]
fn unknown_command_fails_before_repository_discovery() {
    let output = Command::new(env!("CARGO_BIN_EXE_sk"))
        .arg("not-a-command")
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unknown command")
    );
}
