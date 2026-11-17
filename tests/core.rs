#[test]
fn branch_subject_becomes_slug() {
    assert_eq!(sk::core::slug("feat: Foo bar!"), "feat-foo-bar");
    assert_eq!(sk::core::slug("!!!"), "change");
}

use sk::core::{Branch, Repository, Stack, State};
use std::process::Command;
fn git(path: &std::path::Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(path)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().into()
}
#[tokio::test]
async fn amended_parent_replays_without_changing_refs_until_apply() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-b", "main"]);
    git(p, &["config", "user.email", "test@example.invalid"]);
    git(p, &["config", "user.name", "Test"]);
    git(p, &["config", "commit.gpgsign", "false"]);
    git(p, &["commit", "--allow-empty", "-m", "base"]);
    let base = git(p, &["rev-parse", "HEAD"]);
    git(p, &["switch", "-c", "a"]);
    std::fs::write(p.join("a"), "a").unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-m", "A"]);
    let old_a = git(p, &["rev-parse", "HEAD"]);
    git(p, &["switch", "-c", "b"]);
    std::fs::write(p.join("b"), "b").unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-m", "B"]);
    let old_b = git(p, &["rev-parse", "HEAD"]);
    git(p, &["switch", "a"]);
    std::fs::write(p.join("a"), "new a").unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "--amend", "--no-edit"]);
    let stack = Stack {
        id: "s".into(),
        trunk: "main".into(),
        remote: "origin".into(),
        github_number: None,
        branches: vec![
            Branch {
                name: "a".into(),
                base,
                remote_oid: None,
                fetched_oid: None,
                pr: None,
            },
            Branch {
                name: "b".into(),
                base: old_a,
                remote_oid: None,
                fetched_oid: None,
                pr: None,
            },
        ],
    };
    let repo = Repository::at(p).await.unwrap();
    let plan = sk::core::plan_stack(&repo, &stack, &Default::default())
        .await
        .unwrap();
    assert_eq!(git(p, &["rev-parse", "b"]), old_b);
    assert_ne!(plan["b"], old_b);
    let state = State {
        version: 1,
        stacks: vec![stack],
    };
    sk::core::apply_plan(&repo, &state, &state, &plan)
        .await
        .unwrap();
    assert_eq!(git(p, &["rev-parse", "b"]), plan["b"]);
    assert!(repo.clean().await.is_ok());
}

fn sk(path: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_sk"))
        .current_dir(path)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap()
}
fn conflict_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-b", "main"]);
    git(p, &["config", "user.email", "test@example.invalid"]);
    git(p, &["config", "user.name", "Test"]);
    git(p, &["config", "commit.gpgsign", "false"]);
    git(p, &["commit", "--allow-empty", "-m", "base"]);
    std::fs::write(p.join("file"), "A\n").unwrap();
    git(p, &["add", "."]);
    assert!(sk(p, &["create", "a", "-m", "A"]).status.success());
    std::fs::write(p.join("file"), "B\n").unwrap();
    git(p, &["add", "."]);
    assert!(sk(p, &["create", "b", "-m", "B"]).status.success());
    git(p, &["switch", "a"]);
    std::fs::write(p.join("file"), "amended A\n").unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "--amend", "--no-edit"]);
    dir
}
#[tokio::test]
async fn conflict_abort_restores_all_refs_and_checkout() {
    let dir = conflict_repo();
    let p = dir.path();
    let a = git(p, &["rev-parse", "a"]);
    let b = git(p, &["rev-parse", "b"]);
    let result = sk(p, &["restack"]);
    assert!(!result.status.success());
    assert!(p.join(".git/sk/journal.json").exists());
    let result = sk(p, &["abort"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(git(p, &["rev-parse", "a"]), a);
    assert_eq!(git(p, &["rev-parse", "b"]), b);
    assert_eq!(git(p, &["branch", "--show-current"]), "a");
    assert!(git(p, &["status", "--porcelain"]).is_empty());
}
#[tokio::test]
async fn conflict_continue_updates_recorded_boundary() {
    let dir = conflict_repo();
    let p = dir.path();
    assert!(!sk(p, &["restack"]).status.success());
    std::fs::write(p.join("file"), "resolved\n").unwrap();
    git(p, &["add", "."]);
    let result = sk(p, &["continue"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let repo = Repository::at(p).await.unwrap();
    let state = repo.load().await.unwrap();
    assert_eq!(
        state.stacks[0].branches[1].base,
        git(p, &["rev-parse", "a"])
    );
    assert_eq!(git(p, &["branch", "--show-current"]), "a");
}

#[tokio::test]
async fn dirty_restack_and_middle_create_preserve_work() {
    let dir = conflict_repo();
    let p = dir.path();
    std::fs::write(p.join("untracked"), "important").unwrap();
    let before = git(p, &["status", "--porcelain"]);
    let a = git(p, &["rev-parse", "a"]);
    assert!(!sk(p, &["restack"]).status.success());
    assert!(
        !sk(p, &["create", "oops", "-a", "-m", "oops"])
            .status
            .success()
    );
    assert_eq!(git(p, &["status", "--porcelain"]), before);
    assert_eq!(git(p, &["rev-parse", "a"]), a);
    assert!(!p.join(".git/sk/journal.json").exists());
}
#[tokio::test]
async fn foreign_worktree_blocks_rewrite_without_a_journal() {
    let dir = conflict_repo();
    let other = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["worktree", "add", other.path().to_str().unwrap(), "b"]);
    let result = sk(p, &["restack"]);
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("worktree")
            || String::from_utf8_lossy(&result.stderr).contains("checked out")
    );
    assert!(!p.join(".git/sk/journal.json").exists());
}
