mod reorder;
mod workflow;

use anyhow::{Context, Result, bail};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::process::Command;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub url: String,
    pub state: String,
    pub draft: bool,
    pub head_oid: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Branch {
    pub name: String,
    pub base: String,
    pub remote_oid: Option<String>,
    pub fetched_oid: Option<String>,
    pub pr: Option<PullRequest>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stack {
    pub id: String,
    pub trunk: String,
    pub remote: String,
    pub branches: Vec<Branch>,
    pub github_number: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct State {
    pub version: u32,
    pub stacks: Vec<Stack>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            version: 1,
            stacks: vec![],
        }
    }
}
#[derive(Clone, Debug)]
pub struct Repository {
    pub root: PathBuf,
    pub common: PathBuf,
    snapshots: Arc<Mutex<BTreeMap<String, Option<String>>>>,
}
pub struct RepositoryLock(File);
impl Drop for RepositoryLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}
impl Repository {
    pub async fn discover() -> Result<Self> {
        Self::at(std::env::current_dir()?).await
    }
    pub async fn at(path: impl AsRef<Path>) -> Result<Self> {
        let root =
            git_at(path.as_ref(), &["rev-parse", "--show-toplevel"]).await?;
        let common = git_at(
            path.as_ref(),
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .await?;
        Ok(Self {
            root: root.trim().into(),
            common: common.trim().into(),
            snapshots: Arc::default(),
        })
    }
    pub async fn git(&self, args: &[&str]) -> Result<String> {
        git_at(&self.root, args).await
    }
    pub async fn current(&self) -> Result<String> {
        Ok(self
            .git(&["symbolic-ref", "--short", "HEAD"])
            .await?
            .trim()
            .into())
    }
    pub async fn clean(&self) -> Result<()> {
        if !self.git(&["status", "--porcelain"]).await?.is_empty() {
            bail!("Commit or stash changes before this operation");
        }
        Ok(())
    }
    pub fn dir(&self) -> PathBuf {
        self.common.join("sk")
    }
    pub fn lock(&self) -> Result<RepositoryLock> {
        std::fs::create_dir_all(self.dir())?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.dir().join("lock"))?;
        file.try_lock_exclusive()
            .context("Another sk operation is active")?;
        Ok(RepositoryLock(file))
    }
    pub async fn load(&self) -> Result<State> {
        match tokio::fs::read(self.dir().join("state.json")).await {
            Ok(bytes) => {
                let state: State = serde_json::from_slice(&bytes)?;
                if state.version != 1 {
                    bail!("Unsupported state version");
                }
                Ok(state)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(State::default())
            }
            Err(e) => Err(e.into()),
        }
    }
    pub async fn save(&self, state: &State) -> Result<()> {
        atomic_json(&self.dir().join("state.json"), state)
    }
    pub async fn ensure_idle(&self) -> Result<()> {
        if self.dir().join("journal.json").exists() {
            bail!("An operation is pending; run sk continue or sk abort");
        }
        for marker in [
            "rebase-merge",
            "rebase-apply",
            "MERGE_HEAD",
            "CHERRY_PICK_HEAD",
        ] {
            let path = self.git(&["rev-parse", "--git-path", marker]).await?;
            if self.root.join(path.trim()).exists() {
                bail!("Finish the existing Git operation first");
            }
        }
        Ok(())
    }
}
async fn git_at(root: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .current_dir(root)
        .args(args)
        .env("GIT_NO_LAZY_FETCH", "1")
        .output()
        .await?;
    if !out.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8(out.stdout)?)
}
fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("No state directory")?;
    std::fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut temp, value)?;
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}
pub fn slug(message: &str) -> String {
    let text = message.to_lowercase();
    let parts: Vec<_> = text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();
    let value = parts.join("-");
    if value.is_empty() {
        "change".into()
    } else {
        value
    }
}
pub async fn run(args: &[String]) -> Result<()> {
    let repo = Repository::discover().await?;
    let _lock = repo.lock()?;
    match args.first().map(String::as_str) {
        Some("modify") => workflow::modify(&repo, &args[1..]).await,
        Some("track") => workflow::track(&repo, &args[1..]).await,
        Some("split") => workflow::split(&repo, &args[1..]).await,
        Some("create") => create(&repo, &args[1..]).await,
        Some("reorder") => reorder::run(&repo, &args[1..]).await,
        Some("restack") => restack(&repo).await,
        Some("continue") => resume(&repo).await,
        Some("abort") => abort(&repo).await,
        _ => bail!("Unknown core command"),
    }
}

