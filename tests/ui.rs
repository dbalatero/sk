use sk::ui;

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
fn compact_view_matches_graphite_single_row_layout() {
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
    assert_eq!(view, "◉  api (needs restack)\n◯  auth\n◯  main\n");
    assert!(!view.contains("#12"));
    assert!(!view.contains('\u{1b}'));
}

fn sample_stack(prefix: &str, count: usize, trunk: &str) -> ui::ViewStack {
    ui::ViewStack {
        trunk: trunk.into(),
        branches: (1..=count)
            .map(|i| ui::ViewBranch {
                name: format!("{prefix}-{i}"),
                pr: None,
                state: String::new(),
                needs_restack: false,
            })
            .collect(),
    }
}

#[test]
fn many_stacks_match_gt_created_fixture_and_align_names() {
    let stacks = vec![
        sample_stack("gamma", 2, "main"),
        sample_stack("alpha", 3, "main"),
        sample_stack("epsilon", 1, "main"),
        sample_stack("delta", 4, "main"),
        sample_stack("beta", 1, "main"),
    ];
    let view = ui::render_stacks(&stacks, "gamma-1", false);
    assert_eq!(
        view,
        concat!(
            "◯          alpha-3\n",
            "◯          alpha-2\n",
            "◯          alpha-1\n",
            "│ ◯        beta-1\n",
            "│ │ ◯      delta-4\n",
            "│ │ ◯      delta-3\n",
            "│ │ ◯      delta-2\n",
            "│ │ ◯      delta-1\n",
            "│ │ │ ◯    epsilon-1\n",
            "│ │ │ │ ◯  gamma-2\n",
            "│ │ │ │ ◉  gamma-1\n",
            "◯─┴─┴─┴─┘  main\n",
        )
    );
    assert_eq!(view.lines().count(), 12);
    let trunk = ui::render_stacks(&stacks, "main", false);
    assert!(trunk.ends_with("◉─┴─┴─┴─┘  main\n"));
}

#[test]
fn color_does_not_change_graph_spacing() {
    let stacks =
        vec![sample_stack("a", 2, "main"), sample_stack("b", 1, "main")];
    let colored = ui::render_stacks(&stacks, "a-1", true);
    assert!(colored.contains("\x1b["));
    let mut plain = String::new();
    let mut chars = colored.chars();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            for ch in chars.by_ref() {
                if ch == 'm' {
                    break;
                }
            }
        } else {
            plain.push(ch);
        }
    }
    assert_eq!(plain, ui::render_stacks(&stacks, "a-1", false));
}

#[test]
fn multiple_trunks_keep_separate_compact_graphs() {
    let stacks = vec![
        sample_stack("a", 1, "main"),
        sample_stack("b", 1, "release"),
    ];
    assert_eq!(
        ui::render_stacks(&stacks, "release", false),
        concat!(
            "◯  a-1\n◯  main (trunk)\n\n",
            "◯  b-1\n◉  release (trunk)\n",
        )
    );
    assert_eq!(
        ui::render_stacks(&[], "main", false),
        "No tracked stacks. Start one with sk create.\n"
    );
}
