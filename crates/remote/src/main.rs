use aigle_remote::{Remote, checkout_server};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "thesauros-client",
    about = "Connect an Aigle editing workflow to a Thesauros RAW library"
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    List {
        server: String,
        #[arg(long, default_value = "")]
        search: String,
        #[arg(long, default_value_t = 0)]
        offset: u32,
    },
    Checkout {
        server: String,
        id: String,
        #[arg(long, default_value = "./checkouts")]
        cache: PathBuf,
        #[arg(long, value_name = "AIGLE_APP")]
        open: Option<PathBuf>,
    },
    Push {
        folder: PathBuf,
    },
    Upload {
        server: String,
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    Import {
        server: String,
        path: String,
    },
}
fn main() -> Result<()> {
    let args = Args::parse();
    let token = std::env::var("THESAUROS_TOKEN").context("Set THESAUROS_TOKEN")?;
    match args.command {
        Command::List {
            server,
            search,
            offset,
        } => println!(
            "{}",
            serde_json::to_string_pretty(
                &Remote::new(&server, token)?.list(&search, 100, offset)?
            )?
        ),
        Command::Checkout {
            server,
            id,
            cache,
            open,
        } => {
            let path = Remote::new(&server, token)?.checkout(&id, &cache)?;
            println!("{}", path.display());
            if let Some(exe) = open {
                std::process::Command::new(exe)
                    .arg(path.canonicalize()?)
                    .spawn()?;
            }
        }
        Command::Push { folder } => {
            Remote::new(&checkout_server(&folder)?, token)?.push(&folder)?;
            println!("Edits synchronized");
        }
        Command::Upload { server, files } => {
            let remote = Remote::new(&server, token)?;
            let mut failed = 0;
            for file in files {
                match remote.upload(&file) {
                    Ok(p) => println!("{} {}", p.id, p.name),
                    Err(e) => {
                        failed += 1;
                        eprintln!("{}: {e:#}", file.display());
                    }
                }
            }
            anyhow::ensure!(failed == 0, "{failed} uploads failed");
        }
        Command::Import { server, path } => {
            println!("{}", Remote::new(&server, token)?.import(&path)?)
        }
    }
    Ok(())
}
