//! Offline terminal navigation, stack rendering, and browser integration.

fn pr_identity(url: &str) -> Option<(String, String, String, u64)> {
    let remainder = url.strip_prefix("https://")?;
    let mut pieces = remainder.split('/');
    let host = pieces.next()?;
    if host.is_empty() || host.contains('@') {
        return None;
    }
    let owner = pieces.next()?;
    let repo = pieces.next()?;
    if owner.is_empty() || repo.is_empty() || pieces.next()? != "pull" {
        return None;
    }
    let number = pieces.next()?.split(['?', '#']).next()?.parse().ok()?;
    Some((
        host.to_ascii_lowercase(),
        owner.to_ascii_lowercase(),
        repo.to_ascii_lowercase(),
        number,
    ))
}

pub fn same_pr(left: &str, right: &str) -> bool {
    match (pr_identity(left), pr_identity(right)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

pub fn navigation_target(
    branches: &[String],
    trunk: &str,
    current: &str,
    up: bool,
    steps: usize,
) -> Option<String> {
    let mut chain = vec![trunk.to_owned()];
    chain.extend_from_slice(branches);
    let index = chain.iter().position(|branch| branch == current)?;
    let destination = if up {
        index.saturating_add(steps).min(chain.len() - 1)
    } else {
        index.saturating_sub(steps)
    };
    Some(chain[destination].clone())
}

#[derive(Clone, Debug)]
pub struct ViewBranch {
    pub name: String,
    pub pr: Option<u64>,
    pub state: String,
    pub needs_restack: bool,
}

#[derive(Clone, Debug)]
pub struct ViewStack {
    pub trunk: String,
    pub branches: Vec<ViewBranch>,
}

pub fn render_stacks(
    stacks: &[ViewStack],
    current: &str,
    color: bool,
) -> String {
    let rows = stack_rows(stacks, current, color);
    if rows.is_empty() {
        return "No tracked stacks. Start one with sk create.\n".into();
    }
    rows.into_iter().map(|(_, label)| label + "\n").collect()
}

fn stack_rows(
    stacks: &[ViewStack],
    current: &str,
    color: bool,
) -> Vec<(String, String)> {
    let mut trunks: Vec<_> = stacks.iter().map(|s| &s.trunk).collect();
    trunks.sort();
    trunks.dedup();
    let multiple_trunks = trunks.len() > 1;
    let mut rows = Vec::new();
    for trunk in trunks {
        if !rows.is_empty() {
            rows.push((String::new(), String::new()));
        }
        let mut group: Vec<_> = stacks
            .iter()
            .filter(|s| &s.trunk == trunk && !s.branches.is_empty())
            .collect();
        group.sort_by(|a, b| a.branches[0].name.cmp(&b.branches[0].name));
        let width = 2 * group.len().max(1) + 1;
        for (lane, stack) in group.iter().enumerate() {
            for branch in stack.branches.iter().rev() {
                let selected = branch.name == current;
                let marker = if selected { "◉" } else { "◯" };
                let graph = format!("{}{marker}", "│ ".repeat(lane));
                let annotation = if branch.needs_restack {
                    " (needs restack)"
                } else {
                    ""
                };
                rows.push((
                    branch.name.clone(),
                    short_row(
                        &graph,
                        &branch.name,
                        annotation,
                        width,
                        lane,
                        color,
                    ),
                ));
            }
        }
        let marker = if trunk == current { "◉" } else { "◯" };
        let graph = if group.len() > 1 {
            format!("{marker}{}─┘", "─┴".repeat(group.len() - 2))
        } else {
            marker.into()
        };
        let annotation = if multiple_trunks { " (trunk)" } else { "" };
        rows.push((
            trunk.clone(),
            short_row(&graph, trunk, annotation, width, 0, color),
        ));
    }
    rows
}

fn short_row(
    graph: &str,
    name: &str,
    annotation: &str,
    width: usize,
    lane: usize,
    color: bool,
) -> String {
    let selected = graph.contains('◉');
    let styled_graph: String = graph
        .chars()
        .enumerate()
        .map(|(i, ch)| lane_color(&ch.to_string(), i / 2, ch == '◉', color))
        .collect();
    format!(
        "{}{}{}{annotation}",
        styled_graph,
        " ".repeat(width.saturating_sub(graph.chars().count())),
        lane_color(name, lane, selected, color)
    )
}

fn lane_color(text: &str, lane: usize, bold: bool, color: bool) -> String {
    if !color || text == " " {
        return text.into();
    }
    let palette = [36, 33, 35, 32, 34, 31];
    let weight = if bold { "1;" } else { "" };
    format!(
        "\x1b[{weight}{}m{text}\x1b[0m",
        palette[lane % palette.len()]
    )
}

use std::io::IsTerminal;

use anyhow::{Context, Result, bail};
use skim::{Skim, helper::item::DefaultSkimItem, options::SkimOptionsBuilder};
use tokio::io::AsyncWriteExt;

use crate::core::{Repository, State};

pub async fn run(args: &[String]) -> Result<()> {
    let command = args.first().map(String::as_str).unwrap_or("ls");
    let repo = Repository::discover().await?;
    let state = repo.load().await?;
    let current = repo.current().await?;
    match command {
        "ls" | "log" => {
            if (command == "ls" && args.len() != 1)
                || (command == "log"
                    && (args.len() > 2
                        || args.get(1).is_some_and(|arg| arg != "short")))
            {
                bail!("usage: sk log short (or sk ls)");
            }
            let views = stack_views(&repo, &state).await;
            let color = std::io::stdout().is_terminal()
                && std::env::var_os("NO_COLOR").is_none();
            print!("{}", render_stacks(&views, &current, color));
        }
        "co" | "checkout" => {
            if args.len() > 2 {
                bail!("usage: sk checkout [branch]");
            }
            let target = match args.get(1) {
                Some(branch) => branch.clone(),
                None => {
                    let views = stack_views(&repo, &state).await;
                    let choices = stack_rows(
                        &views,
                        &current,
                        std::env::var_os("NO_COLOR").is_none(),
                    );
                    let Some(target) =
                        pick("Check out a branch", choices).await?
                    else {
                        return Ok(());
                    };
                    target
                }
            };
            switch(&repo, &target).await?;
        }
        "up" | "down" | "top" | "bottom" => {
            if matches!(command, "top" | "bottom") && args.len() != 1 {
                bail!("usage: sk {command}");
            }
            if args.len() > 2 {
                bail!("usage: sk {command} [steps]");
            }
            let steps =
                args.get(1).map_or(Ok(1), |arg| arg.parse::<usize>())?;
            if steps == 0 {
                bail!("steps must be greater than zero");
            }
            let candidates: Vec<_> = state
                .stacks
                .iter()
                .filter(|stack| {
                    stack.trunk == current
                        || stack.branches.iter().any(|b| b.name == current)
                })
                .collect();
            if candidates.is_empty() {
                bail!("current branch is not tracked by sk");
            }
            if command == "down"
                && candidates.iter().all(|stack| stack.trunk == current)
            {
                println!("Already at trunk {current}");
                return Ok(());
            }
            let mut choices = Vec::new();
            for stack in candidates {
                let branches: Vec<_> = stack
                    .branches
                    .iter()
                    .map(|branch| branch.name.clone())
                    .collect();
                let boundary = match command {
                    "top" => branches.last().cloned(),
                    "bottom" => branches.first().cloned(),
                    _ => None,
                };
                if let Some(target) = boundary.or_else(|| {
                    navigation_target(
                        &branches,
                        &stack.trunk,
                        &current,
                        command == "up",
                        steps,
                    )
                }) && !choices.iter().any(|(name, _)| name == &target)
                {
                    choices.push((target.clone(), target));
                }
            }
            let target = if choices.len() == 1 {
                choices.remove(0).0
            } else {
                let Some(target) = pick("Choose a stack", choices).await?
                else {
                    return Ok(());
                };
                target
            };
            if target == current {
                println!("Already at the stack boundary: {current}");
            } else {
                switch(&repo, &target).await?;
            }
        }
        "pr" => {
            if args.len() > 2 {
                bail!("usage: sk pr [branch]");
            }
            let name = args.get(1).unwrap_or(&current);
            let pr = state
                .stacks
                .iter()
                .flat_map(|stack| &stack.branches)
                .find(|branch| &branch.name == name)
                .and_then(|branch| branch.pr.as_ref())
                .context("No cached PR URL. Run sk sync or sk submit first.")?;
            open_chrome(&pr.url).await?;
        }
        _ => bail!("unknown local command: {command}"),
    }
    Ok(())
}

async fn stack_views(repo: &Repository, state: &State) -> Vec<ViewStack> {
    let mut views = Vec::new();
    for stack in &state.stacks {
        let mut parent = stack.trunk.as_str();
        let mut branches = Vec::new();
        for branch in &stack.branches {
            let needs_restack = repo
                .git(&["merge-base", "--is-ancestor", parent, &branch.name])
                .await
                .is_err();
            branches.push(ViewBranch {
                name: branch.name.clone(),
                pr: branch.pr.as_ref().map(|pr| pr.number),
                state: branch
                    .pr
                    .as_ref()
                    .map(|pr| {
                        if pr.draft && pr.state == "OPEN" {
                            "DRAFT".into()
                        } else {
                            pr.state.clone()
                        }
                    })
                    .unwrap_or_default(),
                needs_restack,
            });
            parent = &branch.name;
        }
        views.push(ViewStack {
            trunk: stack.trunk.clone(),
            branches,
        });
    }
    views
}

async fn switch(repo: &Repository, branch: &str) -> Result<()> {
    let _lock = repo.lock()?;
    repo.ensure_idle().await?;
    let reference = format!("refs/heads/{branch}");
    repo.git(&["show-ref", "--verify", &reference])
        .await
        .with_context(|| format!("local branch {branch:?} does not exist"))?;
    repo.git(&["switch", "--no-guess", "--", branch]).await?;
    println!("Checked out {branch}");
    Ok(())
}

pub(crate) async fn pick(
    title: &str,
    choices: Vec<(String, String)>,
) -> Result<Option<String>> {
    if choices.is_empty() {
        bail!("No tracked branches. Start a stack with sk create.");
    }
    run_picker(title, choices, false).await
}

pub(crate) async fn pick_menu(
    title: &str,
    choices: Vec<(String, String)>,
) -> Result<Option<String>> {
    run_picker(title, choices, true).await
}

async fn run_picker(
    title: &str,
    choices: Vec<(String, String)>,
    menu: bool,
) -> Result<Option<String>> {
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        bail!("interactive selection requires a terminal");
    }
    let prompt = format!("{title} > ");
    tokio::task::spawn_blocking(move || {
        let mut builder = SkimOptionsBuilder::default();
        if menu {
            builder
                .height((choices.len() + 2).to_string())
                .cycle(true)
                .bind(vec!["j:down,k:up,tab:down,left:up,right:down".into()]);
        }
        let options = builder
            .prompt(prompt)
            .reverse(true)
            .no_sort(true)
            .ansi(true)
            .build()?;
        let items: Vec<_> = choices
            .iter()
            .map(|(value, label)| {
                let mut item = DefaultSkimItem::new(
                    label,
                    true,
                    &[],
                    &[],
                    &options.delimiter,
                );
                if value.is_empty() {
                    item.disable();
                }
                item
            })
            .collect();
        let output = Skim::run_items(options, items)
            .map_err(|err| anyhow::anyhow!("{err}"))?;
        if output.is_abort {
            return Ok(None);
        }
        Ok(output
            .selected_items
            .first()
            .map(|item| choices[item.rank.index as usize].0.clone()))
    })
    .await?
}

async fn apple_script(script: &str, args: &[&str]) -> Result<String> {
    let mut child = tokio::process::Command::new("osascript")
        .arg("-")
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("could not start macOS AppleScript")?;
    child
        .stdin
        .take()
        .context("missing AppleScript input")?
        .write_all(script.as_bytes())
        .await?;
    let result = child.wait_with_output().await?;
    if !result.status.success() {
        bail!(
            "Chrome automation failed: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }
    Ok(String::from_utf8(result.stdout)?)
}

async fn open_chrome(url: &str) -> Result<()> {
    if pr_identity(url).is_none() {
        bail!("cached URL is not a valid HTTPS pull request URL");
    }
    if !cfg!(target_os = "macos") {
        bail!("Chrome integration currently requires macOS. PR: {url}");
    }
    let tabs = apple_script(CHROME_TABS, &[]).await?;
    for line in tabs.lines() {
        let mut fields = line.splitn(3, '\t');
        let (Some(window), Some(tab), Some(tab_url)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if same_pr(url, tab_url) {
            apple_script(CHROME_FOCUS, &[window, tab]).await?;
            println!("Focused {url}");
            return Ok(());
        }
    }
    apple_script(CHROME_OPEN, &[url]).await?;
    println!("Opened {url}");
    Ok(())
}

const CHROME_TABS: &str = r#"
set rows to ""
if application "Google Chrome" is running then
    tell application "Google Chrome"
        repeat with w in windows
            repeat with t in tabs of w
                set rowPrefix to (id of w as text) & (ASCII character 9)
                set rowPrefix to rowPrefix & (id of t as text) & ¬
                    (ASCII character 9)
                set rows to rows & rowPrefix & (URL of t) & linefeed
            end repeat
        end repeat
    end tell
end if
return rows
"#;

const CHROME_FOCUS: &str = r#"
on run argv
    tell application "Google Chrome"
        repeat with w in windows
            if (id of w as text) is item 1 of argv then
                repeat with n from 1 to count of tabs of w
                    if (id of tab n of w as text) is item 2 of argv then
                        set active tab index of w to n
                        set minimized of w to false
                        set index of w to 1
                        activate
                        return
                    end if
                end repeat
            end if
        end repeat
    end tell
    error "The matching Chrome tab disappeared; run sk pr again."
end run
"#;

const CHROME_OPEN: &str = r#"
on run argv
    tell application "Google Chrome"
        set destination to missing value
        repeat with w in windows
            if mode of w is "normal" then
                set destination to w
                exit repeat
            end if
        end repeat
        if destination is missing value then
            set destination to make new window
            set URL of active tab of destination to item 1 of argv
        else
            tell destination
                make new tab at end of tabs with properties ¬
                    {URL:item 1 of argv}
                set active tab index to count of tabs
            end tell
        end if
        set minimized of destination to false
        set index of destination to 1
        activate
    end tell
end run
"#;
