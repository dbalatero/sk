//! Offline terminal navigation, stack rendering, and browser integration.

/// Lower scores prefer earlier and more tightly packed matches.
pub fn fuzzy_score(query: &str, candidate: &str) -> Option<usize> {
    let query = query.to_lowercase();
    let candidate = candidate.to_lowercase();
    let mut positions = candidate.chars().enumerate();
    let mut score = 0;
    let mut previous = None;
    for wanted in query.chars() {
        let (index, _) = positions.find(|(_, ch)| *ch == wanted)?;
        score += match previous {
            Some(last) => index - last - 1,
            None => index,
        };
        previous = Some(index);
    }
    Some(score)
}

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
    let mut trunks: Vec<_> = stacks.iter().map(|s| &s.trunk).collect();
    trunks.sort();
    trunks.dedup();
    let multiple_trunks = trunks.len() > 1;
    let mut output = String::new();
    for trunk in trunks {
        if !output.is_empty() {
            output.push('\n');
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
                output.push_str(&short_row(
                    &graph,
                    &branch.name,
                    annotation,
                    width,
                    lane,
                    color,
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
        output.push_str(&short_row(&graph, trunk, annotation, width, 0, color));
    }
    if output.is_empty() {
        output.push_str("No tracked stacks. Start one with sk create.\n");
    }
    output
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
        "{}{}{}{annotation}\n",
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

use std::io::{IsTerminal, Write};

use anyhow::{Context, Result, bail};
use crossterm::{cursor, event, execute, terminal};
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
            let mut views = Vec::new();
            for stack in &state.stacks {
                let mut parent = stack.trunk.as_str();
                let mut branches = Vec::new();
                for branch in &stack.branches {
                    let needs_restack = repo
                        .git(&[
                            "merge-base",
                            "--is-ancestor",
                            parent,
                            &branch.name,
                        ])
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
                    let choices = checkout_choices(&state, &current);
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

pub fn checkout_choices(state: &State, current: &str) -> Vec<(String, String)> {
    let mut choices = Vec::new();
    for stack in &state.stacks {
        for branch in &stack.branches {
            let marker = if branch.name == current { "●" } else { "○" };
            let mut label =
                format!("{marker} {} ({})", branch.name, stack.trunk);
            if let Some(pr) = &branch.pr {
                label.push_str(&format!("  #{} {}", pr.number, pr.state));
            }
            choices.push((branch.name.clone(), label));
        }
        if !choices.iter().any(|(name, _)| name == &stack.trunk) {
            let marker = if stack.trunk == current { "●" } else { "◉" };
            choices.push((
                stack.trunk.clone(),
                format!("{marker} {} (trunk)", stack.trunk),
            ));
        }
    }
    choices
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
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        bail!("interactive selection requires a terminal; specify a branch");
    }
    let title = title.to_owned();
    tokio::task::spawn_blocking(move || picker(&title, &choices)).await?
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(
            std::io::stderr(),
            cursor::Show,
            terminal::LeaveAlternateScreen
        );
    }
}

fn picker(title: &str, choices: &[(String, String)]) -> Result<Option<String>> {
    terminal::enable_raw_mode()?;
    let _guard = TerminalGuard;
    let mut out = std::io::stderr();
    execute!(out, terminal::EnterAlternateScreen, cursor::Hide)?;
    let mut query = String::new();
    let mut selected = 0usize;
    loop {
        let mut matches: Vec<_> = choices
            .iter()
            .enumerate()
            .filter_map(|(index, (_, label))| {
                fuzzy_score(&query, label).map(|score| (score, index))
            })
            .collect();
        matches.sort_by_key(|&(score, index)| (score, index));
        selected = selected.min(matches.len().saturating_sub(1));
        let (width, height) = terminal::size()?;
        let capacity = usize::from(height.saturating_sub(4)).max(1);
        let start = selected.saturating_sub(capacity - 1);
        execute!(
            out,
            cursor::MoveTo(0, 0),
            terminal::Clear(terminal::ClearType::All)
        )?;
        writeln!(out, "{title}\r")?;
        writeln!(out, "> {query}\r")?;
        for (position, &(_, index)) in
            matches.iter().enumerate().skip(start).take(capacity)
        {
            let marker = if position == selected { ">" } else { " " };
            let label: String = choices[index]
                .1
                .chars()
                .take(usize::from(width.saturating_sub(3)))
                .collect();
            writeln!(out, "{marker} {label}\r")?;
        }
        out.flush()?;
        let event::Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind == event::KeyEventKind::Release {
            continue;
        }
        match key.code {
            event::KeyCode::Esc => return Ok(None),
            event::KeyCode::Char('c')
                if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
            {
                return Ok(None);
            }
            event::KeyCode::Enter => {
                if let Some(&(_, index)) = matches.get(selected) {
                    return Ok(Some(choices[index].0.clone()));
                }
            }
            event::KeyCode::Up => selected = selected.saturating_sub(1),
            event::KeyCode::Down => {
                selected = (selected + 1).min(matches.len().saturating_sub(1));
            }
            event::KeyCode::Backspace => {
                query.pop();
                selected = 0;
            }
            event::KeyCode::Char(ch)
                if !key.modifiers.intersects(
                    event::KeyModifiers::CONTROL | event::KeyModifiers::ALT,
                ) =>
            {
                query.push(ch);
                selected = 0;
            }
            _ => {}
        }
    }
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
