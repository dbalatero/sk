use sk::core::{Repository, State};
use std::path::Path;
use std::process::{Command, Output};

fn git(p: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(p)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap().trim().into()
}
fn sk(p: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sk"))
        .current_dir(p)
        .args(args)
        .env("XDG_CONFIG_HOME", p.join(".git/config-home"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_ALLOW_PROTOCOL", "")
        .output()
        .unwrap()
}
fn success(p: &Path, args: &[&str]) -> Output {
    let out = sk(p, args);
    assert!(out.status.success(), "{args:?}: {out:?}");
    out
}
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-b", "main"]);
    git(p, &["config", "user.name", "Test"]);
    git(p, &["config", "user.email", "test@example.invalid"]);
    git(p, &["config", "commit.gpgsign", "false"]);
    git(p, &["commit", "--allow-empty", "-m", "root"]);
    dir
}
fn commit(p: &Path, file: &str, text: &str) -> String {
    std::fs::write(p.join(file), text).unwrap();
    git(p, &["add", file]);
    git(p, &["commit", "-m", text]);
    git(p, &["rev-parse", "HEAD"])
}
async fn state(p: &Path) -> State {
    Repository::at(p).await.unwrap().load().await.unwrap()
}

#[tokio::test]
async fn modify_amends_staged_changes_and_requires_explicit_restack() {
    let dir = fixture();
    let p = dir.path();
    success(p, &["create", "a"]);
    commit(p, "a", "A");
    success(p, &["create", "b"]);
    let b = commit(p, "b", "B");
    git(p, &["switch", "a"]);
    std::fs::write(p.join("a"), "new A").unwrap();
    git(p, &["add", "a"]);
    std::fs::write(p.join("untracked"), "keep").unwrap();
    let out = success(p, &["m", "-m", "amended"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("sk restack"));
    assert_eq!(git(p, &["rev-list", "--count", "main..a"]), "1");
    assert_eq!(git(p, &["rev-parse", "b"]), b);
    assert!(git(p, &["status", "--porcelain"]).contains("untracked"));
    std::fs::remove_file(p.join("untracked")).unwrap();
    success(p, &["restack"]);
    assert_eq!(
        state(p).await.stacks[0].branches[1].base,
        git(p, &["rev-parse", "a"])
    );
}

#[test]
fn modify_empty_branch_creates_commit_without_amending_trunk() {
    let dir = fixture();
    let p = dir.path();
    let trunk = git(p, &["rev-parse", "main"]);
    success(p, &["create", "a"]);
    std::fs::write(p.join("a"), "A").unwrap();
    success(p, &["modify", "-am", "A"]);
    assert_eq!(git(p, &["rev-parse", "main"]), trunk);
    assert_eq!(git(p, &["rev-list", "--count", "main..a"]), "1");
    std::fs::write(p.join("a"), "A2").unwrap();
    success(p, &["modify", "-cam", "second"]);
    assert_eq!(git(p, &["rev-list", "--count", "main..a"]), "2");
    git(p, &["switch", "main"]);
    assert!(!sk(p, &["modify", "-m", "nope"]).status.success());
}

#[tokio::test]
async fn track_recursively_adopts_existing_branches_without_checkout() {
    let dir = fixture();
    let p = dir.path();
    git(p, &["switch", "-c", "a"]);
    commit(p, "a", "A");
    git(p, &["switch", "-c", "b"]);
    commit(p, "b", "B");
    std::fs::write(p.join("untracked"), "keep").unwrap();
    let refs = git(p, &["show-ref"]);
    success(p, &["track"]);
    let state = state(p).await;
    assert_eq!(
        state.stacks[0]
            .branches
            .iter()
            .map(|b| b.name.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(git(p, &["show-ref"]), refs);
    assert_eq!(git(p, &["branch", "--show-current"]), "b");
    success(p, &["track"]);
}

#[tokio::test]
async fn track_rejects_sibling_and_ambiguous_parent_without_state_changes() {
    let dir = fixture();
    let p = dir.path();
    success(p, &["create", "a"]);
    commit(p, "a", "A");
    success(p, &["create", "b"]);
    commit(p, "b", "B");
    git(p, &["switch", "a"]);
    git(p, &["switch", "-c", "sibling"]);
    commit(p, "s", "sibling");
    let before = serde_json::to_value(state(p).await).unwrap();
    assert!(!sk(p, &["track", "--parent", "a"]).status.success());
    assert_eq!(serde_json::to_value(state(p).await).unwrap(), before);
    git(p, &["switch", "b"]);
    git(p, &["branch", "same-tip"]);
    git(p, &["switch", "-c", "child"]);
    commit(p, "child", "child");
    assert!(!sk(p, &["track"]).status.success());
    success(p, &["track", "--parent", "b"]);
}

#[tokio::test]
async fn split_each_commit_preserves_original_tip_and_descendants() {
    let dir = fixture();
    let p = dir.path();
    success(p, &["create", "feature"]);
    let a = commit(p, "a", "first");
    let b = commit(p, "b", "second");
    let tip = commit(p, "c", "third");
    success(p, &["create", "child"]);
    let child = commit(p, "d", "fourth");
    git(p, &["switch", "feature"]);
    success(p, &["split", "--by-commit", "--each"]);
    let state = state(p).await;
    let branches = &state.stacks[0].branches;
    assert_eq!(branches.len(), 4);
    assert_eq!(git(p, &["rev-parse", &branches[0].name]), a);
    assert_eq!(git(p, &["rev-parse", &branches[1].name]), b);
    assert_eq!(branches[2].name, "feature");
    assert_eq!(branches[2].base, b);
    assert_eq!(git(p, &["rev-parse", "feature"]), tip);
    assert_eq!(git(p, &["rev-parse", "child"]), child);
    assert_eq!(git(p, &["branch", "--show-current"]), "feature");
    assert!(git(p, &["status", "--porcelain"]).is_empty());
}

#[test]
fn top_and_bottom_select_branches_not_trunk() {
    let dir = fixture();
    let p = dir.path();
    success(p, &["create", "a"]);
    commit(p, "a", "A");
    success(p, &["create", "b"]);
    commit(p, "b", "B");
    success(p, &["bottom"]);
    assert_eq!(git(p, &["branch", "--show-current"]), "a");
    success(p, &["top"]);
    assert_eq!(git(p, &["branch", "--show-current"]), "b");
    git(p, &["switch", "main"]);
    success(p, &["bottom"]);
    assert_eq!(git(p, &["branch", "--show-current"]), "a");
    assert!(!sk(p, &["top", "2"]).status.success());
}

fn split_editor(p: &Path, text: &str, fail: bool) -> Output {
    let editor = p.join(".git/split-editor.sh");
    std::fs::write(
        &editor,
        "printf '%s\\n' \"$SK_TEST_TEXT\" > \"$1\"\nexit \"$SK_TEST_EXIT\"\n",
    )
    .unwrap();
    Command::new(env!("CARGO_BIN_EXE_sk"))
        .current_dir(p)
        .args(["split", "--by-commit"])
        .env("GIT_EDITOR", format!("sh '{}'", editor.display()))
        .env("SK_TEST_TEXT", text)
        .env("SK_TEST_EXIT", if fail { "1" } else { "0" })
        .env("XDG_CONFIG_HOME", p.join(".git/config-home"))
        .output()
        .unwrap()
}

#[tokio::test]
async fn split_editor_groups_commits_and_preserves_pr_association() {
    let dir = fixture();
    let p = dir.path();
    success(p, &["create", "feature"]);
    commit(p, "a", "first");
    let boundary = commit(p, "b", "second");
    let tip = commit(p, "c", "third");
    let repo = Repository::at(p).await.unwrap();
    let mut original = repo.load().await.unwrap();
    original.stacks[0].branches[0].pr = Some(sk::core::PullRequest {
        number: 42,
        url: "https://github.com/o/r/pull/42".into(),
        state: "OPEN".into(),
        draft: false,
        head_oid: tip.clone(),
    });
    repo.save(&original).await.unwrap();
    let out = split_editor(
        p,
        &format!("{boundary} foundation\n{tip} feature\n"),
        false,
    );
    assert!(out.status.success(), "{out:?}");
    let branches = &state(p).await.stacks[0].branches;
    assert_eq!(branches.len(), 2);
    assert_eq!(branches[0].name, "foundation");
    assert!(branches[0].pr.is_none());
    assert_eq!(branches[1].pr.as_ref().unwrap().number, 42);
    assert_eq!(git(p, &["rev-list", "--count", "main..foundation"]), "2");
    assert_eq!(git(p, &["rev-list", "--count", "foundation..feature"]), "1");
}

#[test]
fn invalid_split_edits_and_editor_failure_preserve_state_and_refs() {
    let dir = fixture();
    let p = dir.path();
    success(p, &["create", "feature"]);
    let first = commit(p, "a", "first");
    let tip = commit(p, "b", "last");
    let refs = git(p, &["show-ref"]);
    let state = std::fs::read(p.join(".git/sk/state.json")).unwrap();
    for text in [
        String::new(),
        format!("{first} lower"),
        format!("{tip} feature\n{first} lower"),
        format!("{first} main\n{tip} feature"),
        format!("{first} lower\n{tip} renamed"),
        format!("{first} lower\n{first} duplicate\n{tip} feature"),
    ] {
        assert!(!split_editor(p, &text, false).status.success());
        assert_eq!(git(p, &["show-ref"]), refs);
        assert_eq!(std::fs::read(p.join(".git/sk/state.json")).unwrap(), state);
        assert!(!p.join(".git/sk/journal.json").exists());
    }
    assert!(
        !split_editor(p, &format!("{first} lower\n{tip} feature"), true)
            .status
            .success()
    );
    assert_eq!(git(p, &["show-ref"]), refs);
}

#[test]
fn dirty_split_is_rejected_and_navigation_preserves_dirty_work() {
    let dir = fixture();
    let p = dir.path();
    success(p, &["create", "a"]);
    commit(p, "a", "A");
    success(p, &["create", "b"]);
    commit(p, "b", "B");
    commit(p, "c", "C");
    std::fs::write(p.join("a"), "dirty").unwrap();
    let refs = git(p, &["show-ref"]);
    assert!(!sk(p, &["split", "--each"]).status.success());
    assert_eq!(git(p, &["show-ref"]), refs);
    success(p, &["bottom"]);
    success(p, &["top"]);
    assert_eq!(std::fs::read_to_string(p.join("a")).unwrap(), "dirty");
}

#[test]
fn modify_help_like_message_is_committed_and_colliding_split_names_are_unique()
{
    let dir = fixture();
    let p = dir.path();
    success(p, &["create", "feature"]);
    commit(p, "a", "same");
    std::fs::write(p.join("b"), "B").unwrap();
    success(p, &["modify", "-cam", "--help"]);
    assert_eq!(git(p, &["log", "-1", "--format=%s"]), "--help");
    commit(p, "c", "same");
    commit(p, "d", "last");
    git(p, &["branch", "feature-same", "main"]);
    success(p, &["split", "--each"]);
    assert_eq!(
        git(p, &["rev-parse", "feature-same"]),
        git(p, &["rev-parse", "main"])
    );
    assert_eq!(
        git(p, &["log", "-1", "--format=%s", "feature-same-2"]),
        "same"
    );
    assert_eq!(
        git(p, &["log", "-1", "--format=%s", "feature-same-3"]),
        "same"
    );
}

#[tokio::test]
async fn track_respects_configured_trunk_and_explicit_parent() {
    let dir = fixture();
    let p = dir.path();
    git(p, &["branch", "-m", "develop"]);
    git(p, &["config", "sk.trunk", "develop"]);
    git(p, &["switch", "-c", "existing"]);
    commit(p, "a", "A");
    git(p, &["switch", "develop"]);
    let refs = git(p, &["show-ref"]);
    success(p, &["track", "existing", "-p", "develop"]);
    assert_eq!(state(p).await.stacks[0].trunk, "develop");
    assert_eq!(git(p, &["branch", "--show-current"]), "develop");
    assert_eq!(git(p, &["show-ref"]), refs);
    assert!(!sk(p, &["track", "develop"]).status.success());
}

#[test]
fn boundary_navigation_requires_stack_choice_from_shared_trunk() {
    let dir = fixture();
    let p = dir.path();
    success(p, &["create", "a"]);
    commit(p, "a", "A");
    git(p, &["switch", "main"]);
    success(p, &["create", "other"]);
    commit(p, "other", "other");
    git(p, &["switch", "main"]);
    for command in ["top", "bottom"] {
        let out = sk(p, &[command]);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("terminal"));
        assert_eq!(git(p, &["branch", "--show-current"]), "main");
    }
}

#[tokio::test]
async fn create_insert_preserves_descendants_until_restack() {
    let dir = fixture();
    let p = dir.path();
    success(p, &["create", "a"]);
    commit(p, "a", "A");
    success(p, &["create", "b"]);
    let b = commit(p, "b", "B");
    success(p, &["create", "c"]);
    let c = commit(p, "c", "C");
    git(p, &["switch", "a"]);
    std::fs::write(p.join("inserted"), "I").unwrap();
    let out = success(p, &["create", "inserted", "--insert", "-am", "I"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("sk restack"));
    let s = state(p).await;
    let names: Vec<_> = s.stacks[0]
        .branches
        .iter()
        .map(|b| b.name.as_str())
        .collect();
    assert_eq!(names, ["a", "inserted", "b", "c"]);
    assert_eq!(git(p, &["rev-parse", "b"]), b);
    assert_eq!(git(p, &["rev-parse", "c"]), c);
    success(p, &["restack"]);
    assert_eq!(git(p, &["branch", "--show-current"]), "inserted");
    assert_eq!(git(p, &["log", "--format=%s", "inserted..b"]), "B");
    assert_eq!(git(p, &["log", "--format=%s", "b..c"]), "C");
    assert_eq!(git(p, &["show", "c:inserted"]), "I");
}

#[tokio::test]
async fn create_insert_from_trunk_and_tip() {
    let dir = fixture();
    let p = dir.path();
    success(p, &["create", "a", "-i"]);
    commit(p, "a", "A");
    success(p, &["create", "b", "-i"]);
    commit(p, "b", "B");
    git(p, &["switch", "main"]);
    success(p, &["create", "bottom", "-i"]);
    commit(p, "bottom", "bottom");
    success(p, &["restack"]);
    let s = state(p).await;
    assert_eq!(s.stacks.len(), 1);
    let names: Vec<_> = s.stacks[0]
        .branches
        .iter()
        .map(|b| b.name.as_str())
        .collect();
    assert_eq!(names, ["bottom", "a", "b"]);
    assert_eq!(git(p, &["show", "b:bottom"]), "bottom");
}

#[test]
fn create_insert_rejects_ambiguous_trunk_before_staging() {
    let dir = fixture();
    let p = dir.path();
    success(p, &["create", "a"]);
    git(p, &["switch", "main"]);
    success(p, &["create", "b"]);
    git(p, &["switch", "main"]);
    std::fs::write(p.join("work"), "keep").unwrap();
    let before = git(p, &["status", "--porcelain"]);
    let out = sk(p, &["create", "inserted", "-i", "-am", "I"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("multiple stacks"));
    assert_eq!(git(p, &["status", "--porcelain"]), before);
    assert_eq!(git(p, &["branch", "--show-current"]), "main");
}
