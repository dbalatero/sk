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
            "create [name] [-m <message>] [-a]",
            "Create a child branch from the stack tip or trunk.\n\
             Without a name, generate a date-prefixed name from the message.\n\
             Commit staged changes, or create an empty named branch.\n\
             This command stays local.",
            "  -m, --message <message>  Set the commit message\n\
               -a, --all                Stage all changes, including untracked\n\
               -am <message>            Stage all changes and set the message",
        ),
        "submit" | "ss" => (
            "submit [-d] [-n] [-s]",
            "Push ancestors through the current branch and create/update PRs.\n\
             Alias: s. Use ss to include descendants (--stack).\n\
             New PRs open in $EDITOR unless -n is supplied.\n\
             Existing PR titles, descriptions and draft status are preserved.",
            "  -d, --draft    Create new PRs as drafts\n\
               -n, --no-edit  Generate PR metadata without opening an editor\n\
               -s, --stack    Include descendants (whole stack)\n\
               --no-stack    Submit only through the current branch\n\n\
             Example: sk ss -dn",
        ),
        "merge" => (
            "merge [--include-current | --stack] [--dry-run]",
            "Merge open PRs below the current branch into trunk. The current\n\
             branch is excluded by default. Already merged PRs are skipped.\n\
             Draft, closed, unpublished, or changed PRs block the operation.\n\
             Uses GitHub's async stack merge API and honors merge queues/rules.\n\
             Run sync afterward to refresh local state; no local refs change.",
            "  --include-current  Also merge the current branch\n\
               --stack            Merge the entire stack\n\
               --dry-run          Fetch and show the selection without merging\n\
               --squash           Squash each PR\n\
               --rebase           Rebase merge\n\
               --merge            Merge commits\n\
             Without a method flag, GitHub chooses its default. Merge queues\n\
             control their own method. No branch-protection bypass is used.",
        ),
        "sync" => (
            "sync",
            "Fetch and refresh all tracked stacks, restack locally and prune\n\
             eligible merged or closed branches. Requires a clean checkout.\n\
             Conflicting stacks remain unchanged; use restack to resolve them.\n\
             Sync accesses GitHub but never pushes or changes remote PRs.",
            "",
        ),
        "reorder" => (
            "reorder [--stack]",
            "Open an editor to reorder branches from trunk through the current\n\
             branch, then restack descendants. Lines run from tip to trunk.\n\
             Keep each branch exactly once; additions/removals are unsupported.\n\
             Requires a clean checkout. Resolve conflicts with continue/abort.\n\
             Uses GIT_EDITOR, core.editor, or EDITOR. Stays offline.",
            "  --stack  Include all descendants in the editor",
        ),
        "restack" => (
            "restack",
            "Restack the current stack against local parents. Requires a clean\n\
             checkout. Resolve cached divergence interactively without fetching.\n\
             For rebase conflicts, edit and stage files, then run sk continue.\n\
             Run sk abort to restore the original stack. This command is offline.",
            "",
        ),
        "continue" => (
            "continue",
            "Resume an interrupted local operation. After resolving rebase\n\
             conflicts, stage the resolution with git add first. Stays offline.",
            "",
        ),
        "abort" => (
            "abort",
            "Abort the interrupted local operation and restore its original\n\
             stack and checkout. External changes may require manual attention.\n\
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
                "Display all tracked stacks, current branch, cached PR state\n\
                 and local restack indicators. Alias: ls. No network requests.",
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
            "Open the cached PR for a branch (default: current branch) in Chrome.\n\
             On macOS, focus an existing tab for that PR before opening a new one.\n\
             No GitHub API lookup is performed; sync or submit refreshes the cache.",
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
            "-m" | "--message" | "-am" | "-ma" if command == "create" => {
                args.next();
            }
            "--help" | "-h" => return true,
            _ => {}
        }
    }
    false
}
