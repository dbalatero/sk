//! Network operations, isolated to sync, submit, and merge.
mod merge;
mod submit_ui;
mod sync_ui;

use anyhow::{Context, Result, bail};
use futures::{StreamExt, stream};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{IsTerminal, Write};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

use crate::core::{self, PullRequest, Repository, Stack, State};

#[derive(Clone)]
pub struct GitHub {
    client: reqwest::Client,
    base: String,
    repository: String,
    token: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Pull {
    pub number: u64,
    pub html_url: String,
    pub state: String,
    #[serde(default)]
    pub draft: bool,
    pub merged_at: Option<String>,
    pub head: RefInfo,
    #[serde(default)]
    pub base: Option<RefInfo>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RefInfo {
    pub sha: String,
    #[serde(rename = "ref")]
    pub name: String,
}

#[derive(Debug, Deserialize)]
struct RemoteStack {
    number: u64,
    pull_requests: Vec<StackPull>,
}

#[derive(Debug, Deserialize)]
struct StackPull {
    number: u64,
    state: String,
    merged_at: Option<String>,
}

impl Pull {
    fn cache(&self) -> PullRequest {
        PullRequest {
            number: self.number,
            url: self.html_url.clone(),
            state: if self.merged_at.is_some() {
                "MERGED"
            } else if self.state == "closed" {
                "CLOSED"
            } else {
                "OPEN"
            }
            .into(),
            draft: self.draft,
            head_oid: self.head.sha.clone(),
        }
    }
}

impl GitHub {
    pub fn new(base: &str, repository: &str, token: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            base: base.trim_end_matches('/').into(),
            repository: repository.into(),
            token,
        }
    }

    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        self.request_url(
            method,
            &format!("{}/repos/{}/{}", self.base, self.repository, path),
        )
    }

    fn request_url(
        &self,
        method: Method,
        url: &str,
    ) -> reqwest::RequestBuilder {
        self.client
            .request(method, url)
            .bearer_auth(&self.token)
            .header("User-Agent", "sk/0.1")
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2026-03-10")
            .timeout(Duration::from_secs(30))
    }

    async fn response(
        request: reqwest::RequestBuilder,
    ) -> Result<Option<Value>> {
        let response = request.send().await.context("GitHub request failed")?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if status == reqwest::StatusCode::NO_CONTENT {
            return Ok(Some(Value::Null));
        }
        let body = response.text().await?;
        if !status.is_success() {
            let parsed = serde_json::from_str::<Value>(&body).ok();
            let mut message = parsed
                .as_ref()
                .and_then(|v| v["message"].as_str().map(str::to_owned))
                .unwrap_or_else(|| "unexpected API response".into());
            if let Some(details) = parsed.as_ref().and_then(|v| v.get("errors"))
            {
                message.push_str(&format!(": {details}"));
            }
            bail!("GitHub returned {status}: {message}");
        }
        Ok(Some(serde_json::from_str(&body)?))
    }

    pub async fn find_open(&self, branch: &str) -> Result<Option<Pull>> {
        let owner = self.repository.split('/').next().unwrap_or_default();
        let head = format!("{owner}:{branch}");
        let value = Self::response(
            self.request(Method::GET, "pulls")
                .query(&[("state", "open"), ("head", &head)]),
        )
        .await?
        .context("repository unavailable on GitHub")?;
        let pulls: Vec<Pull> = serde_json::from_value(value)?;
        Ok(pulls.into_iter().find(|pr| pr.head.name == branch))
    }

    async fn pull(&self, number: u64) -> Result<Option<Pull>> {
        Self::response(self.request(Method::GET, &format!("pulls/{number}")))
            .await?
            .map(serde_json::from_value)
            .transpose()
            .map_err(Into::into)
    }

    async fn create_pull(
        &self,
        head: &str,
        base: &str,
        title: &str,
        body: &str,
        draft: bool,
    ) -> Result<Pull> {
        let value =
            Self::response(self.request(Method::POST, "pulls").json(&json!({
                "head": head, "base": base, "title": title,
                "body": body, "draft": draft,
            })))
            .await?
            .context("repository unavailable on GitHub")?;
        Ok(serde_json::from_value(value)?)
    }

    async fn graphql(&self, query: &str, variables: Value) -> Result<Value> {
        let value = Self::response(
            self.request_url(Method::POST, &format!("{}/graphql", self.base))
                .json(&json!({"query": query, "variables": variables})),
        )
        .await?
        .context("GitHub GraphQL response unavailable")?;
        if let Some(errors) = value.get("errors") {
            bail!("GitHub GraphQL error: {errors}");
        }
        Ok(value)
    }

    async fn publish_pull(&self, number: u64) -> Result<bool> {
        let (owner, name) = self
            .repository
            .split_once('/')
            .context("invalid GitHub repository")?;
        let value = self
            .graphql(
                concat!(
                    "query($owner:String!,$name:String!,$number:Int!){",
                    "repository(owner:$owner,name:$name){",
                    "pullRequest(number:$number){id isDraft}}}",
                ),
                json!({"owner":owner,"name":name,"number":number}),
            )
            .await?;
        let pull = &value["data"]["repository"]["pullRequest"];
        if !pull["isDraft"]
            .as_bool()
            .context("Missing PR draft status")?
        {
            return Ok(false);
        }
        let id = pull["id"].as_str().context("Missing PR node ID")?;
        let value = self
            .graphql(
                concat!(
                    "mutation($id:ID!){markPullRequestReadyForReview(",
                    "input:{pullRequestId:$id}){pullRequest{isDraft}}}",
                ),
                json!({"id":id}),
            )
            .await?;
        if value["data"]["markPullRequestReadyForReview"]["pullRequest"]
            ["isDraft"].as_bool() != Some(false) {
            bail!("GitHub did not confirm PR #{number} is ready for review");
        }
        Ok(true)
    }

    async fn update_base(
        &self,
        number: u64,
        base: &str,
        allowed: &[u64],
    ) -> Result<bool> {
        let current = self
            .pull(number)
            .await?
            .context("pull request no longer exists")?;
        if current.base.as_ref().is_some_and(|b| b.name == base) {
            return Ok(false);
        }
        for stack in self.stacks().await? {
            if !stack.pull_requests.iter().any(|p| p.number == number) {
                continue;
            }
            if stack.pull_requests.iter().any(|p| {
                p.merged_at.is_none()
                    && p.state == "open"
                    && p.number != number
                    && !allowed.contains(&p.number)
            }) {
                bail!("Remote stack has other active PRs; run sk sync");
            }
            let path = format!("stacks/{}/unstack", stack.number);
            if let Some(value) =
                Self::response(self.request(Method::POST, &path)).await?
                && !value.is_null()
            {
                let remaining: RemoteStack = serde_json::from_value(value)?;
                if remaining.pull_requests.iter().any(|p| p.number == number) {
                    bail!(
                        "GitHub cannot change the base of a queued/locked PR"
                    );
                }
            }
        }
        Self::response(
            self.request(Method::PATCH, &format!("pulls/{number}"))
                .json(&json!({"base": base})),
        )
        .await?
        .context("pull request no longer exists")?;
        Ok(true)
    }

    async fn stacks(&self) -> Result<Vec<RemoteStack>> {
        let mut result = Vec::new();
        for page in 1.. {
            let value = Self::response(
                self.request(Method::GET, "stacks")
                    .query(&[("per_page", 100), ("page", page)]),
            )
            .await?
            .context("GitHub stacks are unavailable for this repository")?;
            let batch: Vec<RemoteStack> = serde_json::from_value(value)?;
            let count = batch.len();
            result.extend(batch);
            if count < 100 {
                break;
            }
        }
        Ok(result)
    }

