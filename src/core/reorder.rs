use super::*;
use std::collections::BTreeSet;

pub(super) async fn run(repo: &Repository, args: &[String]) -> Result<()> {
    let whole = match args {
        [] => false,
        [flag] if flag == "--stack" => true,
        _ => bail!("usage: sk reorder [--stack]"),
    };
    repo.ensure_idle().await?;
    repo.clean().await?;
    let original = repo.load().await?;
    let current = repo.current().await?;
    let index = original
        .stacks
        .iter()
        .position(|s| s.branches.iter().any(|b| b.name == current))
        .context("Check out a tracked branch before reordering")?;
    let stack = &original.stacks[index];
    foreign_worktrees(repo, stack).await?;
    let end = if whole {
        stack.branches.len()
    } else {
        stack
            .branches
            .iter()
            .position(|b| b.name == current)
            .unwrap()
            + 1
    };
    snapshot_state(repo, &original).await?;
    let mut document = String::new();
    for branch in stack.branches[..end].iter().rev() {
        document.push_str(&branch.name);
        document.push('\n');
    }
    document.push_str(&format!(
        "# {} (trunk)\n#\n# Move lines to reorder; tip first, trunk last.\n\
         # Keep every branch exactly once. Save and exit to apply.\n",
        stack.trunk
    ));
    let editor = repo
        .git(&["var", "GIT_EDITOR"])
        .await
        .context("set GIT_EDITOR, core.editor, or EDITOR for reorder")?;
    let words = shell_words::split(editor.trim())?;
    let executable = words.first().context("editor command is empty")?;
    let file = tempfile::Builder::new()
        .prefix("sk-reorder-")
        .suffix(".txt")
        .tempfile()?;
    tokio::fs::write(file.path(), document).await?;
    let status = Command::new(executable)
        .args(&words[1..])
        .arg(file.path())
        .current_dir(&repo.root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await?;
    if !status.success() {
        bail!("Editor failed; reorder cancelled");
    }
    let edited = tokio::fs::read_to_string(file.path()).await?;
    let names: Vec<_> = edited
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
        .collect();
    let expected: BTreeSet<_> = stack.branches[..end]
        .iter()
        .map(|b| b.name.as_str())
        .collect();
    let actual: BTreeSet<_> = names.iter().copied().collect();
    if names.len() != expected.len() || actual != expected {
        bail!(
            "Keep each selected branch exactly once; sk's linear stacks \
               do not support adding or removing branches in reorder"
        );
    }
    repo.ensure_idle().await?;
    repo.clean().await?;
    if repo.current().await? != current {
        bail!("Checkout changed while editing; reorder cancelled");
    }
    verify_snapshots(repo).await?;
    let mut proposed = original.clone();
    for (position, name) in names.iter().rev().enumerate() {
        proposed.stacks[index].branches[position] = stack
            .branches
            .iter()
            .find(|b| b.name == *name)
            .unwrap()
            .clone();
    }
    if proposed.stacks[index]
        .branches
        .iter()
        .map(|b| &b.name)
        .eq(stack.branches.iter().map(|b| &b.name))
    {
        println!("Branch order unchanged.");
        return Ok(());
    }
    match plan_stack(repo, &proposed.stacks[index], &BTreeMap::new()).await {
        Ok(plan) => apply_plan(repo, &original, &proposed, &plan).await?,
        Err(error) => {
            if error.downcast_ref::<ReplayConflict>().is_none() {
                return Err(error);
            }
            eprintln!("{error}; starting interactive rebase");
            begin_rebase(repo, original, proposed, index, vec![]).await?;
        }
    }
    println!("Reordered stack and restacked descendants.");
    Ok(())
}
