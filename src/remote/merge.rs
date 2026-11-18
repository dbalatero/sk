use super::*;

#[derive(Default)]
struct Options {
    include_current: bool,
    stack: bool,
    dry_run: bool,
    method: Option<String>,
}

fn options(args: &[String]) -> Result<Options> {
    let mut options = Options::default();
    for arg in args {
        match arg.as_str() {
            "--include-current" => options.include_current = true,
            "--stack" => options.stack = true,
            "--dry-run" => options.dry_run = true,
            "--squash" | "--rebase" | "--merge" => {
                if options.method.is_some() {
                    bail!("choose only one merge method");
                }
                options.method = Some(arg[2..].into());
            }
            _ => bail!("unsupported merge argument: {arg}"),
        }
    }
    Ok(options)
}

fn merge_end(current: usize, length: usize, options: &Options) -> usize {
    if options.stack {
        length
    } else {
        current + usize::from(options.include_current)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MergeConfig {
    merge_commit_allowed: bool,
    squash_merge_allowed: bool,
    rebase_merge_allowed: bool,
    viewer_default_merge_method: Option<String>,
}

impl MergeConfig {
    fn choose(&self, requested: Option<&str>) -> Result<String> {
        let allowed: Vec<_> = [
            ("merge", self.merge_commit_allowed),
            ("squash", self.squash_merge_allowed),
            ("rebase", self.rebase_merge_allowed),
        ]
        .into_iter()
        .filter_map(|(name, enabled)| enabled.then_some(name))
        .collect();
        let first = allowed
            .first()
            .context("Repository has no enabled merge methods")?;
        if let Some(requested) = requested {
            if !allowed.contains(&requested) {
                let flags = allowed
                    .iter()
                    .map(|method| format!("--{method}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                bail!("Repository does not allow --{requested}; use {flags}");
            }
            return Ok(requested.into());
        }
        let preferred = self
            .viewer_default_merge_method
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        Ok(if allowed.contains(&preferred.as_str()) {
            preferred
        } else {
            (*first).into()
        })
    }
}

async fn resolve_method(
    client: &GitHub,
    requested: Option<&str>,
) -> Result<String> {
    let (owner, name) = client
        .repository
        .split_once('/')
        .context("invalid GitHub repository")?;
    let value = GitHub::response(client.request_url(
        Method::POST, &format!("{}/graphql", client.base),
    ).json(&json!({
        "query": concat!(
            "query($owner:String!,$name:String!){",
            "repository(owner:$owner,name:$name){mergeCommitAllowed ",
            "squashMergeAllowed rebaseMergeAllowed viewerDefaultMergeMethod}}",
        ),
        "variables": {"owner": owner, "name": name},
    }))).await?.context("Repository merge settings unavailable")?;
    if let Some(errors) = value.get("errors") {
        bail!("Could not read repository merge settings: {errors}");
    }
    let config: MergeConfig =
        serde_json::from_value(value["data"]["repository"].clone())
            .context("Invalid repository merge settings response")?;
    config.choose(requested)
}

pub(super) async fn run(args: &[String]) -> Result<()> {
    let options = options(args)?;
    let repo = Repository::discover().await?;
    let _lock = repo.lock()?;
    repo.ensure_idle().await?;
    let state = repo.load().await?;
    let current = repo.current().await?;
    let stack = state
        .stacks
        .iter()
        .find(|s| s.branches.iter().any(|b| b.name == current))
        .context("checkout a tracked branch before merging")?;
    let position = stack
        .branches
        .iter()
        .position(|b| b.name == current)
        .unwrap();
    let end = merge_end(position, stack.branches.len(), &options);
    if end == 0 {
        println!("No branches below the current branch to merge.");
        return Ok(());
    }
    let client = api(&repo, &stack.remote).await?;
    let pulls = preflight(&repo, stack, end, &client).await?;
    if pulls.is_empty() {
        println!("No open PRs in the selected range. Run sk sync to refresh.");
        return Ok(());
    }
    let method = resolve_method(&client, options.method.as_deref()).await?;
    let color = submit_ui::color(std::io::stdout().is_terminal());
    let paint = |text: &str, code| submit_ui::paint(text, code, color);
    println!("Merge method: {}", paint(&method, "36"));
    println!(
        "{} into {}:",
        if options.dry_run {
            "Would merge"
        } else {
            "Merging"
        },
        paint(&stack.trunk, "36")
    );
    for pull in &pulls {
        println!(
            "  {} {}",
            paint(&format!("#{}", pull.number), "94"),
            paint(&pull.head.name, "36")
        );
    }
    if options.dry_run {
        return Ok(());
    }
    let target = pulls.last().unwrap();
    let result =
        execute(&client, target, Some(&method), Duration::from_secs(1), 60)
            .await?;
    match result.as_str() {
        "merged" => println!(
            "{}",
            paint(
                &format!(
                    "Merged {} PRs into {}. Run sk sync to refresh local branches.",
                    pulls.len(),
                    stack.trunk,
                ),
                "32"
            )
        ),
        "enqueued" => println!(
            "{}",
            paint(
                "Added to the merge queue; not merged yet. Run sk sync later.",
                "33",
            )
        ),
        _ => unreachable!(),
    }
    Ok(())
}

async fn preflight(
    repo: &Repository,
    stack: &Stack,
    end: usize,
    client: &GitHub,
) -> Result<Vec<Pull>> {
    let mut pulls = Vec::new();
    for branch in &stack.branches[..end] {
        let pull = match &branch.pr {
            Some(pr) => client.pull(pr.number).await?,
            None => client.find_open(&branch.name).await?,
        }
        .with_context(|| format!("{} has no PR; run sk submit", branch.name))?;
        if pull.head.name != branch.name {
            bail!(
                "PR #{} no longer matches {}; run sk sync",
                pull.number,
                branch.name
            );
        }
        if pull.merged_at.is_some() {
            continue;
        }
        if pull.state != "open" || pull.draft {
            bail!(
                "PR #{} is closed or draft; cannot merge this range",
                pull.number
            );
        }
        let local = repo
            .git(&["rev-parse", &format!("refs/heads/{}", branch.name)])
            .await?;
        if local.trim() != pull.head.sha {
            bail!(
                "{} differs from GitHub; sync/submit before merging",
                branch.name
            );
        }
        pulls.push(pull);
    }
    let Some(target) = pulls.last() else {
        return Ok(pulls);
    };
    if pulls
        .first()
        .and_then(|p| p.base.as_ref())
        .is_none_or(|base| base.name != stack.trunk)
    {
        bail!("remote stack base differs from local trunk; run sk sync");
    }
    let stacks = client.stacks().await?;
    if let Some(remote) = stacks
        .iter()
        .find(|s| s.pull_requests.iter().any(|p| p.number == target.number))
    {
        let numbers: Vec<_> = pulls.iter().map(|p| p.number).collect();
        validate_prefix(remote, &numbers)?;
    } else if pulls.len() != 1
        || target
            .base
            .as_ref()
            .is_none_or(|base| base.name != stack.trunk)
    {
        bail!("selected PRs are not a published stack; run sk sync/submit");
    }
    Ok(pulls)
}

fn validate_prefix(stack: &RemoteStack, selected: &[u64]) -> Result<()> {
    let target = selected.last().context("empty merge selection")?;
    let end = stack
        .pull_requests
        .iter()
        .position(|p| p.number == *target)
        .context("merge target is no longer in the remote stack")?;
    let unmerged: Vec<_> = stack.pull_requests[..=end]
        .iter()
        .filter(|p| p.merged_at.is_none())
        .collect();
    if unmerged.iter().any(|p| p.state != "open")
        || unmerged.iter().map(|p| p.number).collect::<Vec<_>>() != selected
    {
        bail!(
            "remote stack differs from selected branches; run sk sync before merging"
        );
    }
    Ok(())
}

#[derive(Deserialize)]
struct MergeResult {
    status: String,
    #[serde(default)]
    details: MergeDetails,
}

#[derive(Default, Deserialize)]
struct MergeDetails {
    #[serde(default)]
    uuid: String,
    #[serde(default)]
    message: String,
}

async fn execute(
    client: &GitHub,
    target: &Pull,
    method: Option<&str>,
    interval: Duration,
    max_polls: usize,
) -> Result<String> {
    execute_with_progress(
        client,
        target,
        method,
        interval,
        max_polls,
        |message| {
            println!(
                "{}",
                submit_ui::paint(
                    message,
                    "94",
                    submit_ui::color(std::io::stdout().is_terminal())
                )
            );
            let _ = std::io::stdout().flush();
        },
    )
    .await
}

async fn execute_with_progress(
    client: &GitHub,
    target: &Pull,
    method: Option<&str>,
    interval: Duration,
    max_polls: usize,
    mut progress: impl FnMut(&str),
) -> Result<String> {
    let started = std::time::Instant::now();
    progress("Submitting atomic stack merge to GitHub...");
    let path = format!("pulls/{}/merge-async", target.number);
    let mut body = json!({
        "sha": target.head.sha,
        "merge_action": "default",
        "bypass_rules": false,
    });
    if let Some(method) = method {
        body["merge_method"] = json!(method);
    }
    let response = client
        .request(Method::PUT, &path)
        .json(&body)
        .send()
        .await
        .context(
            "merge request outcome unknown; check GitHub before retrying",
        )?;
    let status = response.status();
    let value: Value = response
        .json()
        .await
        .context("merge response unreadable; check GitHub before retrying")?;
    if !status.is_success() && status != reqwest::StatusCode::CONFLICT {
        bail!(
            "GitHub rejected merge ({status}): {}",
            value["details"]["message"]
                .as_str()
                .or_else(|| value["message"].as_str())
                .unwrap_or("unknown error")
        );
    }
    let mut result: MergeResult = serde_json::from_value(value)?;
    if status == reqwest::StatusCode::CONFLICT {
        progress("Resuming the existing merge request...");
    }
    // A conflict response reports the existing request; poll, never resubmit.
    let mut uuid = result.details.uuid.clone();
    for attempt in 0..=max_polls {
        match result.status.as_str() {
            "merged" | "enqueued" => return Ok(result.status),
            "failed" => bail!("merge failed: {}", result.details.message),
            "pending" => {}
            other => bail!("unknown merge status {other:?}; check GitHub"),
        }
        if !result.details.uuid.is_empty() {
            uuid = result.details.uuid;
        }
        if uuid.is_empty()
            || !uuid.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            bail!(
                "merge is pending but no valid request ID was returned; check GitHub"
            );
        }
        progress(&format!(
            "Waiting for GitHub: pending ({}s elapsed, check {attempt}/{max_polls}).",
            started.elapsed().as_secs(),
        ));
        if attempt == max_polls {
            bail!(
                "merge still pending (request {uuid}); check GitHub or rerun sk merge to resume polling"
            );
        }
        tokio::time::sleep(interval).await;
        let value = GitHub::response(client.request(Method::GET, &format!("{path}/{uuid}")))
            .await.with_context(|| format!("could not poll merge {uuid}; check GitHub or rerun sk merge"))?
            .context("merge request expired or unavailable; check GitHub and run sk sync")?;
        result = serde_json::from_value(value)?;
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn chooses_allowed_default_and_sends_it_to_merge_api() {
        let (client, task) = server(vec![
            (
                "POST /graphql ",
                200,
                json!({"data": {"repository": {
                    "mergeCommitAllowed": false, "squashMergeAllowed": true,
                    "rebaseMergeAllowed": false,
                    "viewerDefaultMergeMethod": "MERGE"
                }}}),
            ),
            (
                "PUT /repos/o/r/pulls/7/merge-async ",
                200,
                json!({"status": "merged"}),
            ),
        ])
        .await;
        let method = resolve_method(&client, None).await.unwrap();
        assert_eq!(method, "squash");
        let pr = serde_json::from_value(pull(7, "a", "main", "abc")).unwrap();
        execute(&client, &pr, Some(&method), Duration::ZERO, 1)
            .await
            .unwrap();
        let requests = task.await.unwrap();
        let body: Value =
            serde_json::from_str(requests[1].split("\r\n\r\n").nth(1).unwrap())
                .unwrap();
        assert_eq!(body["merge_method"], "squash");
        assert_eq!(body["merge_action"], "default");
    }

    #[tokio::test]
    async fn disallowed_method_and_graphql_errors_stop_before_merge() {
        for response in [
            json!({"data": {"repository": {
                "mergeCommitAllowed": false, "squashMergeAllowed": true,
                "rebaseMergeAllowed": false,
                "viewerDefaultMergeMethod": "SQUASH"
            }}}),
            json!({"errors": [{"message": "Access denied"}]}),
        ] {
            let (client, task) =
                server(vec![("POST /graphql ", 200, response)]).await;
            assert!(resolve_method(&client, Some("merge")).await.is_err());
            assert_eq!(task.await.unwrap().len(), 1);
        }
    }

    #[test]
    fn merge_method_respects_preference_and_explicit_flags() {
        let config = MergeConfig {
            merge_commit_allowed: true,
            squash_merge_allowed: true,
            rebase_merge_allowed: false,
            viewer_default_merge_method: Some("SQUASH".into()),
        };
        assert_eq!(config.choose(None).unwrap(), "squash");
        assert_eq!(config.choose(Some("merge")).unwrap(), "merge");
        assert!(
            config
                .choose(Some("rebase"))
                .unwrap_err()
                .to_string()
                .contains("--squash")
        );
        let disabled = MergeConfig {
            merge_commit_allowed: false,
            squash_merge_allowed: false,
            rebase_merge_allowed: false,
            viewer_default_merge_method: None,
        };
        assert!(disabled.choose(None).is_err());
    }

    #[tokio::test]
    async fn progress_reports_pending_until_atomic_merge_finishes() {
        let (client, server) = server(vec![
            (
                "PUT /repos/o/r/pulls/7/merge-async ",
                409,
                json!({"status":"pending","details":{"uuid":"job-1"}}),
            ),
            (
                "GET /repos/o/r/pulls/7/merge-async/job-1 ",
                200,
                json!({"status":"pending","details":{"uuid":"job-1"}}),
            ),
            (
                "GET /repos/o/r/pulls/7/merge-async/job-1 ",
                200,
                json!({"status":"merged"}),
            ),
        ])
        .await;
        let target =
            serde_json::from_value(pull(7, "a", "main", "abc")).unwrap();
        let mut events = Vec::new();
        let result = execute_with_progress(
            &client,
            &target,
            Some("squash"),
            Duration::ZERO,
            3,
            |message| events.push(message.to_owned()),
        )
        .await
        .unwrap();
        assert_eq!(result, "merged");
        assert!(events[0].contains("Submitting"));
        assert!(events[1].contains("Resuming"));
        assert!(events.iter().any(|line| line.contains("check 1/3")));
        assert!(!events.iter().any(|line| line.contains("Merged")));
        server.await.unwrap();
    }

    #[test]
    fn default_scope_excludes_current_and_flags_extend_it() {
        assert_eq!(merge_end(2, 4, &Options::default()), 2);
        assert_eq!(merge_end(0, 4, &Options::default()), 0);
        assert_eq!(
            merge_end(2, 4, &options(&["--include-current".into()]).unwrap()),
            3
        );
        assert_eq!(merge_end(2, 4, &options(&["--stack".into()]).unwrap()), 4);
        assert!(options(&["--squash".into(), "--rebase".into()]).is_err());
    }

    #[test]
    fn remote_prefix_must_match_exact_requested_open_prs() {
        let stack: RemoteStack = serde_json::from_value(json!({
            "number": 9, "pull_requests": [
                {"number": 1, "state": "closed", "merged_at": "yesterday"},
                {"number": 2, "state": "open", "merged_at": null},
                {"number": 3, "state": "open", "merged_at": null},
                {"number": 4, "state": "open", "merged_at": null}
            ]
        }))
        .unwrap();
        assert!(validate_prefix(&stack, &[2, 3]).is_ok());
        assert!(validate_prefix(&stack, &[3]).is_err());
        assert!(validate_prefix(&stack, &[2, 4]).is_err());
    }
    async fn server(
        replies: Vec<(&'static str, u16, Value)>,
    ) -> (GitHub, tokio::task::JoinHandle<Vec<String>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener =
            tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            for (expected, status, value) in replies {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut buf = [0; 4096];
                    let n = socket.read(&mut buf).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buf[..n]);
                    if let Some(end) =
                        bytes.windows(4).position(|w| w == b"\r\n\r\n")
                    {
                        let header = String::from_utf8_lossy(&bytes[..end]);
                        let length = header
                            .lines()
                            .find_map(|line| {
                                let (key, val) = line.split_once(':')?;
                                key.eq_ignore_ascii_case("content-length").then(
                                    || val.trim().parse::<usize>().unwrap(),
                                )
                            })
                            .unwrap_or(0);
                        if bytes.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let request = String::from_utf8(bytes).unwrap();
                assert!(request.starts_with(expected), "{request}");
                requests.push(request);
                let body = value.to_string();
                let reply = format!(
                    "HTTP/1.1 {status} Response\r\n\
                    Content-Length: {}\r\nContent-Type: application/json\r\n\
                    Connection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
            requests
        });
        (
            GitHub::new(&format!("http://{address}"), "o/r", "test".into()),
            task,
        )
    }

    fn pull(number: u64, name: &str, base: &str, sha: &str) -> Value {
        json!({"number": number, "html_url": "https://github.com/o/r/pull/7",
            "state": "open", "draft": false, "merged_at": null,
            "head": {"sha": sha, "ref": name},
            "base": {"sha": "root", "ref": base}})
    }

    #[tokio::test]
    async fn async_merge_pins_sha_and_resumes_existing_request() {
        for status in [202, 409] {
            let (client, server) = server(vec![
                (
                    "PUT /repos/o/r/pulls/7/merge-async ",
                    status,
                    json!({"status": "pending", "details": {"uuid": "job-1"}}),
                ),
                (
                    "GET /repos/o/r/pulls/7/merge-async/job-1 ",
                    200,
                    json!({"status": "merged", "details": {"sha": "merged"}}),
                ),
            ])
            .await;
            let pr =
                serde_json::from_value(pull(7, "a", "main", "abc")).unwrap();
            assert_eq!(
                execute(&client, &pr, Some("squash"), Duration::ZERO, 2)
                    .await
                    .unwrap(),
                "merged"
            );
            let requests = server.await.unwrap();
            let body = requests[0].split("\r\n\r\n").nth(1).unwrap();
            let body: Value = serde_json::from_str(body).unwrap();
            assert_eq!(
                body,
                json!({"sha": "abc", "merge_method": "squash",
                "merge_action": "default", "bypass_rules": false})
            );
        }
    }

    #[tokio::test]
    async fn queued_failed_and_pending_results_are_not_reported_as_merged() {
        for (status, expected) in [
            ("enqueued", "enqueued"),
            ("failed", "blocked"),
            ("pending", "still pending"),
        ] {
            let (client, server) = server(vec![(
                "PUT /repos/o/r/pulls/7/merge-async ",
                200,
                json!({"status": status, "details": {
                        "uuid": "job-1", "message": "blocked"}}),
            )])
            .await;
            let pr =
                serde_json::from_value(pull(7, "a", "main", "abc")).unwrap();
            let result = execute(&client, &pr, None, Duration::ZERO, 0).await;
            if status == "enqueued" {
                assert_eq!(result.unwrap(), expected);
            } else {
                assert!(result.unwrap_err().to_string().contains(expected));
            }
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn http_rejection_reports_github_reason_without_retrying() {
        let (client, server) = server(vec![(
            "PUT /repos/o/r/pulls/7/merge-async ",
            400,
            json!({"details": {"message": "PR is draft"}}),
        )])
        .await;
        let pr = serde_json::from_value(pull(7, "a", "main", "abc")).unwrap();
        let error = execute(&client, &pr, None, Duration::ZERO, 0)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("PR is draft"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn preflight_rechecks_drafts_heads_and_remote_scope() {
        let (_dir, repo) = super::super::tests::fixture().await;
        let root = repo.git(&["rev-parse", "main"]).await.unwrap();
        repo.git(&["switch", "-c", "a"]).await.unwrap();
        let head = super::super::tests::add_commit(&repo, "a.txt", "A").await;
        let stack = Stack {
            id: "a".into(),
            trunk: "main".into(),
            remote: "origin".into(),
            github_number: Some(9),
            branches: vec![core::Branch {
                name: "a".into(),
                base: root,
                remote_oid: Some(head.clone()),
                fetched_oid: Some(head.clone()),
                pr: Some(
                    serde_json::from_value::<Pull>(pull(7, "a", "main", &head))
                        .unwrap()
                        .cache(),
                ),
            }],
        };
        for kind in ["ok", "draft", "changed", "extra", "merged", "closed"] {
            let mut pr = pull(7, "a", "main", &head);
            match kind {
                "draft" => pr["draft"] = json!(true),
                "changed" => pr["head"]["sha"] = json!("other"),
                "merged" => pr["merged_at"] = json!("yesterday"),
                "closed" => pr["state"] = json!("closed"),
                _ => {}
            }
            let mut replies = vec![("GET /repos/o/r/pulls/7 ", 200, pr)];
            if kind == "ok" || kind == "extra" {
                let mut prs =
                    vec![json!({"number":7, "state":"open", "merged_at":null})];
                if kind == "extra" {
                    prs.insert(
                        0,
                        json!({"number":6, "state":"open", "merged_at":null}),
                    );
                }
                replies.push((
                    "GET /repos/o/r/stacks?",
                    200,
                    json!([{"number":9,"pull_requests":prs}]),
                ));
            }
            let (client, server) = server(replies).await;
            let result = preflight(&repo, &stack, 1, &client).await;
            assert_eq!(
                result.is_ok(),
                matches!(kind, "ok" | "merged"),
                "{kind}"
            );
            if kind == "merged" {
                assert!(result.unwrap().is_empty());
            }
            server.await.unwrap();
        }
    }
}
