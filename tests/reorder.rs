use sk::core::Repository;
use std::path::Path;
use std::process::{Command, Output};

fn git(path: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(path)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    String::from_utf8(out.stdout).unwrap().trim().into()
}

fn sk(path: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sk"))
        .current_dir(path)
        .args(args)
        .env("XDG_CONFIG_HOME", path.join(".git/config-home"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap()
}

fn fixture(conflict: bool) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-b", "main"]);
    git(p, &["config", "user.name", "Test"]);
    git(p, &["config", "user.email", "test@example.invalid"]);
    git(p, &["config", "commit.gpgsign", "false"]);
    git(p, &["commit", "--allow-empty", "-m", "root"]);
    for name in ["a", "b", "c"] {
        let file = if conflict { "file" } else { name };
        std::fs::write(p.join(file), format!("{name}\n")).unwrap();
        git(p, &["add", "."]);
        let out = sk(p, &["create", name, "-m", name]);
        assert!(out.status.success(), "{out:?}");
    }
    dir
}

fn reorder(p: &Path, text: &str, whole: bool, fail: bool) -> Output {
    let editor = p.join(".git/editor.sh");
    std::fs::write(
        &editor,
        concat!(
            "cat \"$1\" > \"$SK_TEST_CAPTURE\"\n",
            "printf '%s\\n' \"$SK_TEST_ORDER\" > \"$1\"\n",
            "exit \"$SK_TEST_EXIT\"\n",
        ),
    )
    .unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_sk"));
    command.current_dir(p).arg("reorder");
    if whole {
        command.arg("--stack");
    }
    command
        .env("GIT_EDITOR", format!("sh '{}'", editor.display()))
        .env("SK_TEST_ORDER", text)
        .env("SK_TEST_CAPTURE", p.join(".git/capture"))
        .env("SK_TEST_EXIT", if fail { "1" } else { "0" })
        .env("XDG_CONFIG_HOME", p.join(".git/config-home"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap()
}

#[tokio::test]
async fn default_reorders_ancestors_and_restacks_descendants() {
    let dir = fixture(false);
    let p = dir.path();
    git(p, &["switch", "b"]);
    let out = reorder(p, "a\nb\n# main", false, false);
    assert!(out.status.success(), "{out:?}");
    let editor = std::fs::read_to_string(p.join(".git/capture")).unwrap();
    assert!(editor.starts_with("b\na\n# main"));
    assert_eq!(git(p, &["branch", "--show-current"]), "b");
    assert_eq!(git(p, &["log", "--format=%s", "main..c"]), "c\na\nb");
    assert_eq!(git(p, &["ls-tree", "--name-only", "b"]), "b");
    let state = Repository::at(p).await.unwrap().load().await.unwrap();
    let branches = &state.stacks[0].branches;
    assert_eq!(
        branches.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
        ["b", "a", "c"]
    );
    assert_eq!(branches[1].base, git(p, &["rev-parse", "b"]));
    assert_eq!(branches[2].base, git(p, &["rev-parse", "a"]));
}

#[test]
fn stack_flag_includes_descendants_and_noop_leaves_refs_unchanged() {
    let dir = fixture(false);
    let p = dir.path();
    git(p, &["switch", "a"]);
    let before = git(p, &["show-ref"]);
    assert!(reorder(p, "c\nb\na", true, false).status.success());
    assert_eq!(git(p, &["show-ref"]), before);
    let out = reorder(p, "a\nb\nc", true, false);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(git(p, &["log", "--format=%s", "main..a"]), "a\nb\nc");
    assert_eq!(git(p, &["branch", "--show-current"]), "a");
}

#[test]
fn invalid_edits_and_editor_failure_leave_refs_and_state_unchanged() {
    let dir = fixture(false);
    let p = dir.path();
    let refs = git(p, &["show-ref"]);
    let state = std::fs::read(p.join(".git/sk/state.json")).unwrap();
    for text in ["", "a\na\nc", "a\nc", "a\nb\nunknown", "main\na\nb\nc"] {
        assert!(!reorder(p, text, true, false).status.success());
        assert_eq!(git(p, &["show-ref"]), refs);
        assert_eq!(std::fs::read(p.join(".git/sk/state.json")).unwrap(), state);
        assert!(!p.join(".git/sk/journal.json").exists());
    }
    assert!(!reorder(p, "a\nb\nc", true, true).status.success());
    assert_eq!(git(p, &["show-ref"]), refs);
}

#[test]
fn conflicts_can_abort_or_continue_with_reordered_metadata() {
    for abort in [true, false] {
        let dir = fixture(true);
        let p = dir.path();
        let refs = git(p, &["show-ref"]);
        let state = std::fs::read(p.join(".git/sk/state.json")).unwrap();
        let out = reorder(p, "c\na\nb", true, false);
        assert!(!out.status.success(), "{out:?}");
        assert!(p.join(".git/sk/journal.json").exists());
        if abort {
            let out = sk(p, &["abort"]);
            assert!(out.status.success(), "{out:?}");
            assert_eq!(git(p, &["show-ref"]), refs);
            assert_eq!(
                std::fs::read(p.join(".git/sk/state.json")).unwrap(),
                state
            );
        } else {
            for _ in 0..3 {
                if !p.join(".git/sk/journal.json").exists() {
                    break;
                }
                std::fs::write(p.join("file"), "resolved\n").unwrap();
                git(p, &["add", "file"]);
                sk(p, &["continue"]);
            }
            assert!(!p.join(".git/sk/journal.json").exists());
            let state: sk::core::State = serde_json::from_slice(
                &std::fs::read(p.join(".git/sk/state.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(state.stacks[0].branches[0].name, "b");
        }
        assert_eq!(git(p, &["branch", "--show-current"]), "c");
        assert!(git(p, &["status", "--porcelain"]).is_empty());
    }
}

#[test]
fn dirty_checkout_and_other_worktrees_block_before_editor() {
    let dir = fixture(false);
    let p = dir.path();
    let refs = git(p, &["show-ref"]);
    std::fs::write(p.join("untracked"), "keep me").unwrap();
    let out = reorder(p, "a\nb\nc", true, false);
    assert!(!out.status.success());
    assert!(!p.join(".git/capture").exists());
    assert_eq!(
        std::fs::read_to_string(p.join("untracked")).unwrap(),
        "keep me"
    );
    std::fs::remove_file(p.join("untracked")).unwrap();
    let other = tempfile::tempdir().unwrap();
    git(p, &["worktree", "add", other.path().to_str().unwrap(), "a"]);
    let out = reorder(p, "a\nb\nc", true, false);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("checked out"));
    assert!(!p.join(".git/capture").exists());
    assert_eq!(git(p, &["show-ref"]), refs);
}

#[test]
fn external_ref_change_during_edit_is_preserved_and_rejected() {
    let dir = fixture(false);
    let p = dir.path();
    let editor = p.join(".git/change-ref.sh");
    std::fs::write(
        &editor,
        concat!(
            "git update-ref refs/heads/a main\n",
            "printf 'a\\nb\\nc\\n' > \"$1\"\n",
        ),
    )
    .unwrap();
    let state = std::fs::read(p.join(".git/sk/state.json")).unwrap();
    let b = git(p, &["rev-parse", "b"]);
    let out = Command::new(env!("CARGO_BIN_EXE_sk"))
        .current_dir(p)
        .args(["reorder", "--stack"])
        .env("GIT_EDITOR", format!("sh '{}'", editor.display()))
        .env("XDG_CONFIG_HOME", p.join(".git/config-home"))
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("changed"));
    assert_eq!(git(p, &["rev-parse", "a"]), git(p, &["rev-parse", "main"]));
    assert_eq!(git(p, &["rev-parse", "b"]), b);
    assert_eq!(std::fs::read(p.join(".git/sk/state.json")).unwrap(), state);
    assert!(!p.join(".git/sk/journal.json").exists());
}