pub async fn foreign_worktrees(repo: &Repository, stack: &Stack) -> Result<()> {
    let text = repo.git(&["worktree", "list", "--porcelain"]).await?;
    let mut path = String::new();
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("worktree ") {
            path = value.into();
        }
        if let Some(name) = line.strip_prefix("branch refs/heads/")
            && Path::new(&path) != repo.root
            && stack.branches.iter().any(|b| b.name == name)
        {
            bail!("{} is checked out in {}; skipping stack", name, path);
        }
    }
    Ok(())
}

pub async fn plan_stack(
    repo: &Repository,
    stack: &Stack,
    heads: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    foreign_worktrees(repo, stack).await?;
    for name in std::iter::once(&stack.trunk)
        .chain(stack.branches.iter().map(|b| &b.name))
    {
        let oid = repo
            .git(&["rev-parse", "--verify", &format!("refs/heads/{name}")])
            .await
            .ok()
            .map(|s| s.trim().to_string());
        repo.snapshots
            .lock()
            .map_err(|_| anyhow::anyhow!("Snapshot lock poisoned"))?
            .entry(name.clone())
            .or_insert(oid);
    }
    let mut result = BTreeMap::new();
    let mut parent = match heads.get(&stack.trunk) {
        Some(oid) => oid.clone(),
        None => repo.git(&["rev-parse", &stack.trunk]).await?.trim().into(),
    };
    for branch in &stack.branches {
        let tip = match heads.get(&branch.name) {
            Some(oid) => oid.clone(),
            None => repo.git(&["rev-parse", &branch.name]).await?.trim().into(),
        };
        let next = if branch.base == parent
            && repo
                .git(&["merge-base", "--is-ancestor", &parent, &tip])
                .await
                .is_ok()
        {
            tip
        } else if tip == branch.base {
            parent.clone()
        } else {
            let range = format!("{}..{}", branch.base, tip);
            let onto = format!("--onto={parent}");
            let target = format!("--ref=refs/heads/{}", branch.name);
            let output = Command::new("git")
                .current_dir(&repo.root)
                .args(["replay", "--ref-action=print", &onto, &target, &range])
                .env("GIT_NO_LAZY_FETCH", "1")
                .output()
                .await?;
            if output.status.code() == Some(1) {
                return Err(ReplayConflict(branch.name.clone()).into());
            }
            if !output.status.success() {
                bail!(
                    "Replay error on {}: {}",
                    branch.name,
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            let out = String::from_utf8(output.stdout)?;
            out.lines()
                .find_map(|l| {
                    let parts: Vec<_> = l.split_whitespace().collect();
                    (parts.len() == 4 && parts[0] == "update")
                        .then(|| parts[2].to_string())
                })
                .context("Replay did not produce a branch update")?
        };
        result.insert(branch.name.clone(), next.clone());
        parent = next;
    }
    Ok(result)
}
#[derive(Debug)]
struct ReplayConflict(String);
impl std::fmt::Display for ReplayConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Replay conflict on {}", self.0)
    }
}
impl std::error::Error for ReplayConflict {}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReconcileStep {
    pub name: String,
    pub base: String,
    pub onto: String,
    pub head: String,
}
#[derive(Clone, Serialize, Deserialize)]
struct Journal {
    original: State,
    proposed: State,
    original_branch: String,
    checkout_oid: String,
    old: BTreeMap<String, Option<String>>,
    new: BTreeMap<String, Option<String>>,
    kind: String,
    stack_index: usize,
    next: usize,
    #[serde(default)]
    reconcile_steps: Vec<ReconcileStep>,
    #[serde(default)]
    reconcile_next: usize,
}
fn journal_path(repo: &Repository) -> PathBuf {
    repo.dir().join("journal.json")
}
async fn journal(repo: &Repository) -> Result<Journal> {
    Ok(serde_json::from_slice(
        &tokio::fs::read(journal_path(repo))
            .await
            .context("No pending operation")?,
    )?)
}
async fn transaction(
    repo: &Repository,
    from: &BTreeMap<String, Option<String>>,
    to: &BTreeMap<String, Option<String>>,
) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut input = String::from("start\n");
    for (name, new) in to {
        let old = from.get(name).and_then(|v| v.as_deref());
        match (old, new.as_deref()) {
            (Some(old), Some(new)) => {
                input += &format!("update refs/heads/{name} {new} {old}\n")
            }
            (None, Some(new)) => {
                input += &format!("create refs/heads/{name} {new}\n")
            }
            (Some(old), None) => {
                input += &format!("delete refs/heads/{name} {old}\n")
            }
            _ => (),
        }
    }
    input += "prepare\ncommit\n";
    let mut child = Command::new("git")
        .current_dir(&repo.root)
        .args(["update-ref", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .context("Missing stdin")?
        .write_all(input.as_bytes())
        .await?;
    let output = child.wait_with_output().await?;
    if !output.status.success() {
        bail!(
            "Ref transaction: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}
pub async fn apply_plan(
    repo: &Repository,
    original: &State,
    proposed: &State,
    heads: &BTreeMap<String, String>,
) -> Result<()> {
    repo.clean().await?;
    repo.ensure_idle().await?;
    let mut old = BTreeMap::new();
    let mut new = BTreeMap::new();
    for (name, oid) in heads {
        old.insert(
            name.clone(),
            repo.git(&["rev-parse", "--verify", &format!("refs/heads/{name}")])
                .await
                .ok()
                .map(|s| s.trim().to_string()),
        );
        new.insert(name.clone(), Some(oid.clone()));
    }
    let proposed_names: Vec<_> = proposed
        .stacks
        .iter()
        .flat_map(|s| s.branches.iter().map(|b| &b.name))
        .collect();
    for b in original.stacks.iter().flat_map(|s| &s.branches) {
        if !proposed_names.contains(&&b.name) {
            let oid = repo
                .git(&["rev-parse", "--verify", &b.name])
                .await
                .ok()
                .map(|s| s.trim().to_string());
            if oid.is_some() {
                old.insert(b.name.clone(), oid);
                new.insert(b.name.clone(), None);
            }
        }
    }
    {
        let snapshots = repo
            .snapshots
            .lock()
            .map_err(|_| anyhow::anyhow!("Snapshot lock poisoned"))?;
        for (name, oid) in &old {
            if let Some(expected) = snapshots.get(name)
                && expected != oid
            {
                bail!("{name} changed since replay planning");
            }
        }
    }
    let snapshots = repo
        .snapshots
        .lock()
        .map_err(|_| anyhow::anyhow!("Snapshot lock poisoned"))?
        .clone();
    for (name, expected) in snapshots {
        let live = repo
            .git(&["rev-parse", "--verify", &format!("refs/heads/{name}")])
            .await
            .ok()
            .map(|s| s.trim().to_string());
        if live != expected {
            bail!("{name} changed since planning");
        }
    }
    let mut state = proposed.clone();
    for stack in &mut state.stacks {
        if !stack.branches.iter().any(|b| heads.contains_key(&b.name)) {
            continue;
        }
        let mut parent = heads.get(&stack.trunk).cloned().unwrap_or(
            repo.git(&["rev-parse", &stack.trunk]).await?.trim().into(),
        );
        for b in &mut stack.branches {
            if let Some(tip) = heads.get(&b.name) {
                b.base = parent.clone();
                parent = tip.clone();
            } else {
                parent = repo.git(&["rev-parse", &b.name]).await?.trim().into();
            }
        }
    }
    let original_branch = repo.current().await?;
    let j = Journal {
        original: original.clone(),
        proposed: state,
        original_branch,
        checkout_oid: repo.git(&["rev-parse", "HEAD"]).await?.trim().into(),
        old,
        new,
        kind: "transaction".into(),
        stack_index: 0,
        next: 0,
        reconcile_steps: vec![],
        reconcile_next: 0,
    };
    save_journal(repo, &j).await?;
    transaction(repo, &j.old, &j.new).await?;
    finish(repo, &j).await
}
async fn finish(repo: &Repository, j: &Journal) -> Result<()> {
    for (name, expected) in &j.new {
        let live = repo
            .git(&["rev-parse", "--verify", &format!("refs/heads/{name}")])
            .await
            .ok()
            .map(|s| s.trim().to_string());
        if &live != expected {
            bail!("{name} changed outside the recorded operation");
        }
    }
    let target = if j.new.get(&j.original_branch) == Some(&None) {
        j.proposed
            .stacks
            .iter()
            .find(|s| {
                j.original.stacks.iter().any(|old| {
                    old.id == s.id
                        && old
                            .branches
                            .iter()
                            .any(|b| b.name == j.original_branch)
                })
            })
            .and_then(|s| {
                s.branches
                    .first()
                    .map(|b| b.name.clone())
                    .or(Some(s.trunk.clone()))
            })
            .unwrap_or_else(|| "main".into())
    } else {
        j.original_branch.clone()
    };
    verify_checkout(repo, j).await?;
    repo.git(&["symbolic-ref", "HEAD", &format!("refs/heads/{target}")])
        .await?;
    repo.git(&["reset", "--hard", &format!("refs/heads/{target}"), "--"])
        .await?;
    repo.save(&j.proposed).await?;
    {
        let mut snapshots = repo
            .snapshots
            .lock()
            .map_err(|_| anyhow::anyhow!("Snapshot lock poisoned"))?;
        for (name, oid) in &j.new {
            snapshots.insert(name.clone(), oid.clone());
        }
    }
    clear_journal(repo).await?;
    Ok(())
}

async fn create(repo: &Repository, args: &[String]) -> Result<()> {
    use std::io::{IsTerminal, Write};
    repo.ensure_idle().await?;
    let mut name = None;
    let mut message = None;
    let mut all = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-m" | "--message" => {
                i += 1;
                message =
                    Some(args.get(i).context("-m needs a message")?.clone());
            }
            "-a" | "--all" => all = true,
            "-am" | "-ma" => {
                all = true;
                i += 1;
                message =
                    Some(args.get(i).context("-am needs a message")?.clone());
            }
            arg if arg.starts_with('-') => bail!("Unknown flag {arg}"),
            arg => {
                if name.replace(arg.to_string()).is_some() {
                    bail!("Only one branch name allowed");
                }
            }
        }
        i += 1;
    }
    let current = repo.current().await?;
    let mut state = repo.load().await?;
    let index = state
        .stacks
        .iter()
        .position(|s| s.branches.iter().any(|b| b.name == current));
    if let Some(index) = index
        && state.stacks[index].branches.last().map(|b| &b.name)
            != Some(&current)
    {
        bail!("Create only from the stack tip or trunk");
    }
    let trunk = if index.is_some() {
        current.clone()
    } else {
        let known = state.stacks.iter().any(|s| s.trunk == current);
        let configured = repo.git(&["config", "--get", "sk.trunk"]).await.ok();
        let cached = repo
            .git(&["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
            .await
            .ok();
        let expected = configured
            .map(|s| s.trim().to_string())
            .or_else(|| {
                cached.and_then(|s| {
                    s.trim().split_once('/').map(|(_, b)| b.to_string())
                })
            })
            .or_else(|| {
                (["main", "master"].contains(&current.as_str()))
                    .then(|| current.clone())
            });
        if !known && expected.as_deref() != Some(&current) {
            bail!("Start from trunk, or set git config sk.trunk <branch>");
        }
        current.clone()
    };
    let explicit = name.is_some();
    let mut branch_name = match name {
        Some(name) => name,
        None => format!(
            "{}_{}",
            chrono::Local::now().format("%m-%d"),
            slug(
                message
                    .as_deref()
                    .context("Provide a branch name or -m message")?
            )
        ),
    };
    repo.git(&["check-ref-format", "--branch", &branch_name])
        .await?;
    let original_name = branch_name.clone();
    let mut suffix = 2;
    while repo
        .git(&["show-ref", "--verify", &format!("refs/heads/{branch_name}")])
        .await
        .is_ok()
    {
        if explicit {
            bail!("Branch {branch_name} already exists");
        }
        branch_name = format!("{original_name}-{suffix}");
        suffix += 1;
    }
    if !all && std::io::stdin().is_terminal() {
        let dirty = repo.git(&["diff", "--name-only"]).await?;
        let untracked = repo
            .git(&["ls-files", "--others", "--exclude-standard"])
            .await?;
        if !dirty.is_empty() || !untracked.is_empty() {
            eprint!("Stage unstaged and untracked changes? [y/N] ");
            std::io::stderr().flush()?;
            let mut reply = String::new();
            std::io::stdin().read_line(&mut reply)?;
            all = reply.trim().eq_ignore_ascii_case("y");
        }
    }
    if all {
        repo.git(&["add", "--all"]).await?;
    }
    let staged = !repo
        .git(&["diff", "--cached", "--name-only"])
        .await?
        .is_empty();
    if staged && message.is_none() && !std::io::stdin().is_terminal() {
        bail!("Provide -m to commit staged changes noninteractively");
    }
    let base = repo.git(&["rev-parse", "HEAD"]).await?.trim().to_string();
    repo.git(&["switch", "-c", &branch_name]).await?;
    if staged {
        let mut command = Command::new("git");
        command.current_dir(&repo.root).arg("commit");
        if let Some(message) = &message {
            command.args(["-m", message]);
        }
        if !command.status().await?.success() {
            repo.git(&["switch", &current]).await?;
            repo.git(&["branch", "-D", &branch_name]).await?;
            bail!("Commit failed; returned to original branch");
        }
    }
    let branch = Branch {
        name: branch_name.clone(),
        base,
        remote_oid: None,
        fetched_oid: None,
        pr: None,
    };
    if let Some(index) = index {
        state.stacks[index].branches.push(branch);
    } else {
        state.stacks.push(Stack {
            id: branch_name.clone(),
            trunk,
            remote: repo
                .git(&["config", "--get", "sk.remote"])
                .await
                .unwrap_or_else(|_| "origin".into())
                .trim()
                .into(),
            branches: vec![branch],
            github_number: None,
        });
    }
    repo.save(&state).await?;
    println!("Created {branch_name}");
    Ok(())
}
async fn restack(repo: &Repository) -> Result<()> {
    repo.ensure_idle().await?;
    repo.clean().await?;
    let original = repo.load().await?;
    let current = repo.current().await?;
    let index = original
        .stacks
        .iter()
        .position(|s| s.branches.iter().any(|b| b.name == current))
        .context("Check out a tracked branch before restacking")?;
    let mut proposed = original.clone();
    snapshot_state(repo, &original).await?;
    let mut heads = BTreeMap::new();
    let mut reconcile_steps = Vec::new();
    let mut reconcile_conflict = None;
    for b in &mut proposed.stacks[index].branches {
        if let Some(remote) = &b.fetched_oid {
            let local =
                repo.git(&["rev-parse", &b.name]).await?.trim().to_string();
            if Some(remote) != b.remote_oid.as_ref() && local != *remote {
                use std::io::{IsTerminal, Write};
                if !std::io::stdin().is_terminal() {
                    bail!(
                        "Divergence on {}; run restack interactively",
                        b.name
                    );
                }
                eprint!(
                    "{} diverged: [l]ocal, [r]emote, [p]lay local changes \
                     onto remote, [c]ancel: ",
                    b.name
                );
                std::io::stderr().flush()?;
                let mut reply = String::new();
                std::io::stdin().read_line(&mut reply)?;
                match reply.trim() {
                    "l" => (),
                    "r" => {
                        heads.insert(b.name.clone(), remote.clone());
                        reconcile_steps.push(ReconcileStep {
                            name: b.name.clone(),
                            base: remote.clone(),
                            onto: remote.clone(),
                            head: remote.clone(),
                        });
                    }
                    "p" => {
                        match plan_reconciliation(repo, b).await {
                            Ok(head) => {
                                heads.insert(b.name.clone(), head);
                            }
                            Err(error) => {
                                if error
                                    .downcast_ref::<ReplayConflict>()
                                    .is_none()
                                {
                                    return Err(error);
                                }
                                reconcile_conflict = Some(error);
                            }
                        }
                        reconcile_steps.push(ReconcileStep {
                            name: b.name.clone(),
                            base: b.remote_oid.clone().unwrap(),
                            onto: remote.clone(),
                            head: local,
                        });
                    }
                    _ => bail!("Cancelled"),
                }
                b.remote_oid = Some(remote.clone());
            }
        }
    }
    let result = match reconcile_conflict {
        Some(error) => Err(error),
        None => plan_stack(repo, &proposed.stacks[index], &heads).await,
    };
    match result {
        Ok(plan) => apply_plan(repo, &original, &proposed, &plan).await,
        Err(error) => {
            if error.downcast_ref::<ReplayConflict>().is_none() {
                return Err(error);
            }
            eprintln!("{error}; starting interactive rebase");
            begin_rebase(repo, original, proposed, index, reconcile_steps).await
        }
    }
}
async fn verify_snapshots(repo: &Repository) -> Result<()> {
    let snapshots = repo
        .snapshots
        .lock()
        .map_err(|_| anyhow::anyhow!("Snapshot lock poisoned"))?
        .clone();
    for (name, expected) in snapshots {
        let actual = repo
            .git(&["rev-parse", "--verify", &format!("refs/heads/{name}")])
            .await
            .ok()
            .map(|s| s.trim().to_owned());
        if actual != expected {
            bail!("{name} changed since planning; operation cancelled");
        }
    }
    Ok(())
}

async fn begin_rebase(
    repo: &Repository,
    original: State,
    proposed: State,
    index: usize,
    reconcile_steps: Vec<ReconcileStep>,
) -> Result<()> {
    verify_snapshots(repo).await?;
    foreign_worktrees(repo, &proposed.stacks[index]).await?;
    let mut old = BTreeMap::new();
    for b in &original.stacks[index].branches {
        old.insert(
            b.name.clone(),
            Some(repo.git(&["rev-parse", &b.name]).await?.trim().into()),
        );
    }
    let j = Journal {
        original,
        proposed,
        original_branch: repo.current().await?,
        checkout_oid: repo.git(&["rev-parse", "HEAD"]).await?.trim().into(),
        new: old.clone(),
        old,
        kind: "rebase".into(),
        stack_index: index,
        next: 0,
        reconcile_steps,
        reconcile_next: 0,
    };
    save_journal(repo, &j).await?;
    advance(repo, j, false).await
}

async fn advance(
    repo: &Repository,
    mut j: Journal,
    continuing: bool,
) -> Result<()> {
    let mut continuing = continuing;
    if j.reconcile_next < j.reconcile_steps.len() {
        j = advance_reconciliations(repo, j, continuing).await?;
        continuing = false;
    }
    if continuing {
        let out = Command::new("git")
            .current_dir(&repo.root)
            .args(["rebase", "--continue"])
            .env("GIT_EDITOR", "true")
            .status()
            .await?;
        if !out.success() {
            bail!("Resolve and stage conflicts, then sk continue");
        }
        let b = &j.proposed.stacks[j.stack_index].branches[j.next];
        j.new.insert(
            b.name.clone(),
            Some(repo.git(&["rev-parse", &b.name]).await?.trim().into()),
        );
        let stack = &j.proposed.stacks[j.stack_index];
        let parent = if j.next == 0 {
            &stack.trunk
        } else {
            &stack.branches[j.next - 1].name
        };
        let base = repo.git(&["rev-parse", parent]).await?.trim().to_string();
        j.proposed.stacks[j.stack_index].branches[j.next].base = base;
        j.next += 1;
        save_journal(repo, &j).await?;
    }
    while j.next < j.proposed.stacks[j.stack_index].branches.len() {
        let stack = &j.proposed.stacks[j.stack_index];
        let b = &stack.branches[j.next];
        let parent = if j.next == 0 {
            &stack.trunk
        } else {
            &stack.branches[j.next - 1].name
        };
        let parent_oid =
            repo.git(&["rev-parse", parent]).await?.trim().to_string();
        save_journal(repo, &j).await?;
        let status = Command::new("git")
            .current_dir(&repo.root)
            .args([
                "-c",
                "rebase.updateRefs=false",
                "-c",
                "rebase.autoStash=false",
                "rebase",
                "--onto",
                &parent_oid,
                &b.base,
                &b.name,
            ])
            .env("GIT_NO_LAZY_FETCH", "1")
            .status()
            .await?;
        if !status.success() {
            bail!("Resolve and stage conflicts, then sk continue; or sk abort");
        }
        let name = b.name.clone();
        j.proposed.stacks[j.stack_index].branches[j.next].base = parent_oid;
        j.new.insert(
            name.clone(),
            Some(repo.git(&["rev-parse", &name]).await?.trim().into()),
        );
        j.next += 1;
        save_journal(repo, &j).await?;
    }
    finish(repo, &j).await
}
async fn rebase_pending(repo: &Repository) -> Result<bool> {
    for marker in ["rebase-merge", "rebase-apply"] {
        let path = repo.git(&["rev-parse", "--git-path", marker]).await?;
        if repo.root.join(path.trim()).exists() {
            return Ok(true);
        }
    }
    Ok(false)
}
async fn recover_completed_step(
    repo: &Repository,
    j: &mut Journal,
) -> Result<()> {
    if j.reconcile_next < j.reconcile_steps.len() {
        let step = j.reconcile_steps[j.reconcile_next].clone();
        let tip = repo
            .git(&["rev-parse", &step.name])
            .await?
            .trim()
            .to_string();
        if j.new.get(&step.name).and_then(|v| v.as_ref()) == Some(&tip) {
            return Ok(());
        }
        repo.clean().await?;
        if tip == step.head {
            j.new.insert(step.name, Some(tip));
            save_journal(repo, j).await?;
            return Ok(());
        }
        let message = repo
            .git(&[
                "reflog",
                "-1",
                "--format=%gs",
                &format!("refs/heads/{}", step.name),
            ])
            .await?;
        if !message.starts_with("rebase (finish)") {
            bail!("{} changed outside the recorded reconciliation", step.name);
        }
        repo.git(&["merge-base", "--is-ancestor", &step.onto, &tip])
            .await?;
        j.new.insert(step.name, Some(tip));
        j.reconcile_next += 1;
        save_journal(repo, j).await?;
        return Ok(());
    }
    let stack = &j.proposed.stacks[j.stack_index];
    if j.next >= stack.branches.len() {
        return Ok(());
    }
    let name = stack.branches[j.next].name.clone();
    let tip = repo.git(&["rev-parse", &name]).await?.trim().to_string();
    if j.new.get(&name).and_then(|v| v.as_ref()) == Some(&tip) {
        return Ok(());
    }
    repo.clean().await?;
    let reflog = repo
        .git(&[
            "reflog",
            "-1",
            "--format=%gs",
            &format!("refs/heads/{name}"),
        ])
        .await?;
    if !reflog.starts_with("rebase (finish)") {
        bail!("{name} changed outside the recorded rebase");
    }
    let parent = if j.next == 0 {
        &stack.trunk
    } else {
        &stack.branches[j.next - 1].name
    };
    let base = repo.git(&["rev-parse", parent]).await?.trim().to_string();
    repo.git(&["merge-base", "--is-ancestor", &base, &tip])
        .await?;
    j.proposed.stacks[j.stack_index].branches[j.next].base = base;
    j.new.insert(name, Some(tip));
    j.next += 1;
    save_journal(repo, j).await?;
    Ok(())
}
async fn resume(repo: &Repository) -> Result<()> {
    let mut j = journal(repo).await?;
    if j.kind == "rebase" {
        if rebase_pending(repo).await? {
            return advance(repo, j, true).await;
        }
        recover_completed_step(repo, &mut j).await?;
        return advance(repo, j, false).await;
    }
    let mut old_match = true;
    let mut new_match = true;
    for (name, expected) in &j.new {
        let now = repo
            .git(&["rev-parse", "--verify", &format!("refs/heads/{name}")])
            .await
            .ok()
            .map(|s| s.trim().to_string());
        old_match &= now == j.old[name];
        new_match &= &now == expected;
    }
    if !old_match && !new_match {
        bail!("Refs changed outside the recorded transaction");
    }
    verify_checkout(repo, &j).await?;
    if old_match && !new_match {
        transaction(repo, &j.old, &j.new).await?;
    }
    finish(repo, &j).await
}
async fn abort(repo: &Repository) -> Result<()> {
    let mut j = journal(repo).await?;
    if j.kind == "rebase" {
        if rebase_pending(repo).await? {
            repo.git(&["rebase", "--abort"]).await?;
        } else {
            recover_completed_step(repo, &mut j).await?;
        }
    }
    verify_checkout(repo, &j).await?;
    let mut now = BTreeMap::new();
    for name in j.old.keys() {
        let oid = repo
            .git(&["rev-parse", "--verify", &format!("refs/heads/{name}")])
            .await
            .ok()
            .map(|s| s.trim().to_string());
        if oid != j.old[name] && oid != j.new[name] {
            bail!("{name} changed outside the recorded operation");
        }
        now.insert(name.clone(), oid);
    }
    transaction(repo, &now, &j.old).await?;
    repo.git(&[
        "symbolic-ref",
        "HEAD",
        &format!("refs/heads/{}", j.original_branch),
    ])
    .await?;
    repo.git(&[
        "reset",
        "--hard",
        &format!("refs/heads/{}", j.original_branch),
        "--",
    ])
    .await?;
    repo.save(&j.original).await?;
    clear_journal(repo).await?;
    Ok(())
}

pub async fn snapshot_state(repo: &Repository, state: &State) -> Result<()> {
    for stack in &state.stacks {
        for name in std::iter::once(&stack.trunk)
            .chain(stack.branches.iter().map(|b| &b.name))
        {
            let oid = repo
                .git(&["rev-parse", "--verify", &format!("refs/heads/{name}")])
                .await
                .ok()
                .map(|s| s.trim().to_string());
            repo.snapshots
                .lock()
                .map_err(|_| anyhow::anyhow!("Snapshot lock poisoned"))?
                .entry(name.clone())
                .or_insert(oid);
        }
    }
    Ok(())
}

async fn verify_checkout(repo: &Repository, j: &Journal) -> Result<()> {
    let current = repo.current().await?;
    let mut checkouts = vec![j.original_branch.clone()];
    if j.kind == "rebase" {
        for index in [j.reconcile_next, j.reconcile_next.saturating_sub(1)] {
            if let Some(step) = j.reconcile_steps.get(index) {
                checkouts.push(step.name.clone());
            }
        }
        let stack = &j.proposed.stacks[j.stack_index];
        for index in [j.next, j.next.saturating_sub(1)] {
            if let Some(branch) = stack.branches.get(index) {
                checkouts.push(branch.name.clone());
            }
        }
    } else if j.new.get(&j.original_branch) == Some(&None) {
        for stack in &j.proposed.stacks {
            checkouts.push(stack.trunk.clone());
            if let Some(branch) = stack.branches.first() {
                checkouts.push(branch.name.clone());
            }
        }
    }
    if !checkouts.contains(&current) {
        bail!("Checkout changed outside the recorded operation");
    }
    if !repo
        .git(&["ls-files", "--others", "--exclude-standard"])
        .await?
        .is_empty()
        || repo.git(&["diff-files", "--quiet", "--"]).await.is_err()
    {
        bail!(
            "Checkout changed since operation; preserve your edits before recovery"
        );
    }
    let tree = repo.git(&["write-tree"]).await?.trim().to_string();
    let mut allowed = vec![j.checkout_oid.clone()];
    allowed.extend(
        j.old
            .values()
            .chain(j.new.values())
            .filter_map(Clone::clone),
    );
    for oid in allowed {
        if repo
            .git(&["rev-parse", &format!("{oid}^{{tree}}")])
            .await?
            .trim()
            == tree
        {
            return Ok(());
        }
    }
    bail!(
        "Index changed since operation; preserve staged edits before recovery"
    )
}

async fn save_journal(repo: &Repository, j: &Journal) -> Result<()> {
    atomic_json(&journal_path(repo), j)?;
    repo.git(&["update-ref", "refs/sk/recovery/checkout", &j.checkout_oid])
        .await?;
    for (phase, refs) in [("old", &j.old), ("new", &j.new)] {
        for (name, oid) in refs {
            if let Some(oid) = oid {
                let reference = format!("refs/sk/recovery/{phase}/{name}");
                repo.git(&["update-ref", &reference, oid]).await?;
            }
        }
    }
    Ok(())
}
async fn clear_journal(repo: &Repository) -> Result<()> {
    tokio::fs::remove_file(journal_path(repo)).await?;
    let refs = repo
        .git(&["for-each-ref", "--format=%(refname)", "refs/sk/recovery/"])
        .await?;
    for reference in refs.lines() {
        repo.git(&["update-ref", "-d", reference]).await?;
    }
    Ok(())
}

/// Reconcile unpublished commits onto a cached fetched head without ref edits.
pub async fn plan_reconciliation(
    repo: &Repository,
    branch: &Branch,
) -> Result<String> {
    let baseline = branch
        .remote_oid
        .as_deref()
        .context("No published boundary; choose local or remote")?;
    let remote = branch
        .fetched_oid
        .as_deref()
        .context("No fetched head; run sync first")?;
    let local = repo.git(&["rev-parse", &branch.name]).await?;
    let local = local.trim();
    repo.git(&["merge-base", "--is-ancestor", baseline, local])
        .await
        .context("Published boundary was rewritten; choose local or remote")?;
    repo.git(&["merge-base", "--is-ancestor", &branch.base, remote])
        .await
        .context("Fetched parent was rewritten; choose local or remote")?;
    if local == baseline {
        return Ok(remote.to_owned());
    }
    let range = format!("{baseline}..{local}");
    let onto = format!("--onto={remote}");
    let target = format!("--ref=refs/heads/{}", branch.name);
    let output = Command::new("git")
        .current_dir(&repo.root)
        .args(["replay", "--ref-action=print", &onto, &target, &range])
        .env("GIT_NO_LAZY_FETCH", "1")
        .output()
        .await?;
    if output.status.code() == Some(1) {
        return Err(ReplayConflict(branch.name.clone()).into());
    }
    if !output.status.success() {
        bail!(
            "Reconciliation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8(output.stdout)?
        .lines()
        .find_map(|line| {
            let parts: Vec<_> = line.split_whitespace().collect();
            (parts.len() == 4 && parts[0] == "update")
                .then(|| parts[2].to_owned())
        })
        .context("Replay did not produce a reconciled head")
}

async fn advance_reconciliations(
    repo: &Repository,
    mut j: Journal,
    continuing: bool,
) -> Result<Journal> {
    if continuing {
        let status = Command::new("git")
            .current_dir(&repo.root)
            .args(["rebase", "--continue"])
            .env("GIT_EDITOR", "true")
            .env("GIT_NO_LAZY_FETCH", "1")
            .status()
            .await?;
        if !status.success() {
            bail!(
                "Resolve and stage reconciliation conflicts, then sk continue"
            );
        }
        let name = j.reconcile_steps[j.reconcile_next].name.clone();
        let tip = repo.git(&["rev-parse", &name]).await?.trim().to_string();
        j.new.insert(name, Some(tip));
        j.reconcile_next += 1;
        save_journal(repo, &j).await?;
    }
    while j.reconcile_next < j.reconcile_steps.len() {
        let step = j.reconcile_steps[j.reconcile_next].clone();
        repo.clean().await?;
        let live = repo
            .git(&["rev-parse", &step.name])
            .await?
            .trim()
            .to_string();
        if j.new.get(&step.name).and_then(|v| v.as_ref()) != Some(&live) {
            bail!("{} changed outside the operation", step.name);
        }
        if live != step.head {
            repo.git(&["switch", "--no-guess", &step.name]).await?;
            repo.git(&["reset", "--hard", &step.head, "--"]).await?;
            j.new.insert(step.name.clone(), Some(step.head.clone()));
            save_journal(repo, &j).await?;
        }
        let status = Command::new("git")
            .current_dir(&repo.root)
            .args([
                "-c",
                "rebase.updateRefs=false",
                "-c",
                "rebase.autoStash=false",
                "rebase",
                "--onto",
                &step.onto,
                &step.base,
                &step.name,
            ])
            .env("GIT_NO_LAZY_FETCH", "1")
            .status()
            .await?;
        if !status.success() {
            bail!(
                "Resolve and stage reconciliation conflicts, then sk continue; or sk abort"
            );
        }
        let tip = repo
            .git(&["rev-parse", &step.name])
            .await?
            .trim()
            .to_string();
        j.new.insert(step.name, Some(tip));
        j.reconcile_next += 1;
        save_journal(repo, &j).await?;
    }
    Ok(j)
}
