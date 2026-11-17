use std::path::Path;
use std::process::{Command, Output};

use serde_json::json;
use tempfile::TempDir;

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}: {:?}", args, output);
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn fixture() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.name", "Fixture"]);
    git(root, &["config", "user.email", "fixture@example.invalid"]);
    git(root, &["config", "commit.gpgSign", "false"]);
    git(root, &["commit", "--allow-empty", "-m", "initial"]);
    let main = git(root, &["rev-parse", "HEAD"]);
    git(root, &["switch", "-c", "a"]);
    std::fs::write(root.join("a.txt"), "a\n").unwrap();
    git(root, &["add", "a.txt"]);
    git(root, &["commit", "-m", "feature A"]);
    let a = git(root, &["rev-parse", "HEAD"]);
    git(root, &["switch", "-c", "b"]);
    std::fs::write(root.join("b.txt"), "b\n").unwrap();
    git(root, &["add", "b.txt"]);
    git(root, &["commit", "-m", "feature B"]);
    let state = json!({
        "version": 1,
        "stacks": [{
            "id": "fixture", "trunk": "main", "remote": "origin",
            "github_number": null,
            "branches": [
                {"name": "a", "base": main, "remote_oid": null,
                 "fetched_oid": null, "pr": null},
                {"name": "b", "base": a, "remote_oid": null,
                 "fetched_oid": null, "pr": null}
            ]
        }]
    });
    std::fs::create_dir(root.join(".git/sk")).unwrap();
    std::fs::write(
        root.join(".git/sk/state.json"),
        serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();
    dir
}

fn sk(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sk"))
        .current_dir(root)
        .args(args)
        .env("GIT_ALLOW_PROTOCOL", "")
        .env("GH_TOKEN", "unusable-offline-test-token")
        .output()
        .unwrap()
}

#[test]
fn navigate_and_render_a_stack_without_a_remote() {
    let dir = fixture();
    let root = dir.path();
    let result = sk(root, &["ls"]);
    assert!(result.status.success(), "{result:?}");
    assert!(String::from_utf8_lossy(&result.stdout).contains("◉  b"));
    assert!(sk(root, &["down"]).status.success());
    assert_eq!(git(root, &["branch", "--show-current"]), "a");
    assert!(sk(root, &["down", "5"]).status.success());
    assert_eq!(git(root, &["branch", "--show-current"]), "main");
    assert!(sk(root, &["up", "2"]).status.success());
    assert_eq!(git(root, &["branch", "--show-current"]), "b");
    assert!(sk(root, &["co", "a"]).status.success());
    assert_eq!(git(root, &["branch", "--show-current"]), "a");
}

#[test]
fn checkout_preserves_work_and_refuses_overwriting_conflicts() {
    let dir = fixture();
    let root = dir.path();
    std::fs::write(root.join("a.txt"), "local work\n").unwrap();
    assert!(sk(root, &["down"]).status.success());
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).unwrap(),
        "local work\n"
    );
    assert!(!sk(root, &["down"]).status.success());
    assert_eq!(git(root, &["branch", "--show-current"]), "a");
}

#[test]
fn picker_without_terminal_and_missing_pr_fail_without_side_effects() {
    let dir = fixture();
    let root = dir.path();
    let picker = sk(root, &["co"]);
    assert!(!picker.status.success());
    assert!(String::from_utf8_lossy(&picker.stderr).contains("terminal"));
    let pr = sk(root, &["pr"]);
    assert!(!pr.status.success());
    assert!(String::from_utf8_lossy(&pr.stderr).contains("cached PR"));
    assert_eq!(git(root, &["branch", "--show-current"]), "b");
}