    async fn publish_stack(
        &self,
        numbers: &[u64],
        known: Option<u64>,
    ) -> Result<Option<u64>> {
        if numbers.len() < 2 {
            return Ok(known);
        }
        let stacks = self.stacks().await?;
        let mut involved = Vec::new();
        for stack in stacks {
            if Some(stack.number) == known
                || stack
                    .pull_requests
                    .iter()
                    .any(|p| numbers.contains(&p.number))
            {
                let active: Vec<u64> = stack
                    .pull_requests
                    .iter()
                    .filter(|p| p.merged_at.is_none() && p.state == "open")
                    .map(|p| p.number)
                    .collect();
                if active.starts_with(numbers) {
                    return Ok(Some(stack.number));
                }
                if !active.is_empty()
                    && numbers.starts_with(&active)
                    && stack.pull_requests.last().map(|p| p.number)
                        == active.last().copied()
                {
                    let path = format!("stacks/{}/add", stack.number);
                    let response =
                        Self::response(self.request(Method::POST, &path).json(
                            &json!({"pull_requests": &numbers[active.len()..]}),
                        ))
                        .await;
                    match response {
                        Ok(Some(_)) => return Ok(Some(stack.number)),
                        Ok(None) => bail!("stack disappeared while appending"),
                        Err(error) => {
                            return self.recover_stack(numbers, error).await;
                        }
                    }
                }
                if active.iter().any(|n| !numbers.contains(n)) {
                    bail!(
                        "remote stack {} contains other active PRs; run sync",
                        stack.number
                    );
                }
                if stack.pull_requests.iter().any(|p| p.merged_at.is_none()) {
                    involved.push(stack);
                }
            }
        }
        for stack in involved {
            let path = format!("stacks/{}/unstack", stack.number);
            if let Some(value) =
                Self::response(self.request(Method::POST, &path)).await?
                && !value.is_null()
            {
                let remaining: RemoteStack = serde_json::from_value(value)?;
                if remaining.pull_requests.iter().any(|p| {
                    p.merged_at.is_none() && numbers.contains(&p.number)
                }) {
                    bail!("GitHub cannot regroup queued or locked PRs");
                }
            }
        }
        let response = Self::response(
            self.request(Method::POST, "stacks")
                .json(&json!({"pull_requests": numbers})),
        )
        .await;
        match response {
            Ok(Some(value)) => {
                let created: RemoteStack = serde_json::from_value(value)?;
                Ok(Some(created.number))
            }
            Ok(None) => {
                bail!("GitHub stacks are unavailable for this repository")
            }
            Err(error) => self.recover_stack(numbers, error).await,
        }
    }

    async fn recover_stack(
        &self,
        numbers: &[u64],
        original_error: anyhow::Error,
    ) -> Result<Option<u64>> {
        // A mutation can succeed even if its response fails or races a native
        // grouping. Rediscover before considering another mutation.
        for attempt in 0..3 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(200 * attempt)).await;
            }
            if let Ok(stacks) = self.stacks().await {
                for stack in stacks {
                    let active: Vec<_> = stack
                        .pull_requests
                        .iter()
                        .filter(|p| p.merged_at.is_none() && p.state == "open")
                        .map(|p| p.number)
                        .collect();
                    if active.starts_with(numbers) {
                        return Ok(Some(stack.number));
                    }
                }
            }
        }
        Err(original_error)
    }
}

pub fn parse_pr_document(text: &str) -> Result<(String, String)> {
    let mut lines = text.lines();
    let title = lines.next().unwrap_or_default().trim();
    if title.is_empty() {
        bail!("PR title must not be empty");
    }
    if !lines.next().unwrap_or_default().trim().is_empty() {
        bail!("line 2 must be blank; description starts on line 3");
    }
    Ok((
        title.into(),
        lines.collect::<Vec<_>>().join("\n").trim().into(),
    ))
}

pub fn commit_metadata(messages: &[String]) -> Result<(String, String)> {
    let first = messages.first().context("branch has no unique commits")?;
    let mut lines = first.lines();
    let title = lines.next().unwrap_or_default().trim().to_owned();
    if title.is_empty() {
        bail!("first commit has an empty subject");
    }
    let first_body = lines.collect::<Vec<_>>().join("\n");
    let body = std::iter::once(first_body.trim())
        .chain(messages.iter().skip(1).map(|m| m.trim()))
        .filter(|m| !m.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    Ok((title, body))
}

async fn edit_metadata(title: &str, body: &str) -> Result<(String, String)> {
    let editor = std::env::var("EDITOR").context("set EDITOR or use -n")?;
    edit_metadata_with_editor(title, body, &editor).await
}

async fn edit_metadata_with_editor(
    title: &str,
    body: &str,
    editor: &str,
) -> Result<(String, String)> {
    let parts = shell_words::split(editor).context("invalid EDITOR command")?;
    let executable = parts.first().context("EDITOR is empty")?;
    let mut file = tempfile::Builder::new().suffix(".md").tempfile()?;
    write!(file, "{title}\n\n{body}\n")?;
    file.flush()?;
    let result = Command::new(executable)
        .args(&parts[1..])
        .arg(file.path())
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await?;
    if !result.success() {
        bail!("editor failed; nothing was submitted");
    }
    parse_pr_document(&tokio::fs::read_to_string(file.path()).await?)
}

pub fn github_repository(remote_url: &str) -> Result<String> {
    let path = remote_url
        .strip_prefix("git@github.com:")
        .or_else(|| remote_url.strip_prefix("https://github.com/"))
        .or_else(|| remote_url.strip_prefix("ssh://git@github.com/"))
        .context("v1 requires a github.com SSH or HTTPS remote")?;
    let path = path.trim_end_matches('/').trim_end_matches(".git");
    let parts: Vec<_> = path.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|p| {
            p.is_empty()
                || !p
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        })
    {
        bail!("invalid GitHub repository remote");
    }
    Ok(path.into())
}

async fn api(repo: &Repository, remote: &str) -> Result<GitHub> {
    let url = repo.git(&["remote", "get-url", remote]).await?;
    let repository = github_repository(url.trim())?;
    let output = Command::new("gh")
        .args(["auth", "token", "--hostname", "github.com"])
        .output()
        .await
        .context("gh is required for authentication")?;
    if !output.status.success() {
        bail!("GitHub authentication unavailable; run gh auth login");
    }
    let token = String::from_utf8(output.stdout)?.trim().to_owned();
    if token.is_empty() {
        bail!("GitHub authentication returned an empty token");
    }
    Ok(GitHub::new("https://api.github.com", &repository, token))
}

#[derive(Debug, Default, PartialEq, Eq)]
struct SubmitOptions {
    stack: bool,
    draft: bool,
    publish: bool,
    no_edit: bool,
}

fn submit_options(args: &[String]) -> Result<SubmitOptions> {
    let mut result = SubmitOptions::default();
    for arg in args {
        match arg.as_str() {
            "--stack" | "--stack=true" | "-s" => result.stack = true,
            "--no-stack" | "--stack=false" => result.stack = false,
            "--draft" => result.draft = true,
            "--publish" => result.publish = true,
            "--no-edit" => result.no_edit = true,
            value if value.starts_with('-') && !value.starts_with("--") => {
                for flag in value[1..].chars() {
                    match flag {
                        's' => result.stack = true,
                        'd' => result.draft = true,
                        'p' => result.publish = true,
                        'n' => result.no_edit = true,
                        _ => bail!("unsupported submit flag: -{flag}"),
                    }
                }
            }
            _ => bail!("unsupported submit argument: {arg}"),
        }
    }
    Ok(result)
}

pub async fn run(args: &[String]) -> Result<()> {
    let command = args.first().context("missing remote command")?;
    match command.as_str() {
        "merge" => merge::run(&args[1..]).await,
        "submit" | "ss" => {
            let mut options = submit_options(&args[1..])?;
            if command == "ss" {
                options.stack = true;
            }
            let repo = Repository::discover().await?;
            let _lock = repo.lock()?;
            repo.ensure_idle().await?;
            let state = repo.load().await?;
            let current = repo.current().await?;
            let index = state
                .stacks
                .iter()
                .position(|s| s.branches.iter().any(|b| b.name == current))
                .context("checkout a tracked stack branch before submitting")?;
            let client = api(&repo, &state.stacks[index].remote).await?;
            let interactive = std::io::stdin().is_terminal()
                && std::io::stderr().is_terminal();
            submit(&repo, state, index, &client, options, interactive)
                .await
                .map(|_| ())
        }
        "sync" => {
            if args.len() != 1 {
                bail!("sync takes no arguments");
            }
            sync().await
        }
        _ => bail!("unsupported remote command: {command}"),
    }
}

