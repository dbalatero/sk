use super::*;
use std::collections::BTreeSet;

pub(super) async fn modify(repo: &Repository, args: &[String]) -> Result<()> {
    repo.ensure_idle().await?;
    let mut all = false;
    let mut new_commit = false;
    let mut edit = false;
    let mut message = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--all" => all = true,
            "--commit" => new_commit = true,
            "--edit" => edit = true,
            "--message" => {
                message = Some(args.next().context("message required")?.clone())
            }
            arg if arg.starts_with('-') && !arg.starts_with("--") => {
                let mut flags = arg[1..].chars();
                while let Some(flag) = flags.next() {
                    match flag {
                        'a' => all = true,
                        'c' => new_commit = true,
                        'e' => edit = true,
                        'm' => {
                            let rest = flags.as_str();
                            message = Some(if rest.is_empty() {
                                args.next()
                                    .context("-m needs a message")?
                                    .clone()
                            } else {
                                rest.into()
                            });
                            break;
                        }
                        _ => bail!("unsupported modify flag -{flag}"),
                    }
                }
            }
            _ => bail!("unsupported modify argument {arg}"),
        }
    }
    let state = repo.load().await?;
    let current = repo.current().await?;
    let stack = state
        .stacks
        .iter()
        .find(|s| s.branches.iter().any(|b| b.name == current))
        .context("Check out a tracked branch before modifying")?;
    let position = stack
        .branches
        .iter()
        .position(|b| b.name == current)
        .unwrap();
    let branch = &stack.branches[position];
    let head = repo.git(&["rev-parse", "HEAD"]).await?.trim().to_owned();
    repo.git(&["merge-base", "--is-ancestor", &branch.base, &head])
        .await
        .context("branch history changed; run sk restack first")?;
    new_commit |= head == branch.base;
    if new_commit && message.is_none() && !edit {
        bail!("new commits need -m <message> or -e to open the editor");
    }
    let has_descendants = position + 1 < stack.branches.len();
    if has_descendants {
        foreign_worktrees(repo, stack).await?;
        if !all {
            let unstaged = repo.git(&["diff", "--name-only"]).await?;
            let untracked = repo
                .git(&["ls-files", "--others", "--exclude-standard"])
                .await?;
            if !unstaged.is_empty() || !untracked.is_empty() {
                bail!(
                    "Automatic restack needs a clean checkout after commit; \
                     stage or stash remaining changes, or use -a"
                );
            }
        }
    }
    if all {
        repo.git(&["add", "--all"]).await?;
    }
    let staged = !repo
        .git(&["diff", "--cached", "--name-only"])
        .await?
        .is_empty();
    if !staged && message.is_none() && !edit {
        bail!("No staged changes; stage changes or pass -a");
    }
    let mut command = Command::new("git");
    command.current_dir(&repo.root).arg("commit");
    if !new_commit {
        command.arg("--amend");
    }
    if let Some(message) = message {
        command.args(["-m", &message]);
    } else if !edit {
        command.arg("--no-edit");
    }
    if edit {
        command.arg("--edit");
    }
    if !command.status().await?.success() {
        bail!("Commit failed; staged changes are retained");
    }
    println!("Modified {current}.");
    if has_descendants {
        println!("Restacking descendants...");
        restack(repo).await.context(
            "Modification committed; automatic restack did not finish",
        )?;
    }
    Ok(())
}

async fn local_refs(repo: &Repository) -> Result<BTreeMap<String, String>> {
    let refs = repo
        .git(&[
            "for-each-ref",
            "--format=%(refname:short) %(objectname)",
            "refs/heads/",
        ])
        .await?;
    Ok(refs
        .lines()
        .filter_map(|l| l.split_once(' '))
        .map(|(name, oid)| (name.into(), oid.into()))
        .collect())
}

