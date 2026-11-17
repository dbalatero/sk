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

#[test]
fn every_command_and_alias_has_offline_help() {
    let directory = tempfile::tempdir().unwrap();
    for command in [
        "create",
        "submit",
        "ss",
        "sync",
        "restack",
        "continue",
        "abort",
        "log",
        "log short",
        "ls",
        "checkout",
        "co",
        "up",
        "down",
        "pr",
    ] {
        for flag in ["--help", "-h"] {
            let output = Command::new(env!("CARGO_BIN_EXE_sk"))
                .args(command.split_whitespace())
                .arg(flag)
                .current_dir(directory.path())
                .env("PATH", "")
                .output()
                .unwrap();
            assert!(output.status.success(), "{command} {flag}: {output:?}");
            let text = String::from_utf8(output.stdout).unwrap();
            assert!(text.contains("Usage: sk "), "{command}: {text}");
            assert!(text.contains("--help"), "{command}: {text}");
        }
    }
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn help_subcommand_and_alias_describe_supported_options() {
    for command in ["submit", "ss"] {
        let output = Command::new(env!("CARGO_BIN_EXE_sk"))
            .args(["help", command])
            .env("PATH", "")
            .output()
            .unwrap();
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        for option in ["--draft", "--no-edit", "--stack"] {
            assert!(text.contains(option), "missing {option}: {text}");
        }
    }
    let output = Command::new(env!("CARGO_BIN_EXE_sk"))
        .args(["help", "does-not-exist"])
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[test]
fn help_flag_in_commit_message_is_not_intercepted() {
    let directory = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_sk"))
        .args(["create", "-m", "--help"])
        .current_dir(directory.path())
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("Usage:"));
}