#[derive(Debug)]
struct SubmitReport {
    branch: String,
    url: String,
    created: bool,
    pushed: bool,
    parent_changed: bool,
    published: bool,
}

impl SubmitReport {
    fn action(&self) -> &'static str {
        if self.published {
            return match (self.pushed, self.parent_changed) {
                (true, true) => "Update code, new parent, ready for review",
                (true, false) => "Update code, ready for review",
                (false, true) => "New parent, ready for review",
                (false, false) => "Ready for review",
            };
        }
        match (self.created, self.pushed, self.parent_changed) {
            (true, _, _) => "Create",
            (false, true, true) => "Update code, new parent",
            (false, true, false) => "Update code",
            (false, false, true) => "New parent",
            (false, false, false) => "No-op",
        }
    }

    fn line(&self, color: bool) -> String {
        let code = if self.action() == "No-op" { "2" } else { "36" };
        let branch = submit_ui::paint(&self.branch, code, color);
        let url = submit_ui::paint(&self.url, "94", color);
        format!("▸ {branch} ({})\n  {url}", self.action())
    }
}

async fn submit(
    repo: &Repository,
    mut state: State,
    index: usize,
    client: &GitHub,
    options: SubmitOptions,
    interactive: bool,
) -> Result<Vec<SubmitReport>> {
    let stack = &state.stacks[index];
    let current = repo.current().await?;
    let end = if options.stack {
        stack.branches.len()
    } else {
        stack
            .branches
            .iter()
            .position(|b| b.name == current)
            .context("checkout a tracked stack branch before submitting")?
            + 1
    };
    let mut metadata = BTreeMap::new();
    let mut drafts = BTreeMap::new();
    let mut snapshots = BTreeMap::new();
    for name in std::iter::once(&stack.trunk)
        .chain(stack.branches[..end].iter().map(|b| &b.name))
    {
        let oid = repo
            .git(&["rev-parse", &format!("refs/heads/{name}")])
            .await?
            .trim()
            .to_owned();
        snapshots.insert(name.clone(), oid);
    }
    let mut parent = snapshots[&stack.trunk].clone();
    for branch in &stack.branches[..end] {
        let head = &snapshots[&branch.name];
        repo.git(&["merge-base", "--is-ancestor", &parent, head])
            .await
            .with_context(|| format!("{} needs sk restack", branch.name))?;
        let range = format!("{parent}..{head}");
        let messages = repo
            .git(&["log", "--reverse", "--format=%B%x00", &range, "--"])
            .await?;
        let messages: Vec<String> = messages
            .split('\0')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        let content = commit_metadata(&messages)
            .with_context(|| format!("empty branch {}", branch.name))?;
        metadata.insert(branch.name.clone(), content);
        parent = head.clone();
    }
    for branch in &mut state.stacks[index].branches[..end] {
        if let Some(cached) = &branch.pr {
            if let Some(pr) = client.pull(cached.number).await? {
                if pr.merged_at.is_some() || pr.state == "closed" {
                    bail!("{} PR is merged/closed; run sk sync", branch.name);
                }
                branch.pr = Some(pr.cache());
            } else {
                branch.pr = None;
            }
        }
        if branch.pr.is_none() {
            branch.pr =
                client.find_open(&branch.name).await?.map(|p| p.cache());
        }
        if branch.pr.is_none() {
            let (title, body) = &metadata[&branch.name];
            let (content, draft) = submit_ui::prepare(
                &repo.root,
                &branch.name,
                title,
                body,
                &options,
                interactive,
            )
            .await?;
            metadata.insert(branch.name.clone(), content);
            drafts.insert(branch.name.clone(), draft);
        }
    }
    let stack = &state.stacks[index];
    let mut lookup = vec![
        "ls-remote".to_owned(),
        "--heads".into(),
        "--".into(),
        stack.remote.clone(),
    ];
    lookup.extend(
        stack.branches[..end]
            .iter()
            .map(|b| format!("refs/heads/{}", b.name)),
    );
    let lookup: Vec<&str> = lookup.iter().map(String::as_str).collect();
    let observed = repo.git(&lookup).await?;
    let observed: BTreeMap<_, _> = observed
        .lines()
        .filter_map(|line| {
            let (oid, name) = line.split_once('\t')?;
            Some((name.to_owned(), oid.to_owned()))
        })
        .collect();
    for (name, expected) in &snapshots {
        let actual = repo
            .git(&["rev-parse", &format!("refs/heads/{name}")])
            .await?;
        if actual.trim() != expected {
            bail!("{name} changed while preparing submit; retry");
        }
    }
    let mut changed = BTreeSet::new();
    let mut args = vec!["push".to_owned(), "--atomic".into()];
    for branch in &stack.branches[..end] {
        let reference = format!("refs/heads/{}", branch.name);
        let remote = observed.get(&reference);
        if remote != branch.remote_oid.as_ref()
            && remote != snapshots.get(&branch.name)
        {
            bail!("{} changed on remote; run sk sync", branch.name);
        }
        if remote == snapshots.get(&branch.name) {
            continue;
        }
        changed.insert(branch.name.clone());
        let expected = remote.map(String::as_str).unwrap_or_default();
        args.push(format!("--force-with-lease={reference}:{expected}"));
    }
    args.push("--".into());
    args.push(stack.remote.clone());
    for branch in &stack.branches[..end] {
        if !changed.contains(&branch.name) {
            continue;
        }
        args.push(format!(
            "{}:refs/heads/{}",
            snapshots[&branch.name], branch.name
        ));
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    if !changed.is_empty() {
        repo.git(&refs)
            .await
            .context("atomic push failed; run sk sync")?;
    }
    for branch in &mut state.stacks[index].branches[..end] {
        branch.remote_oid = Some(snapshots[&branch.name].clone());
        branch.fetched_oid = branch.remote_oid.clone();
    }
    repo.save(&state).await?;
    let mut reports = Vec::new();
    let mut parent = state.stacks[index].trunk.clone();
    for position in 0..end {
        let branch = state.stacks[index].branches[position].clone();
        let allowed: Vec<u64> = state.stacks[index].branches[..end]
            .iter()
            .filter_map(|b| b.pr.as_ref().map(|p| p.number))
            .collect();
        let mut created = false;
        let mut parent_changed = false;
        let mut pr = match branch.pr {
            Some(pr) => {
                parent_changed =
                    client.update_base(pr.number, &parent, &allowed).await?;
                pr
            }
            None => {
                // Rediscovery also handles a prior timed-out create response.
                match client.find_open(&branch.name).await? {
                    Some(pr) => {
                        parent_changed = client
                            .update_base(pr.number, &parent, &allowed)
                            .await?;
                        pr.cache()
                    }
                    None => {
                        let (title, body) = &metadata[&branch.name];
                        created = true;
                        client
                            .create_pull(
                                &branch.name,
                                &parent,
                                title,
                                body,
                                drafts[&branch.name],
                            )
                            .await?
                            .cache()
                    }
                }
            }
        };
        let published = if options.publish && pr.draft {
            let changed = client.publish_pull(pr.number).await?;
            pr.draft = false;
            changed
        } else {
            false
        };
        let report = SubmitReport {
            branch: branch.name.clone(),
            url: pr.url.clone(),
            created,
            pushed: changed.contains(&branch.name),
            parent_changed,
            published,
        };
        pr.head_oid = snapshots[&branch.name].clone();
        state.stacks[index].branches[position].pr = Some(pr);
        repo.save(&state).await?;
        println!(
            "{}",
            report.line(submit_ui::color(std::io::stdout().is_terminal()))
        );
        reports.push(report);
        parent = branch.name;
    }
    let numbers: Vec<u64> = state.stacks[index].branches[..end]
        .iter()
        .filter_map(|b| b.pr.as_ref().map(|p| p.number))
        .collect();
    state.stacks[index].github_number = client
        .publish_stack(&numbers, state.stacks[index].github_number)
        .await?;
    repo.save(&state).await?;
    Ok(reports)
}

async fn sync_transform(
    repo: &Repository,
    stack: &Stack,
) -> Result<(Stack, BTreeMap<String, String>, Vec<String>)> {
    core::foreign_worktrees(repo, stack).await?;
    let mut result = stack.clone();
    result.branches.clear();
    let mut heads = BTreeMap::new();
    let mut notes = Vec::new();
    for (position, original) in stack.branches.iter().enumerate() {
        let mut branch = original.clone();
        let local = repo.git(&["rev-parse", &branch.name]).await?;
        let local = local.trim().to_owned();
        if let Some(pr) = &branch.pr
            && (pr.state == "MERGED" || pr.state == "CLOSED")
        {
            if pr.state == "CLOSED"
                && stack.branches[position + 1..]
                    .iter()
                    .any(|b| b.pr.as_ref().is_none_or(|p| p.state == "OPEN"))
            {
                bail!(
                    "{} is closed but has active descendants; \
                         restructure it before cleanup",
                    branch.name
                );
            }
            if local == pr.head_oid {
                notes.push(format!("Pruned {} ({})", branch.name, pr.state));
                continue;
            }
            if pr.state == "CLOSED" {
                bail!("{} has unpublished work after closure", branch.name);
            }
            repo.git(&["merge-base", "--is-ancestor", &pr.head_oid, &local])
                .await
                .with_context(|| {
                    format!(
                        "Cannot identify merged boundary for {}",
                        branch.name
                    )
                })?;
            let date = chrono::Local::now().format("%m-%d");
            let suffix =
                format!("{}_{}-follow-up", date, core::slug(&branch.name));
            let mut name = suffix.clone();
            let mut number = 2;
            while repo
                .git(&["show-ref", "--verify", &format!("refs/heads/{name}")])
                .await
                .is_ok()
                || result.branches.iter().any(|b| b.name == name)
            {
                name = format!("{suffix}-{number}");
                number += 1;
            }
            notes.push(format!(
                "Moved unpublished {} work to {name}",
                branch.name
            ));
            branch.base = pr.head_oid.clone();
            branch.name = name;
            branch.pr = None;
            branch.remote_oid = None;
            branch.fetched_oid = None;
            heads.insert(branch.name.clone(), local);
            result.branches.push(branch);
            continue;
        }
        if let Some(fetched) = &branch.fetched_oid
            && fetched != &local
        {
            let local_unchanged = branch.remote_oid.as_ref() == Some(&local);
            let remote_ahead = repo
                .git(&["merge-base", "--is-ancestor", &local, fetched])
                .await
                .is_ok();
            let local_ahead = repo
                .git(&["merge-base", "--is-ancestor", fetched, &local])
                .await
                .is_ok();
            if local_unchanged || remote_ahead {
                // GitHub can rewrite a surviving PR onto the squashed trunk.
                // Its old parent boundary no longer belongs to this history.
                if repo
                    .git(&[
                        "merge-base",
                        "--is-ancestor",
                        &branch.base,
                        fetched,
                    ])
                    .await
                    .is_err()
                {
                    let parent = if let Some(parent) = result.branches.last() {
                        match heads
                            .get(&parent.name)
                            .or(parent.fetched_oid.as_ref())
                        {
                            Some(oid) => oid.clone(),
                            None => repo
                                .git(&["rev-parse", &parent.name])
                                .await?
                                .trim()
                                .to_owned(),
                        }
                    } else {
                        let remote_trunk = format!(
                            "refs/remotes/{}/{}",
                            stack.remote, stack.trunk,
                        );
                        repo.git(&["rev-parse", "--verify", &remote_trunk])
                            .await
                            .or(repo.git(&["rev-parse", &stack.trunk]).await)?
                            .trim()
                            .to_owned()
                    };
                    branch.base = repo
                        .git(&["merge-base", &parent, fetched])
                        .await
                        .context("Cannot identify remote branch boundary")?
                        .trim()
                        .to_owned();
                }
                heads.insert(branch.name.clone(), fetched.clone());
            } else if !local_ahead {
                bail!("{} diverged; run sk restack", branch.name);
            }
        }
        branch.remote_oid = branch.fetched_oid.clone();
        result.branches.push(branch);
    }
    Ok((result, heads, notes))
}

async fn refresh_stack(
    repo: &Repository,
    stack: &Stack,
    client: &GitHub,
) -> Result<Stack> {
    let branches = stream::iter(stack.branches.iter().cloned().map(|mut b| {
        let client = client.clone();
        let remote = stack.remote.clone();
        async move {
            let pr = match &b.pr {
                Some(pr) => client
                    .pull(pr.number)
                    .await?
                    .with_context(|| {
                        format!("PR #{} is unavailable", pr.number)
                    })?
                    .into(),
                None => client.find_open(&b.name).await?,
            };
            b.pr = pr.map(|p: Pull| p.cache());
            let reference = format!("refs/remotes/{remote}/{}", b.name);
            b.fetched_oid = repo
                .git(&["rev-parse", "--verify", &reference])
                .await
                .ok()
                .map(|oid| oid.trim().to_owned());
            Ok::<_, anyhow::Error>(b)
        }
    }))
    .buffered(8)
    .collect::<Vec<_>>()
    .await;
    let mut result = stack.clone();
    result.branches = branches.into_iter().collect::<Result<Vec<_>>>()?;
    Ok(result)
}

async fn trunk_candidate(repo: &Repository, stack: &Stack) -> Result<String> {
    let reference = format!("refs/remotes/{}/{}", stack.remote, stack.trunk);
    let remote = repo
        .git(&["rev-parse", "--verify", &reference])
        .await?
        .trim()
        .to_owned();
    let local = repo
        .git(&["rev-parse", &stack.trunk])
        .await?
        .trim()
        .to_owned();
    if local != remote {
        repo.git(&["merge-base", "--is-ancestor", &local, &remote])
            .await
            .with_context(|| {
                format!(
                    "Trunk {} cannot fast-forward; reconcile it first",
                    stack.trunk
                )
            })?;
        let listing = repo.git(&["worktree", "list", "--porcelain"]).await?;
        let mut path = "";
        for line in listing.lines() {
            if let Some(value) = line.strip_prefix("worktree ") {
                path = value;
            }
            if line == format!("branch refs/heads/{}", stack.trunk)
                && std::path::Path::new(path) != repo.root
            {
                bail!("Trunk {} is checked out in {path}", stack.trunk);
            }
        }
    }
    Ok(remote)
}

async fn sync() -> Result<()> {
    let repo = Repository::discover().await?;
    let _lock = repo.lock()?;
    repo.ensure_idle().await?;
    repo.clean().await?;
    let mut state = repo.load().await?;
    if state.stacks.is_empty() {
        println!("No tracked stacks; start one with sk create");
        return Ok(());
    }
    core::snapshot_state(&repo, &state).await?;
    let remotes: BTreeSet<_> =
        state.stacks.iter().map(|s| s.remote.clone()).collect();
    let mut clients = BTreeMap::new();
    for remote in &remotes {
        clients.insert(remote.clone(), api(&repo, remote).await?);
    }
    sync_ui::phase("🌲 Fetching branches from remote...");
    let fetched = stream::iter(remotes.iter().map(|remote| {
        let repo = &repo;
        async move {
            repo.git(&["fetch", "--prune", "--no-tags", "--", remote])
                .await
                .with_context(|| format!("Failed fetching {remote}"))
        }
    }))
    .buffer_unordered(4)
    .collect::<Vec<_>>()
    .await;
    for result in fetched {
        result?;
    }
    sync_ui::phase("🔎 Checking pull requests...");
    let refreshed =
        stream::iter(state.stacks.iter().enumerate().map(|(index, stack)| {
            let repo = &repo;
            let client = &clients[&stack.remote];
            async move { (index, refresh_stack(repo, stack, client).await) }
        }))
        .buffer_unordered(4)
        .collect::<Vec<_>>()
        .await;
    let mut failures = Vec::new();
    let mut ready = Vec::new();
    for (index, result) in refreshed {
        match result {
            Ok(stack) => {
                state.stacks[index] = stack;
                ready.push(index);
            }
            Err(error) => {
                failures.push(format!("{}: {error:#}", state.stacks[index].id))
            }
        }
    }
    // Cache remote observations even if a local stack cannot be rewritten.
    repo.save(&state).await?;
    sync_ui::phase("🔄 Syncing and restacking branches...");
    let proposals = stream::iter(ready.into_iter().map(|index| {
        let stack = state.stacks[index].clone();
        let repo = &repo;
        async move {
            let result = async {
                let (proposed, mut heads, notes) =
                    sync_transform(repo, &stack).await?;
                let trunk = trunk_candidate(repo, &stack).await?;
                heads.insert(stack.trunk.clone(), trunk.clone());
                let mut output =
                    core::plan_stack(repo, &proposed, &heads).await?;
                output.insert(stack.trunk.clone(), trunk);
                Ok::<_, anyhow::Error>((proposed, output, notes))
            }
            .await;
            (index, result)
        }
    }))
    .buffer_unordered(4)
    .collect::<Vec<_>>()
    .await;
    let mut proposals = proposals;
    proposals.sort_by_key(|(index, _)| *index);
    for (index, proposal) in proposals {
        match proposal {
            Ok((stack, heads, notes)) => {
                let mut proposed = state.clone();
                proposed.stacks[index] = stack;
                let mut before = BTreeMap::new();
                for name in std::iter::once(&state.stacks[index].trunk)
                    .chain(state.stacks[index].branches.iter().map(|b| &b.name))
                {
                    before.insert(
                        name.clone(),
                        repo.git(&["rev-parse", &format!("refs/heads/{name}")])
                            .await?
                            .trim()
                            .to_owned(),
                    );
                }
                let color = submit_ui::color(std::io::stdout().is_terminal());
                let lines = sync_ui::report(
                    &state.stacks[index],
                    &proposed.stacks[index],
                    &before,
                    &heads,
                    color,
                );
                core::apply_plan(&repo, &state, &proposed, &heads).await?;
                state = repo.load().await?;
                for line in lines {
                    println!("{line}");
                }
                for note in
                    notes.into_iter().filter(|n| !n.starts_with("Pruned "))
                {
                    println!("{}", submit_ui::paint(&note, "33", color));
                }
            }
            Err(error) => {
                failures.push(format!("{}: {error:#}", state.stacks[index].id))
            }
        }
    }
    // Empty records are retained during indexed processing, then removed.
    state.stacks.retain(|s| !s.branches.is_empty());
    repo.save(&state).await?;
    if !failures.is_empty() {
        for failure in &failures {
            eprintln!(
                "{}",
                submit_ui::paint(
                    &format!("Skipped {failure}"),
                    "33",
                    submit_ui::color(std::io::stderr().is_terminal()),
                )
            );
        }
        bail!("{} stack(s) need attention", failures.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lookup_reuses_existing_pr_and_encodes_branch() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener =
            tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = vec![0; 8192];
            let n = socket.read(&mut buffer).await.unwrap();
            let request = String::from_utf8_lossy(&buffer[..n]);
            assert!(request.contains("state=open"));
            assert!(request.contains("head=owner%3Afeat%2Fone"));
            let body = r#"[{"number":7,"html_url":"https://github.com/owner/repo/pull/7","state":"open","draft":true,"merged_at":null,"head":{"sha":"abc","ref":"feat/one"}}]"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let api = GitHub::new(
            &format!("http://{address}"),
            "owner/repo",
            "test".into(),
        );
        let pull = api.find_open("feat/one").await.unwrap().unwrap();
        assert_eq!(pull.number, 7);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn publishing_checks_graphql_errors_and_confirmation() {
        for response in [
            json!({"errors":[{"message":"forbidden"}]}),
            json!({"data":{"markPullRequestReadyForReview":{
                "pullRequest":{"isDraft":true}
            }}}),
        ] {
            let (base, server) = mock_sequence(vec![
                (
                    "POST /graphql ",
                    200,
                    json!({"data":{"repository":{
                        "pullRequest":{"id":"PR_7","isDraft":true}
                    }}}),
                ),
                ("POST /graphql ", 200, response),
            ])
            .await;
            let api = GitHub::new(&base, "o/r", "test".into());
            assert!(api.publish_pull(7).await.is_err());
            server.await.unwrap();
        }
        let (base, server) = mock_sequence(vec![(
            "POST /graphql ",
            200,
            json!({"data":{"repository":{
                "pullRequest":{"id":"PR_7","isDraft":false}
            }}}),
        )])
        .await;
        assert!(
            !GitHub::new(&base, "o/r", "test".into())
                .publish_pull(7)
                .await
                .unwrap()
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn publish_flag_skips_draft_prompt_and_overrides_draft() {
        let options = submit_options(&["-snpd".into()]).unwrap();
        assert!(options.publish && options.stack && options.no_edit);
        assert!(submit_options(&["--publish".into()]).unwrap().publish);
        let dir = tempfile::tempdir().unwrap();
        let (_, draft) = submit_ui::prepare(
            dir.path(),
            "branch",
            "title",
            "body",
            &options,
            true,
        )
        .await
        .unwrap();
        assert!(!draft);
    }

    #[test]
    fn combined_submit_flags_and_remote_urls() {
        assert_eq!(
            submit_options(&["-sdn".into()]).unwrap(),
            SubmitOptions {
                stack: true,
                draft: true,
                no_edit: true,
                publish: false,
            }
        );
        assert!(!submit_options(&[]).unwrap().stack);
        assert!(
            !submit_options(&["--stack".into(), "--no-stack".into()])
                .unwrap()
                .stack
        );
        assert!(submit_options(&["--force".into()]).is_err());
        assert_eq!(
            github_repository("git@github.com:owner/repo.git").unwrap(),
            "owner/repo"
        );
        assert!(github_repository("https://evil.test/owner/repo").is_err());
    }

    pub(super) async fn fixture() -> (tempfile::TempDir, Repository) {
        let dir = tempfile::tempdir().unwrap();
        let output = Command::new("git")
            .args(["init", "-b", "main"])
            .arg(dir.path())
            .output()
            .await
            .unwrap();
        assert!(output.status.success());
        let repo = Repository::at(dir.path()).await.unwrap();
        repo.git(&["config", "user.name", "Test"]).await.unwrap();
        repo.git(&["config", "user.email", "test@example.invalid"])
            .await
            .unwrap();
        repo.git(&["config", "commit.gpgSign", "false"])
            .await
            .unwrap();
        repo.git(&["commit", "--allow-empty", "-m", "root"])
            .await
            .unwrap();
        (dir, repo)
    }

    pub(super) async fn add_commit(
        repo: &Repository,
        file: &str,
        content: &str,
    ) -> String {
        tokio::fs::write(repo.root.join(file), content)
            .await
            .unwrap();
        repo.git(&["add", file]).await.unwrap();
        repo.git(&["commit", "-m", file]).await.unwrap();
        repo.git(&["rev-parse", "HEAD"])
            .await
            .unwrap()
            .trim()
            .into()
    }

    #[tokio::test]
    async fn merged_extra_work_becomes_followup_without_moving_refs() {
        let (_dir, repo) = fixture().await;
        let root = repo.git(&["rev-parse", "HEAD"]).await.unwrap();
        repo.git(&["switch", "-c", "a"]).await.unwrap();
        let merged = add_commit(&repo, "a.txt", "merged").await;
        let extra = add_commit(&repo, "extra.txt", "unpublished").await;
        let branch = core::Branch {
            name: "a".into(),
            base: root.trim().into(),
            remote_oid: Some(merged.clone()),
            fetched_oid: None,
            pr: Some(PullRequest {
                number: 1,
                url: "https://github.com/o/r/pull/1".into(),
                state: "MERGED".into(),
                draft: false,
                head_oid: merged.clone(),
            }),
        };
        let stack = Stack {
            id: "a".into(),
            trunk: "main".into(),
            remote: "origin".into(),
            branches: vec![branch],
            github_number: None,
        };
        let (proposed, heads, _) = sync_transform(&repo, &stack).await.unwrap();
        assert_eq!(proposed.branches.len(), 1);
        let followup = &proposed.branches[0];
        assert!(followup.name.contains("a-follow-up"));
        assert_eq!(followup.base, merged);
        assert!(followup.pr.is_none());
        assert_eq!(heads[&followup.name], extra);
        assert_eq!(repo.git(&["rev-parse", "a"]).await.unwrap().trim(), extra);
        assert!(
            repo.git(&[
                "show-ref",
                "--verify",
                &format!("refs/heads/{}", followup.name)
            ])
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn divergent_stack_is_reported_without_mutation() {
        let (_dir, repo) = fixture().await;
        let root = repo.git(&["rev-parse", "HEAD"]).await.unwrap();
        repo.git(&["switch", "-c", "a"]).await.unwrap();
        let baseline = add_commit(&repo, "a.txt", "baseline").await;
        let local = add_commit(&repo, "local.txt", "local").await;
        repo.git(&["switch", "-c", "remote", &baseline])
            .await
            .unwrap();
        let remote = add_commit(&repo, "remote.txt", "remote").await;
        repo.git(&["switch", "a"]).await.unwrap();
        let stack = Stack {
            id: "a".into(),
            trunk: "main".into(),
            remote: "origin".into(),
            branches: vec![core::Branch {
                name: "a".into(),
                base: root.trim().into(),
                remote_oid: Some(baseline),
                fetched_oid: Some(remote),
                pr: None,
            }],
            github_number: None,
        };
        assert!(sync_transform(&repo, &stack).await.is_err());
        assert_eq!(repo.git(&["rev-parse", "a"]).await.unwrap().trim(), local);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn submit_reports_real_actions_and_skips_noop_push() {
        use std::os::unix::fs::PermissionsExt;
        for action in [
            "Create",
            "Update code",
            "No-op",
            "New parent",
            "Update code, new parent",
            "Ready for review",
        ] {
            let (_dir, repo) = fixture().await;
            let bare = tempfile::tempdir().unwrap();
            assert!(
                Command::new("git")
                    .args(["init", "--bare"])
                    .arg(bare.path())
                    .output()
                    .await
                    .unwrap()
                    .status
                    .success()
            );
            repo.git(&[
                "remote",
                "add",
                "origin",
                bare.path().to_str().unwrap(),
            ])
            .await
            .unwrap();
            let root = repo.git(&["rev-parse", "main"]).await.unwrap();
            repo.git(&["switch", "-c", "feature"]).await.unwrap();
            let old = add_commit(&repo, "a.txt", "A").await;
            if action != "Create" {
                repo.git(&["push", "origin", "feature"]).await.unwrap();
            }
            if action.starts_with("Update code") {
                add_commit(&repo, "b.txt", "B").await;
            }
            let hook = repo.common.join("hooks/pre-push");
            std::fs::write(&hook, "#!/bin/sh\nprintf ran > .git/push-ran\n")
                .unwrap();
            std::fs::set_permissions(
                &hook,
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
            let mut pr =
                api_pull(if action.to_lowercase().contains("parent") {
                    "old-parent"
                } else {
                    "main"
                });
            pr["head"]["sha"] = json!(old);
            pr["draft"] = json!(matches!(action, "Ready for review" | "No-op"));
            let mut replies = if action == "Create" {
                vec![
                    ("GET /repos/o/r/pulls?", 200, json!([])),
                    ("GET /repos/o/r/pulls?", 200, json!([])),
                    ("POST /repos/o/r/pulls ", 200, pr.clone()),
                ]
            } else {
                vec![
                    ("GET /repos/o/r/pulls/7 ", 200, pr.clone()),
                    ("GET /repos/o/r/pulls/7 ", 200, pr.clone()),
                ]
            };
            if action.to_lowercase().contains("parent") {
                replies.extend([
                    ("GET /repos/o/r/stacks?", 200, json!([])),
                    ("PATCH /repos/o/r/pulls/7 ", 200, pr.clone()),
                ]);
            }
            if action == "Ready for review" {
                replies.extend([
                    (
                        "POST /graphql ",
                        200,
                        json!({"data": {"repository": {
                            "pullRequest": {"id": "PR_7", "isDraft": true}
                        }}}),
                    ),
                    (
                        "POST /graphql ",
                        200,
                        json!({"data": {
                            "markPullRequestReadyForReview": {
                                "pullRequest": {"isDraft": false}
                            }
                        }}),
                    ),
                ]);
            }
            let state = State {
                version: 1,
                stacks: vec![Stack {
                    id: "feature".into(),
                    trunk: "main".into(),
                    remote: "origin".into(),
                    github_number: None,
                    branches: vec![core::Branch {
                        name: "feature".into(),
                        base: root.trim().into(),
                        remote_oid: (action != "Create").then(|| old.clone()),
                        fetched_oid: None,
                        pr: (action != "Create").then(|| {
                            serde_json::from_value::<Pull>(pr).unwrap().cache()
                        }),
                    }],
                }],
            };
            let (base, server) = mock_sequence(replies).await;
            let api = GitHub::new(&base, "o/r", "test".into());
            let reports = submit(
                &repo,
                state,
                0,
                &api,
                SubmitOptions {
                    no_edit: true,
                    publish: action == "Ready for review",
                    ..Default::default()
                },
                false,
            )
            .await
            .unwrap();
            assert_eq!(reports[0].action(), action);
            assert_eq!(
                repo.load().await.unwrap().stacks[0].branches[0]
                    .pr
                    .as_ref()
                    .unwrap()
                    .draft,
                action == "No-op",
            );
            assert_eq!(
                reports[0].line(false),
                format!(
                    "▸ feature ({action})\n  https://github.com/o/r/pull/7"
                )
            );
            assert_eq!(
                repo.common.join("push-ran").exists(),
                action == "Create" || action.starts_with("Update code")
            );
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn submission_retry_reuses_pr_created_before_server_error() {
        check_submission_scope(false).await;
    }

    #[tokio::test]
    async fn stack_submission_pushes_descendants() {
        check_submission_scope(true).await;
    }

    async fn check_submission_scope(whole_stack: bool) {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        };
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (_dir, repo) = fixture().await;
        let bare = tempfile::tempdir().unwrap();
        let output = Command::new("git")
            .args(["init", "--bare"])
            .arg(bare.path())
            .output()
            .await
            .unwrap();
        assert!(output.status.success());
        repo.git(&["remote", "add", "origin", bare.path().to_str().unwrap()])
            .await
            .unwrap();
        let root = repo.git(&["rev-parse", "main"]).await.unwrap();
        repo.git(&["switch", "-c", "feature"]).await.unwrap();
        let head = add_commit(&repo, "feature.txt", "feature").await;
        let mut state = State {
            version: 1,
            stacks: vec![Stack {
                id: "feature".into(),
                trunk: "main".into(),
                remote: "origin".into(),
                branches: vec![core::Branch {
                    name: "feature".into(),
                    base: root.trim().into(),
                    remote_oid: None,
                    fetched_oid: None,
                    pr: None,
                }],
                github_number: None,
            }],
        };
        repo.git(&["switch", "-c", "descendant"]).await.unwrap();
        add_commit(&repo, "descendant.txt", "descendant").await;
        state.stacks[0].branches.push(core::Branch {
            name: "descendant".into(),
            base: head.clone(),
            remote_oid: None,
            fetched_oid: None,
            pr: None,
        });
        repo.git(&["switch", "feature"]).await.unwrap();
        repo.save(&state).await.unwrap();
        std::fs::create_dir(repo.root.join(".github")).unwrap();
        std::fs::write(
            repo.root.join(".github/PULL_REQUEST_TEMPLATE.md"),
            "## Template body",
        )
        .unwrap();
        let listener =
            tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let created = Arc::new(AtomicBool::new(false));
        let creates = Arc::new(AtomicUsize::new(0));
        let create_count = creates.clone();
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut buf = [0; 4096];
                    let n = socket.read(&mut buf).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&buf[..n]);
                    if let Some(end) =
                        request.windows(4).position(|w| w == b"\r\n\r\n")
                    {
                        let header = String::from_utf8_lossy(&request[..end]);
                        let len = header
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| {
                                        value.trim().parse::<usize>().unwrap()
                                    })
                            })
                            .unwrap_or(0);
                        if request.len() >= end + 4 + len {
                            break;
                        }
                    }
                }
                let request = String::from_utf8(request).unwrap();
                let pull = json!({
                    "number": 42, "html_url": "https://github.com/o/r/pull/42",
                    "state": "open", "draft": true, "merged_at": null,
                    "head": {"sha": head, "ref": "feature"},
                    "base": {"sha": "root", "ref": "main"},
                });
                let (status, body) = if request.starts_with("POST ") {
                    create_count.fetch_add(1, Ordering::SeqCst);
                    created.store(true, Ordering::SeqCst);
                    assert!(request.contains("\"draft\":true"));
                    assert!(request.contains("\"body\":\"## Template body\""));
                    ("500 Internal Server Error", json!({"message":"retry"}))
                } else if request.starts_with("PATCH ") {
                    assert!(request.contains("\"base\":\"main\""));
                    ("200 OK", pull)
                } else if request.starts_with("GET /repos/o/r/pulls/42 ") {
                    ("200 OK", pull)
                } else if created.load(Ordering::SeqCst) {
                    ("200 OK", json!([pull]))
                } else {
                    ("200 OK", json!([]))
                };
                let body = body.to_string();
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\n\
                     Content-Type: application/json\r\n\
                     Connection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let api =
            GitHub::new(&format!("http://{address}"), "o/r", "test".into());
        let options = || SubmitOptions {
            stack: whole_stack,
            draft: true,
            publish: false,
            no_edit: true,
        };
        assert!(
            submit(&repo, state, 0, &api, options(), false)
                .await
                .is_err()
        );
        let retry_state = repo.load().await.unwrap();
        assert!(retry_state.stacks[0].branches[0].remote_oid.is_some());
        if whole_stack {
            assert!(retry_state.stacks[0].branches[1].remote_oid.is_some());
            assert!(
                !repo
                    .git(&["ls-remote", "origin", "refs/heads/descendant"])
                    .await
                    .unwrap()
                    .trim()
                    .is_empty()
            );
            server.abort();
            return;
        }
        let reports = submit(&repo, retry_state, 0, &api, options(), false)
            .await
            .unwrap();
        assert_eq!(reports[0].action(), "No-op");
        assert_eq!(creates.load(Ordering::SeqCst), 1);
        assert_eq!(
            repo.load().await.unwrap().stacks[0].branches[0]
                .pr
                .as_ref()
                .unwrap()
                .number,
            42
        );
        let saved = repo.load().await.unwrap();
        assert_eq!(saved.stacks[0].branches.len(), 2);
        assert!(saved.stacks[0].branches[1].remote_oid.is_none());
        assert!(saved.stacks[0].branches[1].pr.is_none());
        assert!(
            repo.git(&["ls-remote", "origin", "refs/heads/descendant"])
                .await
                .unwrap()
                .trim()
                .is_empty()
        );
        server.abort();
    }

    #[tokio::test]
    async fn sync_adopts_rebased_survivor_after_squash_merging_parent() {
        let (_dir, repo) = fixture().await;
        let root = repo.git(&["rev-parse", "main"]).await.unwrap();
        repo.git(&["switch", "-c", "a"]).await.unwrap();
        let a = add_commit(&repo, "a.txt", "A").await;
        repo.git(&["switch", "-c", "b"]).await.unwrap();
        let b = add_commit(&repo, "b.txt", "B").await;
        repo.git(&["switch", "main"]).await.unwrap();
        repo.git(&["merge", "--squash", "a"]).await.unwrap();
        repo.git(&["commit", "-m", "Squashed parent"])
            .await
            .unwrap();
        let trunk = repo
            .git(&["rev-parse", "HEAD"])
            .await
            .unwrap()
            .trim()
            .to_owned();
        repo.git(&["switch", "-c", "remote-survivor"])
            .await
            .unwrap();
        repo.git(&["cherry-pick", &b]).await.unwrap();
        let fetched = repo
            .git(&["rev-parse", "HEAD"])
            .await
            .unwrap()
            .trim()
            .to_owned();
        repo.git(&["switch", "main"]).await.unwrap();
        repo.git(&["update-ref", "refs/remotes/origin/main", &trunk])
            .await
            .unwrap();
        let stack = Stack {
            id: "a".into(),
            trunk: "main".into(),
            remote: "origin".into(),
            github_number: None,
            branches: vec![
                core::Branch {
                    name: "a".into(),
                    base: root.trim().into(),
                    remote_oid: Some(a.clone()),
                    fetched_oid: Some(a.clone()),
                    pr: Some(PullRequest {
                        number: 1,
                        url: "url".into(),
                        state: "MERGED".into(),
                        draft: false,
                        head_oid: a.clone(),
                    }),
                },
                core::Branch {
                    name: "b".into(),
                    base: a.clone(),
                    remote_oid: Some(b.clone()),
                    fetched_oid: Some(fetched.clone()),
                    pr: Some(PullRequest {
                        number: 2,
                        url: "url".into(),
                        state: "OPEN".into(),
                        draft: false,
                        head_oid: fetched.clone(),
                    }),
                },
            ],
        };
        let (proposed, mut heads, _) =
            sync_transform(&repo, &stack).await.unwrap();
        assert_eq!(proposed.branches.len(), 1);
        assert_eq!(proposed.branches[0].base, trunk);
        heads.insert("main".into(), trunk.clone());
        let plan = core::plan_stack(&repo, &proposed, &heads).await.unwrap();
        assert_eq!(plan["b"], fetched);
        let original = State {
            version: 1,
            stacks: vec![stack],
        };
        let next = State {
            version: 1,
            stacks: vec![proposed],
        };
        repo.save(&original).await.unwrap();
        core::apply_plan(&repo, &original, &next, &plan)
            .await
            .unwrap();
        assert!(
            repo.git(&["show-ref", "--verify", "refs/heads/a"])
                .await
                .is_err()
        );
        assert_eq!(
            repo.git(&["rev-parse", "b"]).await.unwrap().trim(),
            fetched
        );
        assert_eq!(repo.current().await.unwrap(), "main");
        assert_eq!(repo.git(&["show", "b:b.txt"]).await.unwrap().trim(), "B");
    }

    #[tokio::test]
    async fn closed_ancestor_is_retained_for_active_descendant() {
        let (_dir, repo) = fixture().await;
        let root = repo.git(&["rev-parse", "main"]).await.unwrap();
        repo.git(&["switch", "-c", "a"]).await.unwrap();
        let a = add_commit(&repo, "a.txt", "A").await;
        repo.git(&["switch", "-c", "b"]).await.unwrap();
        let b = add_commit(&repo, "b.txt", "B").await;
        let stack = Stack {
            id: "a".into(),
            trunk: "main".into(),
            remote: "origin".into(),
            branches: vec![
                core::Branch {
                    name: "a".into(),
                    base: root.trim().into(),
                    remote_oid: Some(a.clone()),
                    fetched_oid: Some(a.clone()),
                    pr: Some(PullRequest {
                        number: 1,
                        url: "https://github.com/o/r/pull/1".into(),
                        state: "CLOSED".into(),
                        draft: false,
                        head_oid: a.clone(),
                    }),
                },
                core::Branch {
                    name: "b".into(),
                    base: a.clone(),
                    remote_oid: None,
                    fetched_oid: None,
                    pr: None,
                },
            ],
            github_number: None,
        };
        let error = sync_transform(&repo, &stack).await.unwrap_err();
        assert!(error.to_string().contains("active descendants"));
        assert_eq!(repo.git(&["rev-parse", "a"]).await.unwrap().trim(), a);
        assert_eq!(repo.git(&["rev-parse", "b"]).await.unwrap().trim(), b);
    }

    #[tokio::test]
    async fn conflict_after_clean_parent_keeps_entire_stack_unchanged() {
        let (_dir, repo) = fixture().await;
        let root = repo.git(&["rev-parse", "main"]).await.unwrap();
        repo.git(&["switch", "-c", "a"]).await.unwrap();
        let a = add_commit(&repo, "a.txt", "A").await;
        repo.git(&["switch", "-c", "b"]).await.unwrap();
        let b = add_commit(&repo, "conflict.txt", "ours").await;
        repo.git(&["switch", "main"]).await.unwrap();
        add_commit(&repo, "conflict.txt", "theirs").await;
        repo.git(&["switch", "b"]).await.unwrap();
        let stack = Stack {
            id: "a".into(),
            trunk: "main".into(),
            remote: "origin".into(),
            branches: vec![
                core::Branch {
                    name: "a".into(),
                    base: root.trim().into(),
                    remote_oid: None,
                    fetched_oid: None,
                    pr: None,
                },
                core::Branch {
                    name: "b".into(),
                    base: a.clone(),
                    remote_oid: None,
                    fetched_oid: None,
                    pr: None,
                },
            ],
            github_number: None,
        };
        let (proposed, heads, _) = sync_transform(&repo, &stack).await.unwrap();
        assert!(core::plan_stack(&repo, &proposed, &heads).await.is_err());
        assert_eq!(repo.git(&["rev-parse", "a"]).await.unwrap().trim(), a);
        assert_eq!(repo.git(&["rev-parse", "b"]).await.unwrap().trim(), b);
        repo.clean().await.unwrap();
    }

    async fn mock_sequence(
        sequence: Vec<(&'static str, u16, Value)>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener =
            tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            for (expected, status, body) in sequence {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = [0; 8192];
                let n = socket.read(&mut buffer).await.unwrap();
                let request = String::from_utf8_lossy(&buffer[..n]);
                assert!(request.starts_with(expected), "{request}");
                let body = if status == 204 {
                    String::new()
                } else {
                    body.to_string()
                };
                let response = format!(
                    "HTTP/1.1 {status} Response\r\nContent-Length: {}\r\n\
                     Content-Type: application/json\r\n\
                     Connection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        (format!("http://{address}"), task)
    }

    fn api_pull(base: &str) -> Value {
        json!({
            "number": 7, "html_url": "https://github.com/o/r/pull/7",
            "state": "open", "draft": true, "merged_at": null,
            "head": {"sha": "abcdef", "ref": "feature"},
            "base": {"sha": "fedcba", "ref": base},
        })
    }

    #[tokio::test]
    async fn unchanged_base_does_not_patch_stacked_pr() {
        let (base, server) = mock_sequence(vec![(
            "GET /repos/o/r/pulls/7 ",
            200,
            api_pull("main"),
        )])
        .await;
        let api = GitHub::new(&base, "o/r", "test".into());
        api.update_base(7, "main", &[7]).await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn narrow_submission_preserves_remote_descendants() {
        let stack = json!({"number": 9, "pull_requests": [
            {"number": 7, "state": "open", "merged_at": null},
            {"number": 8, "state": "open", "merged_at": null},
            {"number": 10, "state": "open", "merged_at": null},
        ]});
        let (base, server) = mock_sequence(vec![(
            "GET /repos/o/r/stacks?",
            200,
            json!([stack]),
        )])
        .await;
        let api = GitHub::new(&base, "o/r", "test".into());
        assert_eq!(api.publish_stack(&[7, 8], Some(9)).await.unwrap(), Some(9));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn stack_creation_rediscovery_after_ambiguous_response() {
        let stack = json!({"number": 9, "pull_requests": [
            {"number": 7, "state": "open", "merged_at": null},
            {"number": 8, "state": "open", "merged_at": null},
        ]});
        let (base, server) = mock_sequence(vec![
            ("GET /repos/o/r/stacks?", 200, json!([])),
            (
                "POST /repos/o/r/stacks ",
                422,
                json!({"message":"Already stacked"}),
            ),
            ("GET /repos/o/r/stacks?", 200, json!([stack])),
        ])
        .await;
        let api = GitHub::new(&base, "o/r", "test".into());
        assert_eq!(api.publish_stack(&[7, 8], None).await.unwrap(), Some(9));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn changing_base_unstacks_before_patch() {
        let stack = json!({"number": 9, "pull_requests": [
            {"number": 7, "state": "open", "merged_at": null},
        ]});
        let (base, server) = mock_sequence(vec![
            ("GET /repos/o/r/pulls/7 ", 200, api_pull("old")),
            ("GET /repos/o/r/stacks?", 200, json!([stack])),
            ("POST /repos/o/r/stacks/9/unstack ", 204, Value::Null),
            ("PATCH /repos/o/r/pulls/7 ", 200, api_pull("main")),
        ])
        .await;
        let api = GitHub::new(&base, "o/r", "test".into());
        api.update_base(7, "main", &[7]).await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn template_replaces_generated_body_and_preserves_title() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(
            submit_ui::default_body(root, "commit body").await.unwrap(),
            "commit body"
        );
        std::fs::create_dir(root.join(".github")).unwrap();
        std::fs::write(
            root.join(".github/PULL_REQUEST_TEMPLATE.md"),
            "## Why\n\n## Testing\n",
        )
        .unwrap();
        let body = submit_ui::default_body(root, "commit body").await.unwrap();
        assert_eq!(body, "## Why\n\n## Testing\n");
        let (title, body) =
            parse_pr_document(&format!("Commit title\n\n{body}")).unwrap();
        assert_eq!(title, "Commit title");
        assert_eq!(body, "## Why\n\n## Testing");
        std::fs::remove_file(root.join(".github/PULL_REQUEST_TEMPLATE.md"))
            .unwrap();
        std::fs::create_dir(root.join(".github/PULL_REQUEST_TEMPLATE.md"))
            .unwrap();
        assert!(submit_ui::default_body(root, "fallback").await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn editor_receives_template_and_returns_edited_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("test editor.sh");
        std::fs::write(&script, "printf '\\nEdited details\\n' >> \"$1\"\n")
            .unwrap();
        let editor =
            format!("sh {}", shell_words::quote(script.to_str().unwrap()));
        let result = edit_metadata_with_editor(
            "Commit title",
            "## Template body",
            &editor,
        )
        .await
        .unwrap();
        assert_eq!(result.0, "Commit title");
        assert_eq!(result.1, "## Template body\n\nEdited details");
        assert!(
            edit_metadata_with_editor("Commit title", "body", "false")
                .await
                .is_err()
        );
    }

    #[test]
    fn submit_colors_match_graphite_actions() {
        let mut report = SubmitReport {
            branch: "feature".into(),
            url: "https://github.com/o/r/pull/7".into(),
            created: false,
            pushed: false,
            parent_changed: false,
            published: false,
        };
        assert!(report.line(true).contains("\x1b[2mfeature\x1b[0m"));
        for action in 0..3 {
            report.created = action == 0;
            report.pushed = action == 1;
            report.parent_changed = action == 2;
            assert!(report.line(true).contains("\x1b[36mfeature\x1b[0m"));
            assert!(!report.line(false).contains('\x1b'));
        }
    }

    #[test]
    fn editor_document_has_title_blank_line_and_markdown() {
        let parsed =
            parse_pr_document("feat: auth\n\n# Why\nDetails\n").unwrap();
        assert_eq!(parsed.0, "feat: auth");
        assert_eq!(parsed.1, "# Why\nDetails");
        assert!(parse_pr_document("\n\nbody").is_err());
        assert!(parse_pr_document("title\nbody").is_err());
    }

    #[test]
    fn metadata_uses_first_subject_and_all_remaining_messages() {
        let messages = vec![
            "feat: auth\n\nAuthentication details".into(),
            "test: auth\n\nTest details".into(),
        ];
        assert_eq!(
            commit_metadata(&messages).unwrap(),
            (
                "feat: auth".into(),
                "Authentication details\n\ntest: auth\n\nTest details".into()
            )
        );
        assert!(commit_metadata(&[]).is_err());
    }
}
