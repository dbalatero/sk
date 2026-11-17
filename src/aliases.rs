use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;

const DEFAULTS: &str = "\
# Edit this file to configure sk aliases.\n\
# First word: alias; remaining words: command and arguments.\n\
# Lines starting with # are ignored. Quoted arguments are supported.\n\
# User entries override built-in aliases. No shell commands are executed.\n\n\
c create\n\
m modify\n\
rs restack\n\
tr track\n\
sp split\n\
t top\n\
b bottom\n\
s submit\n\
ss submit --stack\n\
ls log short\n\
co checkout\n";

fn path() -> Result<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config"))
        })
        .context("set HOME or XDG_CONFIG_HOME to locate sk aliases")?;
    Ok(config.join("sk/aliases"))
}

pub async fn create(args: &[String]) -> Result<()> {
    if args.len() != 1 {
        bail!("aliases takes no arguments");
    }
    let path = path()?;
    tokio::fs::create_dir_all(path.parent().unwrap()).await?;
    match tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .await
    {
        Ok(mut file) => file.write_all(DEFAULTS.as_bytes()).await?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    println!("{}", path.display());
    Ok(())
}

pub async fn expand(args: Vec<String>) -> Result<Vec<String>> {
    // Keep aliases accessible even if the file needs repair.
    if args.is_empty()
        || matches!(
            args[0].as_str(),
            "aliases" | "--help" | "-h" | "--version" | "-V"
        )
    {
        return Ok(args);
    }
    let mut aliases = parse(DEFAULTS)?;
    if let Ok(path) = path() {
        match tokio::fs::read_to_string(&path).await {
            Ok(contents) => {
                aliases.extend(parse(&contents).with_context(|| {
                    format!("invalid aliases in {}", path.display())
                })?)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).context("could not read sk aliases");
            }
        }
    }
    resolve(args, &aliases)
}

fn resolve(
    mut args: Vec<String>,
    aliases: &BTreeMap<String, Vec<String>>,
) -> Result<Vec<String>> {
    let help = args.first().is_some_and(|a| a == "help");
    if help {
        args.remove(0);
    }
    let mut seen = BTreeSet::new();
    while let Some(command) = args.first() {
        let Some(expansion) = aliases.get(command) else {
            break;
        };
        if !seen.insert(command.clone()) {
            bail!("alias cycle involving {command:?}; edit sk aliases");
        }
        args.splice(..1, expansion.iter().cloned());
    }
    if args.first().is_some_and(|a| a == "stack")
        && args.get(1).is_some_and(|a| a == "--submit")
    {
        args.splice(..2, ["submit".into(), "--stack".into()]);
    }
    if help {
        args.push("--help".into());
    }
    Ok(args)
}

fn parse(contents: &str) -> Result<BTreeMap<String, Vec<String>>> {
    let mut aliases = BTreeMap::new();
    for (index, line) in contents.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut words = shell_words::split(line).with_context(|| {
            format!("alias line {}: invalid quoting", index + 1)
        })?;
        if words.len() < 2 {
            bail!("alias line {} needs a name and command", index + 1);
        }
        let name = words.remove(0);
        aliases.insert(name, words);
    }
    Ok(aliases)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expansion_preserves_quoted_and_trailing_arguments() {
        let mut aliases = parse(DEFAULTS).unwrap();
        aliases.extend(
            parse("msg c -m 'hello world'\ns submit --no-edit").unwrap(),
        );
        let args = vec!["msg".into(), "--all".into()];
        assert_eq!(
            resolve(args, &aliases).unwrap(),
            ["create", "-m", "hello world", "--all"]
        );
        assert_eq!(
            resolve(vec!["s".into()], &aliases).unwrap(),
            ["submit", "--no-edit"]
        );
        assert_eq!(
            resolve(vec!["ss".into(), "-dn".into()], &aliases).unwrap(),
            ["submit", "--stack", "-dn"]
        );
    }

    #[test]
    fn malformed_aliases_report_the_line() {
        for input in ["# comment\nbroken", "# comment\nbroken 'quote"] {
            let error = parse(input).unwrap_err().to_string();
            assert!(error.contains("line 2"), "{error}");
        }
    }
}