async fn trunk(
    repo: &Repository,
    refs: &BTreeMap<String, String>,
) -> Result<String> {
    if let Ok(name) = repo.git(&["config", "--get", "sk.trunk"]).await {
        let name = name.trim();
        if !refs.contains_key(name) {
            bail!("configured trunk {name} does not exist");
        }
        return Ok(name.into());
    }
    if let Ok(name) = repo
        .git(&["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .await
        && let Some((_, name)) = name.trim().split_once('/')
        && refs.contains_key(name)
    {
        return Ok(name.into());
    }
    let candidates: Vec<_> = ["main", "master"]
        .into_iter()
        .filter(|name| refs.contains_key(*name))
        .collect();
    match candidates.as_slice() {
        [name] => Ok((*name).into()),
        _ => bail!("set git config sk.trunk <branch> to identify trunk"),
    }
}

pub(super) async fn track(repo: &Repository, args: &[String]) -> Result<()> {
    repo.ensure_idle().await?;
    let mut name = None;
    let mut parent = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-p" | "--parent" => {
                parent = Some(args.next().context("parent required")?.clone())
            }
            arg if arg.starts_with('-') => {
                bail!("unsupported track flag {arg}")
            }
            arg => {
                if name.replace(arg.to_owned()).is_some() {
                    bail!("one branch name allowed");
                }
            }
        }
    }
    let target = match name {
        Some(name) => name,
        None => repo.current().await?,
    };
    let mut state = repo.load().await?;
    if let Some(stack) = state
        .stacks
        .iter()
        .find(|s| s.branches.iter().any(|b| b.name == target))
    {
        let pos = stack
            .branches
            .iter()
            .position(|b| b.name == target)
            .unwrap();
        let old_parent = if pos == 0 {
            &stack.trunk
        } else {
            &stack.branches[pos - 1].name
        };
        if parent.as_ref().is_some_and(|p| p != old_parent) {
            bail!("branch already tracked; use reorder to change its parent");
        }
        println!("Already tracking {target}.");
        return Ok(());
    }
    let refs = local_refs(repo).await?;
    let tracked_parent = parent.as_ref().and_then(|p| {
        state
            .stacks
            .iter()
            .find(|s| s.branches.iter().any(|b| &b.name == p))
    });
    let trunk = match tracked_parent {
        Some(stack) => stack.trunk.clone(),
        None => trunk(repo, &refs).await?,
    };
    if target == trunk {
        bail!("trunk does not need tracking");
    }
    let mut chain = Vec::new();
    let mut seen = BTreeSet::new();
    let mut cursor = target.clone();
    loop {
        if cursor == trunk
            || state
                .stacks
                .iter()
                .any(|s| s.branches.iter().any(|b| b.name == cursor))
        {
            break;
        }
        if !seen.insert(cursor.clone()) {
            bail!("ambiguous parent cycle; specify --parent");
        }
        let oid = refs
            .get(&cursor)
            .with_context(|| format!("no local branch {cursor}"))?;
        let next = if let Some(parent) = parent.take() {
            parent
        } else {
            let history =
                repo.git(&["rev-list", "--first-parent", oid]).await?;
            let mut found = None;
            for commit in history.lines() {
                let candidates: Vec<_> = refs
                    .iter()
                    .filter(|(name, sha)| {
                        !seen.contains(*name) && sha.as_str() == commit
                    })
                    .map(|(name, _)| name.clone())
                    .collect();
                match candidates.as_slice() {
                    [] => continue,
                    [name] => {
                        found = Some(name.clone());
                        break;
                    }
                    _ => {
                        bail!("ambiguous parent for {cursor}; specify --parent")
                    }
                }
            }
            found.with_context(|| {
                format!("no parent for {cursor}; specify --parent")
            })?
        };
        let base = refs
            .get(&next)
            .with_context(|| format!("no local parent {next}"))?;
        repo.git(&["merge-base", "--is-ancestor", base, oid])
            .await
            .context("parent must be an ancestor of the tracked branch")?;
        chain.push(Branch {
            name: cursor,
            base: base.clone(),
            remote_oid: None,
            fetched_oid: None,
            pr: None,
        });
        cursor = next;
    }
    chain.reverse();
    if cursor == trunk {
        state.stacks.push(Stack {
            id: chain[0].name.clone(),
            trunk,
            remote: repo
                .git(&["config", "--get", "sk.remote"])
                .await
                .unwrap_or_else(|_| "origin".into())
                .trim()
                .into(),
            github_number: None,
            branches: chain,
        });
    } else {
        let stack = state
            .stacks
            .iter_mut()
            .find(|s| s.branches.iter().any(|b| b.name == cursor))
            .unwrap();
        if stack.branches.last().map(|b| &b.name) != Some(&cursor) {
            bail!("parent must be a stack tip; siblings are unsupported");
        }
        stack.branches.extend(chain);
    }
    if local_refs(repo).await? != refs {
        bail!("local branches changed while tracking; retry");
    }
    repo.save(&state).await?;
    println!("Tracked {target}.");
    Ok(())
}

