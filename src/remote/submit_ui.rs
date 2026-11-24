//! Local preparation of new PRs, before any push or API mutation.
use super::{SubmitOptions, edit_metadata};
use anyhow::{Context, Result, bail};
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
    interactive: bool,
) -> Result<((String, String), bool)> {
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
            {
                metadata = edit_metadata(&metadata.0, &metadata.1).await?;
            }
        }
    }
    let draft = if options.publish {
        false
    } else if interactive && !options.draft {
        choose(
            "Submit",
            ["Publish Pull Request", "Create Draft Pull Request"],
        )
        .await?
    } else {
        options.draft
    };
    Ok((metadata, draft))
}

async fn choose(title: &str, choices: [&str; 2]) -> Result<bool> {
    let items = choices
        .iter()
        .map(|&label| (label.to_owned(), label.to_owned()))
        .collect();
    let selected = crate::ui::pick_menu(title, items)
        .await?
        .context("Submit cancelled; nothing was submitted")?;
    eprintln!("{title}: {selected}");
    Ok(selected == choices[1])
}
