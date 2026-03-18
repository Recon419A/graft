mod github;
mod install;
mod platform;

use clap::{Parser, Subcommand};
use std::process;

#[derive(Parser)]
#[command(name = "graft", version, about = "Trust maintainers. Install the latest GitHub release.")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Install a package: org/repo
    #[arg(value_name = "ORG/REPO")]
    package: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Install a package from a GitHub release
    Install {
        /// GitHub repository (org/repo)
        package: String,
    },
    /// List installed packages
    List,
    /// Remove an installed package
    Remove {
        /// Package to remove
        package: String,
    },
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Some(Commands::Install { package }) => run_install(&package),
        Some(Commands::List) => run_list(),
        Some(Commands::Remove { package }) => run_remove(&package),
        None => {
            if let Some(package) = cli.package {
                run_install(&package)
            } else {
                eprintln!("Usage: graft <org/repo>");
                eprintln!("Run `graft --help` for more information.");
                process::exit(1);
            }
        }
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        process::exit(1);
    }
}

fn run_install(package: &str) -> Result<(), String> {
    let (owner, repo) = parse_package(package)?;

    eprintln!("==> Finding latest release for {owner}/{repo}...");
    let release = github::latest_release(&owner, &repo)?;
    eprintln!("==> Found release: {}", release.tag_name);

    let target = platform::detect()?;
    eprintln!("==> Detected platform: {target}");

    let asset = github::pick_asset(&release, &target)?;
    eprintln!("==> Downloading {}...", asset.name);

    let data = github::download_asset(&asset)?;
    eprintln!("==> Installing...");

    install::install(&repo, &asset.name, &data)?;
    install::save_manifest(&owner, &repo, &release.tag_name, &repo)?;
    eprintln!("==> Installed {repo} ({})", release.tag_name);

    Ok(())
}

fn run_list() -> Result<(), String> {
    let bin_dir = install::bin_dir()?;
    let manifest_dir = install::manifest_dir()?;

    if !manifest_dir.exists() {
        eprintln!("No packages installed.");
        return Ok(());
    }

    let entries: Vec<_> = std::fs::read_dir(&manifest_dir)
        .map_err(|e| format!("Failed to read manifest dir: {e}"))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .collect();

    if entries.is_empty() {
        eprintln!("No packages installed.");
        return Ok(());
    }

    for entry in entries {
        let content = std::fs::read_to_string(entry.path())
            .map_err(|e| format!("Failed to read manifest: {e}"))?;
        if let Ok(manifest) = serde_json::from_str::<install::Manifest>(&content) {
            let status = if bin_dir.join(&manifest.binary).exists() {
                "ok"
            } else {
                "missing"
            };
            println!(
                "{}/{} {} [{}] ({})",
                manifest.owner, manifest.repo, manifest.version, status, manifest.binary
            );
        }
    }

    Ok(())
}

fn run_remove(package: &str) -> Result<(), String> {
    let (owner, repo) = parse_package(package)?;
    install::uninstall(&owner, &repo)?;
    eprintln!("==> Removed {owner}/{repo}");
    Ok(())
}

fn parse_package(package: &str) -> Result<(String, String), String> {
    let parts: Vec<&str> = package.split('/').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err(format!("Invalid package format: {package:?}. Expected org/repo"));
    }
    Ok((parts[0].to_string(), parts[1].to_string()))
}