pub(super) async fn split(repo: &Repository, args: &[String]) -> Result<()> {
    let mut each = false;
    for arg in args {
        match arg.as_str() {
            "--by-commit" | "-c" | "--commit" => {}
            "--each" => each = true,
            _ => bail!("usage: sk split --by-commit [--each]"),
        }
    }
    repo.ensure_idle().await?;
    repo.clean().await?;
    let original = repo.load().await?;
    let current = repo.current().await?;
    let index = original
        .stacks
        .iter()
        .position(|s| s.branches.iter().any(|b| b.name == current))
        .context("Check out a tracked branch before splitting")?;
    let stack = &original.stacks[index];
    foreign_worktrees(repo, stack).await?;
    let position = stack
        .branches
        .iter()
        .position(|b| b.name == current)
        .unwrap();
    let branch = &stack.branches[position];
    let tip = repo.git(&["rev-parse", "HEAD"]).await?.trim().to_owned();
    repo.git(&["merge-base", "--is-ancestor", &branch.base, &tip])
        .await
        .context("run sk restack before splitting changed history")?;
    let range = format!("{}..{tip}", branch.base);
    if !repo
        .git(&["rev-list", "--merges", &range])
        .await?
        .trim()
        .is_empty()
    {
        bail!("split requires linear commits; merge commits are unsupported");
    }
    let commits = repo.git(&["rev-list", "--reverse", &range]).await?;
    let commits: Vec<_> = commits.lines().collect();
    if commits.len() < 2 {
        bail!("branch needs at least two commits to split");
    }
    snapshot_state(repo, &original).await?;
    let mut names: BTreeSet<_> = local_refs(repo).await?.into_keys().collect();
    let mut document = String::new();
    for oid in &commits {
        let name = if *oid == tip {
            current.clone()
        } else {
            let subject = repo.git(&["show", "-s", "--format=%s", oid]).await?;
            let base = format!("{}-{}", current, slug(subject.trim()));
            let mut name = base.clone();
            let mut suffix = 2;
            while names.contains(&name) {
                name = format!("{base}-{suffix}");
                suffix += 1;
            }
            names.insert(name.clone());
            name
        };
        document.push_str(&format!("{oid} {name}\n"));
    }
    if !each {
        document.push_str(
            "# Oldest first. Each line ends a new branch.\n\
            # Delete lines to group commits; edit new branch names.\n\
            # Keep the last line (original tip and branch). Do not reorder.\n",
        );
        let editor = repo.git(&["var", "GIT_EDITOR"]).await?;
        let words = shell_words::split(editor.trim())?;
        let file = tempfile::Builder::new()
            .prefix("sk-split-")
            .suffix(".txt")
            .tempfile()?;
        tokio::fs::write(file.path(), &document).await?;
        let status = Command::new(words.first().context("editor is empty")?)
            .args(&words[1..])
            .arg(file.path())
            .current_dir(&repo.root)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .await?;
        if !status.success() {
            bail!("editor failed; split cancelled");
        }
        document = tokio::fs::read_to_string(file.path()).await?;
    }
    let refs = local_refs(repo).await?;
    let mut cuts = Vec::new();
    let mut previous = None;
    let mut used = BTreeSet::new();
    for line in document
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
    {
        let parts: Vec<_> = line.split_whitespace().collect();
        if parts.len() != 2 {
            bail!("each split line needs a full commit SHA and branch name");
        }
        let pos = commits
            .iter()
            .position(|oid| *oid == parts[0])
            .context("unknown split commit")?;
        if previous.is_some_and(|last| pos <= last) {
            bail!("split commits must stay in original order");
        }
        previous = Some(pos);
        let name = parts[1];
        repo.git(&["check-ref-format", "--branch", name]).await?;
        if !used.insert(name.to_owned()) {
            bail!("duplicate split branch {name}");
        }
        if pos == commits.len() - 1 {
            if name != current {
                bail!("keep the original branch name on the last line");
            }
        } else if refs.contains_key(name) {
            bail!("branch {name} already exists");
        }
        cuts.push((parts[0].to_owned(), name.to_owned()));
    }
    if cuts.last().map(|(oid, name)| (oid, name)) != Some((&tip, &current)) {
        bail!("keep the original tip and branch on the last line");
    }
    if cuts.len() == 1 {
        println!("No split points selected.");
        return Ok(());
    }
    if repo.current().await? != current {
        bail!("checkout changed while editing; split cancelled");
    }
    verify_snapshots(repo).await?;
    let mut proposed = original.clone();
    let mut branches = Vec::new();
    let mut heads = BTreeMap::new();
    let mut base = branch.base.clone();
    for (oid, name) in cuts {
        let mut b = if name == current {
            branch.clone()
        } else {
            repo.snapshots
                .lock()
                .map_err(|_| anyhow::anyhow!("Snapshot lock poisoned"))?
                .insert(name.clone(), None);
            Branch {
                name: name.clone(),
                base: base.clone(),
                remote_oid: None,
                fetched_oid: None,
                pr: None,
            }
        };
        b.base = base;
        base = oid.clone();
        branches.push(b);
        heads.insert(name, oid);
    }
    proposed.stacks[index]
        .branches
        .splice(position..=position, branches);
    apply_plan(repo, &original, &proposed, &heads).await?;
    println!(
        "Split {current} by commit; original tip and descendants preserved."
    );
    Ok(())
}
