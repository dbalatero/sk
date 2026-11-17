use std::path::Path;
use std::process::Command;

use serde_json::json;
use sk::core::{Branch, Repository, Stack, State};

fn git(path: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(path)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    String::from_utf8(out.stdout).unwrap().trim().into()
}
fn sk(path: &Path, command: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_sk"))
        .current_dir(path)
        .arg(command)
        .output()
        .unwrap()
}
#[tokio::test]
async fn reconciliation_conflict_can_continue_or_abort_entire_operation() {
    for action in ["continue", "abort"] {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-b", "main"]);
        git(p, &["config", "user.name", "Fixture"]);
        git(p, &["config", "user.email", "test@example.invalid"]);
        git(p, &["config", "commit.gpgsign", "false"]);
        git(p, &["commit", "--allow-empty", "-m", "root"]);
        let base = git(p, &["rev-parse", "HEAD"]);
        git(p, &["switch", "-c", "a"]);
        std::fs::write(p.join("file"), "published\n").unwrap();
        git(p, &["add", "."]);
        git(p, &["commit", "-m", "published"]);
        let published = git(p, &["rev-parse", "HEAD"]);
        std::fs::write(p.join("file"), "local\n").unwrap();
        git(p, &["add", "."]);
        git(p, &["commit", "-m", "local"]);
        let local = git(p, &["rev-parse", "HEAD"]);
        git(p, &["switch", "-c", "remote-a", &published]);
        std::fs::write(p.join("file"), "remote\n").unwrap();
        git(p, &["add", "."]);
        git(p, &["commit", "-m", "remote"]);
        let remote = git(p, &["rev-parse", "HEAD"]);
        git(p, &["switch", "a"]);
        let repo = Repository::at(p).await.unwrap();
        let state = State {
            version: 1,
            stacks: vec![Stack {
                id: "a".into(),
                trunk: "main".into(),
                remote: "origin".into(),
                github_number: None,
                branches: vec![Branch {
                    name: "a".into(),
                    base: base.clone(),
                    remote_oid: Some(published.clone()),
                    fetched_oid: Some(remote.clone()),
                    pr: None,
                }],
            }],
        };
        repo.save(&state).await.unwrap();
        let mut proposed = state.clone();
        proposed.stacks[0].branches[0].remote_oid = Some(remote.clone());
        let journal = json!({
            "original":state,"proposed":proposed,
            "original_branch":"a","checkout_oid":local,
            "old":{"a":local},"new":{"a":local},
            "kind":"rebase","stack_index":0,"next":0,
            "reconcile_steps":[{"name":"a","base":published,
                "onto":remote,"head":local}],"reconcile_next":0,
        });
        std::fs::write(
            repo.dir().join("journal.json"),
            serde_json::to_vec(&journal).unwrap(),
        )
        .unwrap();
        let paused = sk(p, "continue");
        assert!(!paused.status.success(), "{paused:?}");
        assert!(p.join(".git/rebase-merge").exists());
        if action == "continue" {
            std::fs::write(p.join("file"), "combined\n").unwrap();
            git(p, &["add", "."]);
        }
        let result = sk(p, action);
        assert!(result.status.success(), "{action}: {result:?}");
        assert_eq!(git(p, &["branch", "--show-current"]), "a");
        assert!(git(p, &["status", "--porcelain"]).is_empty());
        assert!(!repo.dir().join("journal.json").exists());
        if action == "abort" {
            assert_eq!(git(p, &["rev-parse", "a"]), local);
        } else {
            git(p, &["merge-base", "--is-ancestor", &remote, "a"]);
            assert_eq!(
                repo.load().await.unwrap().stacks[0].branches[0].base,
                base
            );
        }
    }
}
