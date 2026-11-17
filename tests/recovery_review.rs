//! Regression cases found during independent recovery review.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use serde_json::json;
use sk::core::{Branch, Repository, Stack, State};
use tempfile::TempDir;

fn git(path: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(path)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

async fn fixture() -> (TempDir, Repository, State) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    git(path, &["init", "-b", "main"]);
    git(path, &["config", "user.name", "Fixture"]);
    git(path, &["config", "user.email", "fixture@example.invalid"]);
    git(path, &["config", "commit.gpgSign", "false"]);
    std::fs::write(path.join("file.txt"), "base\n").unwrap();
    git(path, &["add", "."]);
    git(path, &["commit", "-m", "base"]);
    let base = git(path, &["rev-parse", "HEAD"]);
    git(path, &["switch", "-c", "a"]);
    std::fs::write(path.join("feature.txt"), "feature\n").unwrap();
    git(path, &["add", "."]);
    git(path, &["commit", "-m", "feature"]);
    let repo = Repository::at(path).await.unwrap();
    let state = State {
        version: 1,
        stacks: vec![Stack {
            id: "fixture".into(),
            trunk: "main".into(),
            remote: "origin".into(),
            github_number: None,
            branches: vec![Branch {
                name: "a".into(),
                base,
                remote_oid: None,
                fetched_oid: None,
                pr: None,
            }],
        }],
    };
    repo.save(&state).await.unwrap();
    (dir, repo, state)
}

#[tokio::test]
async fn external_trunk_change_invalidates_a_replay_plan() {
    let (dir, repo, state) = fixture().await;
    let plan = sk::core::plan_stack(&repo, &state.stacks[0], &BTreeMap::new())
        .await
        .unwrap();
    let old = git(dir.path(), &["rev-parse", "main"]);
    let new = git(
        dir.path(),
        &["commit-tree", "main^{tree}", "-p", "main", "-m", "external"],
    );
    git(dir.path(), &["update-ref", "refs/heads/main", &new, &old]);
    assert!(
        sk::core::apply_plan(&repo, &state, &state, &plan)
            .await
            .is_err()
    );
    assert_eq!(git(dir.path(), &["rev-parse", "main"]), new);
}

async fn pending_transaction() -> (TempDir, Repository) {
    let (dir, repo, state) = fixture().await;
    let tip = git(dir.path(), &["rev-parse", "a"]);
    let journal = json!({
        "original": state,
        "proposed": state,
        "original_branch": "a",
        "checkout_oid": tip,
        "old": {"a": tip},
        "new": {"a": tip},
        "kind": "transaction",
        "stack_index": 0,
        "next": 0
    });
    std::fs::write(
        repo.dir().join("journal.json"),
        serde_json::to_vec_pretty(&journal).unwrap(),
    )
    .unwrap();
    (dir, repo)
}

#[tokio::test]
async fn continue_does_not_erase_work_written_after_interruption() {
    let (dir, repo) = pending_transaction().await;
    std::fs::write(dir.path().join("file.txt"), "work after crash\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_sk"))
        .current_dir(dir.path())
        .arg("continue")
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("Checkout changed since operation"),
        "{output:?}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        "work after crash\n"
    );
    assert!(repo.dir().join("journal.json").exists());
}

#[tokio::test]
async fn abort_does_not_erase_work_written_after_interruption() {
    let (dir, repo) = pending_transaction().await;
    std::fs::write(dir.path().join("file.txt"), "work after crash\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_sk"))
        .current_dir(dir.path())
        .arg("abort")
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("Checkout changed since operation"),
        "{output:?}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        "work after crash\n"
    );
    assert!(repo.dir().join("journal.json").exists());
}

#[tokio::test]
async fn transaction_recovery_before_and_after_refs() {
    for published in [false, true] {
        for action in ["continue", "abort"] {
            let (dir, repo, state) = fixture().await;
            let p = dir.path();
            let old = git(p, &["rev-parse", "a"]);
            std::fs::write(p.join("feature.txt"), "new tree\n").unwrap();
            git(p, &["add", "."]);
            git(p, &["commit", "-m", "new tree"]);
            let new = git(p, &["rev-parse", "a"]);
            git(p, &["reset", "--hard", &old]);
            let journal = json!({
                "original": state,
                "proposed": state,
                "original_branch": "a",
                "checkout_oid": old,
                "old": {"a": old},
                "new": {"a": new},
                "kind": "transaction",
                "stack_index": 0,
                "next": 0
            });
            std::fs::write(
                repo.dir().join("journal.json"),
                serde_json::to_vec_pretty(&journal).unwrap(),
            )
            .unwrap();
            if published {
                git(p, &["update-ref", "refs/heads/a", &new, &old]);
            }
            let output = Command::new(env!("CARGO_BIN_EXE_sk"))
                .current_dir(p)
                .arg(action)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{published}/{action}: {output:?}"
            );
            let expected = if action == "continue" { &new } else { &old };
            assert_eq!(&git(p, &["rev-parse", "a"]), expected);
            assert!(git(p, &["status", "--porcelain"]).is_empty());
            assert!(!repo.dir().join("journal.json").exists());
        }
    }
}

#[tokio::test]
async fn interrupted_transaction_rejects_external_ref_update() {
    let (dir, repo) = pending_transaction().await;
    let p = dir.path();
    let new = git(p, &["commit-tree", "a^{tree}", "-p", "a", "-m", "external"]);
    git(p, &["update-ref", "refs/heads/a", &new]);
    for action in ["continue", "abort"] {
        let output = Command::new(env!("CARGO_BIN_EXE_sk"))
            .current_dir(p)
            .arg(action)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert_eq!(git(p, &["rev-parse", "a"]), new);
        assert!(repo.dir().join("journal.json").exists());
    }
}
