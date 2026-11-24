use super::*;

pub(super) async fn run(args: &[String]) -> Result<()> {
    let force = match args {
        [] => false,
        [flag] if flag == "--force" || flag == "-f" => true,
        _ => bail!("usage: sk delete [-f|--force]"),
    };
    let repo = Repository::discover().await?;
    let _lock = repo.lock()?;
    let plan = prepare(&repo).await?;
    let stack = &plan.original.stacks[plan.index];
    let remotes = repo.git(&["remote"]).await?;
    let client = if remotes.lines().any(|r| r == stack.remote) {
        Some(api(&repo, &stack.remote).await?)
    } else {
        if stack.branches.last().unwrap().pr.is_some()
            || stack.branches.last().unwrap().remote_oid.is_some()
        {
            bail!("Remote is missing; cannot verify the submitted branch");
        }
        None
    };
    let pull = match &client {
        Some(client) => client.find_open(&plan.branch).await?,
        None => None,
    };
    if let Some(pr) = &pull {
        println!(
            "Close PR #{} and delete local branch {}?",
            pr.number, plan.branch
        );
    } else {
        println!("Delete local branch {}?", plan.branch);
    }
    println!(
        "Its commits will be discarded from the stack; checkout {}.",
        plan.parent
    );
    if !force {
        if !std::io::stdin().is_terminal() {
            bail!(
                "Deletion requires confirmation; use --force noninteractively"
            );
        }
        eprint!("Continue? [y/N] ");
        std::io::stderr().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Delete cancelled.");
            return Ok(());
        }
    }
    execute(&repo, plan, client.as_ref(), pull.as_ref()).await
}

struct Plan {
    original: State,
    proposed: State,
    index: usize,
    branch: String,
    parent: String,
}

async fn prepare(repo: &Repository) -> Result<Plan> {
    repo.ensure_idle().await?;
    repo.clean().await?;
    let original = repo.load().await?;
    let branch = repo.current().await?;
    let index = original
        .stacks
        .iter()
        .position(|stack| stack.branches.iter().any(|b| b.name == branch))
        .context("Check out a tracked stack tip before deleting")?;
    let stack = &original.stacks[index];
    if stack.branches.last().map(|b| &b.name) != Some(&branch) {
        bail!("Only the top branch can be deleted; it still has descendants");
    }
    let parent = stack
        .branches
        .iter()
        .rev()
        .nth(1)
        .map(|b| b.name.clone())
        .unwrap_or_else(|| stack.trunk.clone());
    let listing = repo.git(&["worktree", "list", "--porcelain"]).await?;
    let mut path = "";
    for line in listing.lines() {
        if let Some(value) = line.strip_prefix("worktree ") {
            path = value;
        }
        if let Some(name) = line.strip_prefix("branch refs/heads/")
            && std::path::Path::new(path) != repo.root
            && (name == parent || name == branch)
        {
            bail!("{name} is checked out in {path}");
        }
    }
    core::snapshot_state(repo, &original).await?;
    let mut proposed = original.clone();
    proposed.stacks[index].branches.pop();
    proposed.stacks.retain(|stack| !stack.branches.is_empty());
    Ok(Plan {
        original,
        proposed,
        index,
        branch,
        parent,
    })
}

async fn execute(
    repo: &Repository,
    plan: Plan,
    client: Option<&GitHub>,
    pull: Option<&Pull>,
) -> Result<()> {
    repo.ensure_idle().await?;
    repo.clean().await?;
    if repo.current().await? != plan.branch {
        bail!("Checkout changed; delete cancelled");
    }
    core::verify_snapshots(repo).await?;
    if let Some(pr) = pull {
        let client = client.context("Missing GitHub client")?;
        let response = GitHub::response(
            client
                .request(Method::PATCH, &format!("pulls/{}", pr.number))
                .json(&json!({"state":"closed"})),
        )
        .await?
        .context("PR unavailable; local branch was not deleted")?;
        let closed: Pull = serde_json::from_value(response)?;
        if closed.number != pr.number || closed.state != "closed" {
            bail!("GitHub did not confirm PR closure; local branch retained");
        }
        println!("Closed PR #{}.", pr.number);
    }
    core::apply_plan_with_checkout(
        repo, &plan.original, &plan.proposed, &BTreeMap::new(),
        Some(&plan.parent),
    ).await.context(
        "Local deletion did not finish; any confirmed PR closure remains in effect",
    )?;
    println!("Deleted {}. Checked out {}.", plan.branch, plan.parent);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::tests::{add_commit, fixture, mock_sequence};
    use super::*;

    #[tokio::test]
    async fn detects_uncached_pr_and_deletes_only_after_confirmed_closure() {
        for response in ["closed", "open", "forbidden"] {
            let (_dir, repo) = fixture().await;
            let root = repo.git(&["rev-parse", "HEAD"]).await.unwrap();
            repo.git(&["switch", "-c", "feature"]).await.unwrap();
            let head = add_commit(&repo, "work", "work").await;
            let original = State {
                version: 1,
                stacks: vec![Stack {
                    id: "feature".into(),
                    trunk: "main".into(),
                    remote: "origin".into(),
                    github_number: None,
                    branches: vec![core::Branch {
                        name: "feature".into(),
                        base: root.trim().into(),
                        pr: None,
                        remote_oid: None,
                        fetched_oid: None,
                    }],
                }],
            };
            repo.save(&original).await.unwrap();
            let pull = json!({"number":7,"html_url":"url","state":"open",
                "draft":true,"merged_at":null,
                "head":{"sha":head,"ref":"feature"}});
            let mut closed = pull.clone();
            closed["state"] = json!(response);
            let (base, server) = mock_sequence(vec![
                ("GET /repos/o/r/pulls?", 200, json!([pull])),
                (
                    "PATCH /repos/o/r/pulls/7 ",
                    if response == "forbidden" { 403 } else { 200 },
                    closed,
                ),
            ])
            .await;
            let client = GitHub::new(&base, "o/r", "test".into());
            let plan = prepare(&repo).await.unwrap();
            let pull = client.find_open("feature").await.unwrap().unwrap();
            let result = execute(&repo, plan, Some(&client), Some(&pull)).await;
            if response == "closed" {
                result.unwrap();
                assert_eq!(repo.current().await.unwrap(), "main");
                assert!(repo.load().await.unwrap().stacks.is_empty());
                assert!(
                    repo.git(&["show-ref", "--verify", "refs/heads/feature"])
                        .await
                        .is_err()
                );
            } else {
                assert!(result.is_err());
                assert_eq!(repo.current().await.unwrap(), "feature");
                assert_eq!(
                    repo.git(&["rev-parse", "feature"]).await.unwrap().trim(),
                    head
                );
                assert_eq!(
                    serde_json::to_value(repo.load().await.unwrap()).unwrap(),
                    serde_json::to_value(original).unwrap()
                );
            }
            server.await.unwrap();
        }
    }
}
