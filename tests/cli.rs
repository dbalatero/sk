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
        "c",
        "aliases",
        "submit",
        "s",
        "ss",
        "sync",
        "restack",
        "reorder",
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

fn alias_command(config: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sk"));
    command.env("XDG_CONFIG_HOME", config).env("PATH", "");
    command
}

#[test]
fn aliases_creates_defaults_without_overwriting_edits() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sk/aliases");
    let output = alias_command(directory.path())
        .arg("aliases")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let defaults = std::fs::read_to_string(&path).unwrap();
    assert!(defaults.contains("c create\n"));
    assert!(defaults.contains("s submit\n"));
    assert!(defaults.contains("ss submit --stack\n"));
    std::fs::write(&path, "# my aliases:\nfdsa create\n").unwrap();
    assert!(
        alias_command(directory.path())
            .arg("aliases")
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "# my aliases:\nfdsa create\n"
    );
}

#[test]
fn aliases_expand_with_arguments_and_offline_help() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("sk")).unwrap();
    std::fs::write(directory.path().join("sk/aliases"),
        "# my aliases:\nss stack --submit\nfdsa create\nnext fdsa\nmsg create -m 'hello world'\n").unwrap();
    for (command, expected) in [
        ("c", "create"),
        ("s", "submit"),
        ("ss", "submit"),
        ("fdsa", "create"),
        ("next", "create"),
        ("msg", "create"),
    ] {
        for args in [vec![command, "--help"], vec!["help", command]] {
            let output =
                alias_command(directory.path()).args(args).output().unwrap();
            assert!(output.status.success(), "{command}: {output:?}");
            assert!(
                String::from_utf8_lossy(&output.stdout)
                    .contains(&format!("Usage: sk {expected}"))
            );
        }
    }
}

#[test]
fn alias_cycles_fail_with_an_actionable_error() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("sk")).unwrap();
    std::fs::write(directory.path().join("sk/aliases"), "a b\nb a\n").unwrap();
    let output = alias_command(directory.path()).arg("a").output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("alias cycle"));
}
