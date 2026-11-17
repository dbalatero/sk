use sk::ui;

#[test]
fn fuzzy_matches_noncontiguous_case_insensitive_characters() {
    assert!(ui::fuzzy_score("fat", "Feat-auth").is_some());
    assert!(ui::fuzzy_score("xyz", "feat-auth").is_none());
    assert!(
        ui::fuzzy_score("auth", "auth").unwrap()
            < ui::fuzzy_score("auth", "a-long-u-long-t-long-h").unwrap()
    );
}

#[test]
fn pr_identity_matches_subpages_but_not_other_numbers_or_hosts() {
    let base = "https://github.com/owner/repo/pull/12";
    assert!(ui::same_pr(base, &format!("{base}/files#diff-abc")));
    assert!(ui::same_pr(base, &format!("{base}?x=1")));
    assert!(!ui::same_pr(base, "https://github.com/owner/repo/pull/123"));
    assert!(!ui::same_pr(base, "https://evil.test/owner/repo/pull/12"));
    assert!(!ui::same_pr(base, "https://github.com/owner/other/pull/12"));
    assert!(!ui::same_pr("", ""));
}

#[test]
fn navigation_clamps_at_stack_boundaries() {
    let branches = vec!["a".into(), "b".into()];
    assert_eq!(
        ui::navigation_target(&branches, "main", "a", true, 1),
        Some("b".into())
    );
    assert_eq!(
        ui::navigation_target(&branches, "main", "a", false, 4),
        Some("main".into())
    );
    assert_eq!(
        ui::navigation_target(&branches, "main", "main", true, 8),
        Some("b".into())
    );
    assert_eq!(
        ui::navigation_target(&branches, "main", "unknown", true, 1),
        None
    );
}

#[test]
fn compact_view_marks_current_pr_and_stale_ancestry() {
    let stack = ui::ViewStack {
        trunk: "main".into(),
        branches: vec![
            ui::ViewBranch {
                name: "auth".into(),
                pr: Some(12),
                state: "OPEN".into(),
                needs_restack: false,
            },
            ui::ViewBranch {
                name: "api".into(),
                pr: None,
                state: String::new(),
                needs_restack: true,
            },
        ],
    };
    let view = ui::render_stacks(&[stack], "api", false);
    assert!(view.contains("● api"));
    assert!(view.contains("#12"));
    assert!(view.contains("needs restack"));
    assert!(view.find("api").unwrap() < view.find("auth").unwrap());
    assert!(view.contains("main"));
    assert!(!view.contains('\u{1b}'));
}
