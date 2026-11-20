use super::*;

pub(super) async fn run(repo: &Repository, args: &[String]) -> Result<()> {
    repo.ensure_idle().await?;
    repo.clean().await?;
    let mut source = repo.current().await?;
    let mut onto = None;
    let mut only = false;
    let mut all = false;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--onto" | "-o" => {
                onto =
                    Some(args.next().context("--onto needs a branch")?.clone())
            }
            "--source" | "-s" => {
                source = args.next().context("--source needs a branch")?.clone()
            }
            "--only" => only = true,
            "--all" | "-a" => all = true,
            _ => bail!("Unknown move argument {arg}"),
        }
    }
    let original = repo.load().await?;
    let source_stack = original
        .stacks
        .iter()
        .find(|stack| stack.branches.iter().any(|branch| branch.name == source))
        .context("Move source must be a tracked branch")?;
    let onto = match onto {
        Some(onto) => onto,
        None => {
            let mut names = std::collections::BTreeSet::new();
            for stack in &original.stacks {
                if !all && stack.trunk != source_stack.trunk {
                    continue;
                }
                names.insert(stack.trunk.clone());
                names.extend(stack.branches.iter().map(|b| b.name.clone()));
            }
            let choices = names
                .into_iter()
                .filter(|name| reshape(&original, &source, name, only).is_ok())
                .map(|name| (name.clone(), name))
                .collect();
            match crate::ui::pick("Move onto a branch", choices).await? {
                Some(onto) => onto,
                None => {
                    println!("Move cancelled.");
                    return Ok(());
                }
            }
        }
    };
    let (proposed, indices) = reshape(&original, &source, &onto, only)?;
    if indices.is_empty() {
        println!("{source} is already based on {onto}.");
        return Ok(());
    }
    for index in &indices {
        foreign_worktrees(repo, &proposed.stacks[*index]).await?;
    }
    snapshot_state(repo, &original).await?;
    let mut heads = BTreeMap::new();
    for index in &indices {
        match plan_stack_with_progress(
            repo,
            &proposed.stacks[*index],
            &heads,
            true,
        )
        .await
        {
            Ok(plan) => heads.extend(plan),
            Err(error) => {
                if error.downcast_ref::<ReplayConflict>().is_none() {
                    return Err(error);
                }
                eprintln!("{error}; starting interactive rebase");
                return begin_rebase_stacks(
                    repo,
                    original,
                    proposed,
                    indices,
                    vec![],
                )
                .await;
            }
        }
    }
    apply_plan(repo, &original, &proposed, &heads).await?;
    println!("Moved {source} onto {onto} and restacked affected branches.");
    Ok(())
}

fn reshape(
    original: &State,
    source: &str,
    onto: &str,
    only: bool,
) -> Result<(State, Vec<usize>)> {
    let source_index = original
        .stacks
        .iter()
        .position(|s| s.branches.iter().any(|b| b.name == source))
        .context("Move source must be a tracked branch")?;
    let stack = &original.stacks[source_index];
    let position = stack
        .branches
        .iter()
        .position(|b| b.name == source)
        .unwrap();
    let parent = if position == 0 {
        &stack.trunk
    } else {
        &stack.branches[position - 1].name
    };
    if onto == source {
        bail!("Cannot move a branch onto itself");
    }
    if onto == parent {
        return Ok((original.clone(), vec![]));
    }
    let end = if only {
        position + 1
    } else {
        stack.branches.len()
    };
    let moved = stack.branches[position..end].to_vec();
    if moved.iter().any(|b| b.name == onto) {
        bail!("Cannot move a branch onto its moved descendants");
    }
    let mut proposed = original.clone();
    proposed.stacks[source_index].branches.drain(position..end);
    let target = proposed
        .stacks
        .iter()
        .position(|s| s.branches.iter().any(|b| b.name == onto));
    let mut affected = vec![source_index];
    if let Some(target) = target {
        let destination = &mut proposed.stacks[target];
        if destination.branches.last().map(|b| b.name.as_str()) != Some(onto) {
            bail!(
                "Destination must be a stack tip; sibling branches are unsupported"
            );
        }
        if destination.remote != stack.remote {
            bail!("Cannot move between stacks with different remotes");
        }
        destination.branches.extend(moved);
        if target != source_index {
            affected.push(target);
        }
    } else {
        if !original.stacks.iter().any(|s| s.trunk == onto) {
            bail!("Destination must be a tracked branch or known trunk");
        }
        affected.push(proposed.stacks.len());
        proposed.stacks.push(Stack {
            id: source.into(),
            trunk: onto.into(),
            remote: stack.remote.clone(),
            branches: moved,
            github_number: None,
        });
    }
    for index in &affected {
        let stack = &mut proposed.stacks[*index];
        stack.github_number = None;
        if let Some(first) = stack.branches.first() {
            stack.id = first.name.clone();
        }
    }
    let mut indices = Vec::new();
    let mut stacks = Vec::new();
    for (index, stack) in proposed.stacks.into_iter().enumerate() {
        if stack.branches.is_empty() {
            continue;
        }
        if affected.contains(&index) {
            indices.push(stacks.len());
        }
        stacks.push(stack);
    }
    proposed.stacks = stacks;
    Ok((proposed, indices))
}
