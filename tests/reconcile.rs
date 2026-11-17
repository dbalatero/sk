use sk::core::{Branch, Repository, Stack};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .env("GIT_ALLOW_PROTOCOL", "")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[tokio::test]
async fn reconciliation_keeps_remote_and_unpublished_work_without_ref_edits() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.name", "Fixture"]);
    git(root, &["config", "user.email", "fixture@example.invalid"]);
    git(root, &["config", "commit.gpgSign", "false"]);
    git(root, &["commit", "--allow-empty", "-m", "initial"]);
    let base = git(root, &["rev-parse", "HEAD"]);
    git(root, &["switch", "-c", "feature"]);
    std::fs::write(root.join("published"), "original\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "published"]);
    let published = git(root, &["rev-parse", "HEAD"]);
    std::fs::write(root.join("local"), "unpublished\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "local"]);
    let local = git(root, &["rev-parse", "HEAD"]);
    git(root, &["switch", "-c", "remote-version", &published]);
    std::fs::write(root.join("published"), "remote amendment\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "--amend", "--no-edit"]);
    let fetched = git(root, &["rev-parse", "HEAD"]);
    git(root, &["switch", "feature"]);
    git(root, &["switch", "-c", "child"]);
    std::fs::write(root.join("child"), "descendant\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "descendant"]);
    let child = git(root, &["rev-parse", "HEAD"]);
    let repo = Repository::at(root).await.unwrap();
    let mut branch = Branch {
        name: "feature".into(),
        base,
        remote_oid: Some(published),
        fetched_oid: Some(fetched.clone()),
        pr: None,
    };
    let tip = sk::core::plan_reconciliation(&repo, &branch).await.unwrap();
    assert_eq!(git(root, &["rev-parse", "feature"]), local);
    assert_eq!(git(root, &["show", &format!("{tip}:local")]), "unpublished");
    assert_eq!(
        git(root, &["show", &format!("{tip}:published")]),
        "remote amendment"
    );
    git(root, &["merge-base", "--is-ancestor", &fetched, &tip]);
    let stack = Stack {
        id: "fixture".into(),
        trunk: "main".into(),
        remote: "origin".into(),
        github_number: None,
        branches: vec![
            branch.clone(),
            Branch {
                name: "child".into(),
                base: local.clone(),
                remote_oid: None,
                fetched_oid: None,
                pr: None,
            },
        ],
    };
    let heads = BTreeMap::from([("feature".into(), tip.clone())]);
    let plan = sk::core::plan_stack(&repo, &stack, &heads).await.unwrap();
    assert_eq!(git(root, &["rev-parse", "child"]), child);
    for (file, expected) in [
        ("local", "unpublished"),
        ("published", "remote amendment"),
        ("child", "descendant"),
    ] {
        assert_eq!(
            git(root, &["show", &format!("{}:{file}", plan["child"])]),
            expected
        );
    }
    branch.remote_oid = Some(fetched);
    assert!(sk::core::plan_reconciliation(&repo, &branch).await.is_err());
    assert_eq!(git(root, &["rev-parse", "feature"]), local);
}
