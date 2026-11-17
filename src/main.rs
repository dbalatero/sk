mod aliases;
mod help;

use anyhow::{Result, bail};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("sk: {error:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args = aliases::expand(args).await?;
    if help::requested(&args)? {
        return Ok(());
    }
    match args.first().map(String::as_str) {
        None | Some("help" | "--help" | "-h") => {
            println!(concat!(
                "sk — local-first stacked branches\n\n",
                "Usage: sk <command> [options]\n\n",
                "Commands:\n",
                "  create | c [name] [-m message] [-a]\n",
                "  submit | s | ss [-dn] [--stack]\n",
                "  sync\n",
                "  aliases\n",
                "  reorder [--stack]\n",
                "  restack | continue | abort\n",
                "  log short | ls\n",
                "  checkout [branch] | co [branch]\n",
                "  up [n] | down [n]\n",
                "  pr [branch]\n\n",
                "Use sk <command> --help for command options.\n",
                "Only sync and submit access GitHub."
            ));
            Ok(())
        }
        Some("aliases") => aliases::create(&args).await,
        Some("--version" | "-V") => {
            println!("sk {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("create" | "reorder" | "restack" | "continue" | "abort") => {
            sk::core::run(&args).await
        }
        Some("sync" | "submit" | "ss") => sk::remote::run(&args).await,
        Some("log" | "ls" | "checkout" | "co" | "up" | "down" | "pr") => {
            sk::ui::run(&args).await
        }
        Some(command) => bail!("unknown command {command:?}; run sk --help"),
    }
}
