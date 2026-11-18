use super::*;

pub(super) fn phase(message: &str) {
    let color = submit_ui::color(std::io::stdout().is_terminal());
    println!("{}", submit_ui::paint(message, "94", color));
    let _ = std::io::stdout().flush();
}

pub(super) fn report(
    original: &Stack,
    proposed: &Stack,
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
    color: bool,
) -> Vec<String> {
    let paint = |text: &str, code| submit_ui::paint(text, code, color);
    let mut lines = Vec::new();
    let trunk = paint(&proposed.trunk, "36");
    if before.get(&proposed.trunk) == after.get(&proposed.trunk) {
        lines.push(format!("Trunk {trunk} is up to date with remote."));
    } else {
        lines.push(format!("Updated trunk {trunk} from remote."));
    }
    for branch in &original.branches {
        if proposed.branches.iter().any(|b| b.name == branch.name) {
            continue;
        }
        let name = paint(&branch.name, "31");
        let info = branch
            .pr
            .as_ref()
            .map(|pr| {
                format!(" (PR #{}, {})", pr.number, pr.state.to_lowercase())
            })
            .unwrap_or_default();
        lines.push(format!("Deleted branch {name}{info}."));
    }
    let mut parent = &proposed.trunk;
    for branch in &proposed.branches {
        let name = paint(&branch.name, "32");
        let pr = branch
            .pr
            .as_ref()
            .map(|pr| {
                format!(" ({})", paint(&format!("PR #{}", pr.number), "36"))
            })
            .unwrap_or_default();
        let old = before.get(&branch.name);
        let new = after.get(&branch.name);
        if old != new {
            if branch.fetched_oid.as_ref() == new {
                lines.push(format!("Synced {name}{pr} from remote."));
            } else {
                lines.push(format!(
                    "Restacked {name} on {}.",
                    paint(parent, "36")
                ));
            }
        } else if new == branch.fetched_oid.as_ref() {
            lines.push(format!("Branch {name} is up to date with remote{pr}."));
        } else if branch.fetched_oid.is_none() {
            lines.push(format!(
                "Branch {name} has no remote branch; kept local changes."
            ));
        } else {
            lines.push(format!(
                "Branch {name}{pr} has local changes not yet submitted."
            ));
        }
        parent = &branch.name;
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_report_distinguishes_noop_remote_update_and_restack() {
        let branch = core::Branch {
            name: "feature".into(),
            base: "root".into(),
            remote_oid: Some("old".into()),
            fetched_oid: Some("old".into()),
            pr: Some(PullRequest {
                number: 7,
                url: "url".into(),
                state: "OPEN".into(),
                draft: false,
                head_oid: "old".into(),
            }),
        };
        let mut stack = Stack {
            id: "s".into(),
            trunk: "main".into(),
            remote: "origin".into(),
            branches: vec![branch],
            github_number: None,
        };
        let before = BTreeMap::from([
            ("main".into(), "root".into()),
            ("feature".into(), "old".into()),
        ]);
        let mut after = before.clone();
        let text = report(&stack, &stack, &before, &after, false).join("\n");
        assert!(
            text.contains("Branch feature is up to date with remote (PR #7).")
        );
        assert!(!text.contains('\x1b'));
        after.insert("feature".into(), "new".into());
        stack.branches[0].fetched_oid = Some("new".into());
        assert!(
            report(&stack, &stack, &before, &after, false)
                .join("\n")
                .contains("Synced feature (PR #7) from remote.")
        );
        after.insert("feature".into(), "replayed".into());
        assert!(
            report(&stack, &stack, &before, &after, false)
                .join("\n")
                .contains("Restacked feature on main.")
        );
        let text = report(&stack, &stack, &before, &after, true).join("\n");
        assert!(text.contains("\x1b[32mfeature\x1b[0m"));
        let mut empty = stack.clone();
        empty.branches.clear();
        stack.branches[0].pr.as_mut().unwrap().state = "MERGED".into();
        assert!(
            report(&stack, &empty, &before, &after, false)
                .join("\n")
                .contains("Deleted branch feature (PR #7, merged).")
        );
        stack.branches[0].fetched_oid = None;
        assert!(
            report(&stack, &stack, &before, &before, false)
                .join("\n")
                .contains("has no remote branch; kept local changes.")
        );
    }
}
