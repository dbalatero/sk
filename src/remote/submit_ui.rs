//! Local preparation of new PRs, before any push or API mutation.
use super::{SubmitOptions, edit_metadata};
use anyhow::{Context, Result, bail};
use crossterm::{cursor, event, execute, terminal};
use std::io::{IsTerminal, Write};
use std::path::Path;

pub(super) fn color(terminal: bool) -> bool {
    terminal
        && std::env::var_os("NO_COLOR").is_none()
        && std::env::var("TERM").as_deref() != Ok("dumb")
}

pub(super) fn paint(text: &str, code: &str, color: bool) -> String {
    if color {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.into()
    }
}

pub(super) async fn default_body(
    root: &Path,
    fallback: &str,
) -> Result<String> {
    let path = root.join(".github/PULL_REQUEST_TEMPLATE.md");
    match tokio::fs::read_to_string(&path).await {
        Ok(body) => Ok(body),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(fallback.into())
        }
        Err(error) => Err(error)
            .with_context(|| format!("Cannot read {}", path.display())),
    }
}

pub(super) async fn prepare(
    root: &Path,
    branch: &str,
    title: &str,
    body: &str,
    options: &SubmitOptions,
) -> Result<((String, String), bool)> {
    let interactive =
        std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    if !interactive && !options.no_edit {
        bail!("New PR prompts require a terminal; use -n (or -dn for drafts)");
    }
    let body = default_body(root, body).await?;
    let mut metadata = (title.to_owned(), body);
    if interactive {
        let color = color(true);
        eprintln!("\nNew pull request for {}", paint(branch, "36", color));
        eprintln!("Title: {title}");
        if !options.no_edit {
            eprintln!("\nDefault description:\n{}\n", metadata.1);
            if choose(
                "Body",
                ["Use default description", "Edit Body ($EDITOR)"],
            )
            .await?
                == 1
            {
                metadata = edit_metadata(&metadata.0, &metadata.1).await?;
            }
        }
    }
    let draft = if interactive && !options.draft {
        choose(
            "Submit",
            ["Publish Pull Request", "Create Draft Pull Request"],
        )
        .await?
            == 1
    } else {
        options.draft
    };
    Ok((metadata, draft))
}

async fn choose(
    title: &'static str,
    choices: [&'static str; 2],
) -> Result<usize> {
    tokio::task::spawn_blocking(move || select(title, choices)).await?
}

struct PromptGuard;

impl Drop for PromptGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(
            std::io::stderr(),
            cursor::Show,
            terminal::Clear(terminal::ClearType::FromCursorDown)
        );
    }
}

fn select(title: &str, choices: [&str; 2]) -> Result<usize> {
    terminal::enable_raw_mode()?;
    let _guard = PromptGuard;
    let mut out = std::io::stderr();
    execute!(out, cursor::Hide)?;
    let mut selected = 0;
    loop {
        let width = usize::from(terminal::size()?.0.saturating_sub(1));
        execute!(
            out,
            cursor::MoveToColumn(0),
            terminal::Clear(terminal::ClearType::FromCursorDown)
        )?;
        let heading = format!("? {title} (↑/↓ to choose, Enter to confirm)");
        let heading: String = heading.chars().take(width).collect();
        write!(out, "{heading}\r\n")?;
        for (index, choice) in choices.iter().enumerate() {
            let marker = if index == selected { "❯" } else { " " };
            let line = format!("{marker} {choice}");
            let line: String = line.chars().take(width).collect();
            write!(
                out,
                "{}\r\n",
                paint(&line, "36", color(true) && index == selected,)
            )?;
        }
        execute!(out, cursor::MoveUp(3))?;
        out.flush()?;
        let event::Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind == event::KeyEventKind::Release {
            continue;
        }
        match selection(key, selected)? {
            Selection::Move(index) => selected = index,
            Selection::Accept => {
                execute!(
                    out,
                    cursor::MoveToColumn(0),
                    terminal::Clear(terminal::ClearType::FromCursorDown)
                )?;
                write!(out, "{title}: {}\r\n", choices[selected])?;
                out.flush()?;
                return Ok(selected);
            }
        }
    }
}

enum Selection {
    Move(usize),
    Accept,
}

fn selection(key: event::KeyEvent, selected: usize) -> Result<Selection> {
    use event::{KeyCode, KeyModifiers};
    match key.code {
        KeyCode::Esc => bail!("Submit cancelled; nothing was submitted"),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            bail!("Submit cancelled; nothing was submitted")
        }
        KeyCode::Enter => Ok(Selection::Accept),
        KeyCode::Up
        | KeyCode::Down
        | KeyCode::Left
        | KeyCode::Right
        | KeyCode::Tab
        | KeyCode::Char('j' | 'k') => Ok(Selection::Move(1 - selected)),
        _ => Ok(Selection::Move(selected)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_keys_select_accept_and_cancel() {
        use event::{KeyCode, KeyEvent, KeyModifiers};
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        assert!(matches!(
            selection(key(KeyCode::Down), 0).unwrap(),
            Selection::Move(1)
        ));
        assert!(matches!(
            selection(key(KeyCode::Up), 1).unwrap(),
            Selection::Move(0)
        ));
        assert!(matches!(
            selection(key(KeyCode::Enter), 1).unwrap(),
            Selection::Accept
        ));
        assert!(selection(key(KeyCode::Esc), 0).is_err());
        assert!(
            selection(
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                0
            )
            .is_err()
        );
    }
}
