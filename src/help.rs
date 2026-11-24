use anyhow::{Result, bail};

/// Resolve help before repository discovery or any command side effects.
pub fn requested(args: &[String]) -> Result<bool> {
    let Some(command) = args.first() else {
        return Ok(false);
    };
    let path = if command == "help" && args.len() > 1 {
        &args[1..]
    } else if has_help_flag(command, &args[1..]) {
        args
    } else {
        return Ok(false);
    };
    let name = path[0].as_str();
    let (usage, description, options) = match name {
        "aliases" => (
            "aliases",
            "Create ~/.config/sk/aliases if missing and print its path.\n\
             Honors XDG_CONFIG_HOME. Existing aliases are never overwritten.",
            "",
        ),
        "create" => (
            "create [name] [-m <message>] [-a] [-i]",
            "Create a child branch from the stack tip or trunk.\n\
             Without a name, generate a date-prefixed name from the message.\n\
             Commit staged changes, or create an empty named branch.\n\
             Use --insert to insert above the current branch, then run\n\
             sk restack to move descendants onto it. This command stays local.",
            "  -m, --message <message>  Set the commit message\n\
               -a, --all                Stage all changes and untracked files\n\
               -i, --insert             Insert before existing descendants\n\
               -am <message>            Stage all changes and set the message",
        ),
        "modify" => (
            "modify [-a] [-c] [-m <message>] [-e]",
            "Amend the current tracked branch using staged changes. Alias: m.\n\
             Empty branches create a commit instead of amending their parent.\n\
             Automatically restack descendants afterward. Offline.\n\
             Resolve conflicts with sk continue, or undo restack with sk abort.",
            "  -a, --all      Stage all changes and untracked files\n\
               -c, --commit   Add a new commit instead of amending\n\
               -m, --message  Set the commit message\n\
               -e, --edit     Open Git's commit-message editor\n\
             Short flags combine: sk m -cam 'new commit'",
        ),
        "track" => (
            "track [branch] [-p <parent>]",
            "Adopt an existing local branch (default: current). Alias: tr.\n\
             Infer parents along first-parent history, recursively tracking\n\
             intermediate local branches. Ambiguous parents require -p.\n\
             Tracked parents must be stack tips (no siblings).\n\
             Uses sk.trunk, cached origin/HEAD, or main/master. No fetch or\n\
             checkout occurs, and uncommitted changes are preserved.",
            "  -p, --parent <branch>  Explicit parent for the selected branch",
        ),
        "split" => (
            "split --by-commit [--each]",
            "Split the current branch at commit boundaries. Alias: sp.\n\
             Opens the Git-selected editor with oldest commits first. Delete\n\
             lines to group commits; edit names for new branches. Keep the\n\
             final line's original tip and branch. Commits are not rewritten.\n\
             Requires a clean checkout and linear history. Stays offline.",
            "  -c, --commit, --by-commit  Select commit splitting (default)\n\
               --each                    One branch per commit, without editor",
        ),
        "top" => (
            "top",
            "Check out the tip of the current stack. Alias: t.\n\
             From trunk, choose a stack if needed. Stays offline.",
            "",
        ),
        "bottom" => (
            "bottom",
            "Check out the first branch above trunk in the current stack.\n\
             Alias: b. From trunk, choose a stack if needed. Stays offline.",
            "",
        ),
        "submit" | "ss" => (
            "submit [-d | -p] [-n] [-s]",
            "Push ancestors through the current branch and create/update PRs.\n\
             Alias: s. Use ss to include descendants (--stack).\n\
             New PRs prompt to edit/use the default body and publish/draft.\n\
             Editing uses $EDITOR and .github/PULL_REQUEST_TEMPLATE.md.\n\
             Existing PR metadata is preserved; -p publishes existing drafts.",
            "  -d, --draft    Create new PRs as drafts\n\
               -p, --publish Publish new PRs and mark existing drafts ready\n\
               -n, --no-edit  Use the default body without opening an editor\n\
               -s, --stack    Include descendants (whole stack)\n\
               --no-stack    Submit only through the current branch\n\n\
             Example: sk ss -dn",
        ),
        "merge" => (
            "merge [--stack] [--dry-run]",
            "Merge open PRs through the current branch into trunk, including it.\n\
             Already merged PRs are skipped.\n\
             Draft, closed, unpublished, or changed PRs block the operation.\n\
             Uses GitHub async stack merge and honors merge queues/rules.\n\
             Run sync afterward to refresh local state; no local refs change.",
            "  --include-current  Accepted for compatibility; already the default\n\
               --stack            Merge the entire stack\n\
               --dry-run          Fetch and preview without merging\n\
               --squash           Squash each PR\n\
               --rebase           Rebase merge\n\
               --merge            Merge commits\n\
             Defaults to your allowed GitHub merge preference. Merge queues\n\
             control their own method. No branch-protection bypass is used.",
        ),
        "sync" => (
            "sync",
            "Fetch and refresh all tracked stacks, restack locally and prune\n\
             eligible merged or closed branches. Requires a clean checkout.\n\
             Conflicting stacks stay unchanged; use restack to resolve.\n\
             Sync accesses GitHub but never pushes or changes remote PRs.",
            "",
        ),
        "delete" => (
            "delete [-f]",
            "Delete the current stack tip and check out its parent.\n\
             Requires a clean checkout. Confirms before discarding commits.\n\
             With a GitHub remote, checks for an open PR and closes it first.\n\
             Lookup/closure failures preserve the local branch.\n\
             The remote Git branch is retained. Trunks/descendants are protected.",
            "  -f, --force  Skip confirmation (still checks and closes any PR)",
        ),
        "move" => (
            "move [-o <branch>] [-s <branch>] [--only] [-a]",
            "Move a branch and its descendants onto a new parent. Offline.\n\
             Without --onto, choose a destination interactively.\n\
             Destinations must be stack tips or known trunks; no siblings.\n\
             Restacks affected branches and preserves the original checkout.\n\
             Resolve conflicts with sk continue, or restore with sk abort.",
            "  -o, --onto <branch>    Destination parent\n\
               -s, --source <branch>  Branch to move (default: current)\n\
               --only                 Move only this branch; reparent children\n\
               -a, --all              Show destinations across all trunks",
        ),
        "reorder" => (
            "reorder [--stack]",
            "Edit branch order from trunk through the current\n\
             branch, then restack descendants. Lines run from tip to trunk.\n\
             Keep each branch exactly once; no additions or removals.\n\
             Requires a clean checkout. Use continue/abort for conflicts.\n\
             Uses GIT_EDITOR, core.editor, or EDITOR. Stays offline.",
            "  --stack  Include all descendants in the editor",
        ),
        "restack" => (
            "restack (alias: rs)",
            "Restack against local parents. Requires a clean\n\
             checkout. Resolve cached divergence offline.\n\
             For conflicts, edit and stage files, then run sk continue.\n\
             Run sk abort to restore the original stack. Stays offline.",
            "",
        ),
        "continue" => (
            "continue",
            "Resume an interrupted local operation. After resolving rebase\n\
             conflicts, stage resolved files with git add. Stays offline.",
            "",
        ),
        "abort" => (
            "abort",
            "Abort the interrupted local operation and restore its original\n\
             stack and checkout. External edits may need manual attention.\n\
             This command stays offline.",
            "",
        ),
        "log" | "ls" => {
            if name == "log"
                && path.get(1).is_some_and(|s| {
                    !matches!(s.as_str(), "short" | "--help" | "-h")
                })
            {
                bail!("unsupported log format; use sk log short");
            }
            (
                "log short",
                "Display compact stack lanes with one row per branch\n\
                 and restack warnings. ◉ marks current. Alias: ls. Offline.",
                "",
            )
        }
        "checkout" | "co" => (
            "checkout [branch]",
            "Switch to a local branch, or open the fuzzy picker when omitted.\n\
             Alias: co. Enter selects; Escape cancels. Stays offline.\n\
             Git protects uncommitted changes that cannot be carried across.",
            "",
        ),
        "up" => (
            "up [n]",
            "Move toward the stack tip by n branches (default: 1). Stop at\n\
             the tip. From trunk, select a stack when several are available.\n\
             This command stays offline.",
            "",
        ),
        "down" => (
            "down [n]",
            "Move toward trunk by n branches (default: 1). Stop at trunk.\n\
             This command stays offline.",
            "",
        ),
        "pr" => (
            "pr [branch]",
            "Open a branch’s cached PR in Chrome (default: current branch).\n\
             On macOS, reuse an existing PR tab when possible.\n\
             No API lookup; sync or submit refreshes the cache.",
            "",
        ),
        _ => bail!("unknown command {name:?}; run sk --help"),
    };
    println!("{description}\n\nUsage: sk {usage}\n\nOptions:");
    if !options.is_empty() {
        for line in options.lines() {
            if line.starts_with('-') {
                println!("  {line}");
            } else {
                println!("{line}");
            }
        }
    }
    println!("  -h, --help  Show this command's help");
    Ok(true)
}

fn has_help_flag(command: &str, args: &[String]) -> bool {
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--" => break,
            "-m" | "--message" | "-am" | "-ma"
                if matches!(command, "create" | "modify") =>
            {
                args.next();
            }
            value
                if command == "modify"
                    && value.starts_with('-')
                    && !value.starts_with("--")
                    && value.ends_with('m') =>
            {
                args.next();
            }
            "--help" | "-h" => return true,
            _ => {}
        }
    }
    false
}
