mod build;
mod github;
mod install;
mod platform;
mod system;

use clap::{Parser, Subcommand};
use std::process::{self, Command};

#[derive(Parser)]
#[command(name = "graft", version, about = "Trust maintainers. Install the latest GitHub release.")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Install a package: org/repo
    #[arg(value_name = "ORG/REPO")]
    package: Option<String>,

    /// When a source build fails due to missing system libraries, attempt to
    /// install them via the system package manager (e.g. emerge, apt, dnf).
    /// This will prompt for elevated privileges if needed.
    #[arg(long)]
    install_deps: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Install a package from a GitHub release
    Install {
        /// GitHub repository (org/repo)
        package: String,

        /// Attempt to install missing system dependencies
        #[arg(long)]
        install_deps: bool,
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
        Some(Commands::Install {
            package,
            install_deps,
        }) => run_install(&package, install_deps),
        Some(Commands::List) => run_list(),
        Some(Commands::Remove { package }) => run_remove(&package),
        None => {
            if let Some(package) = cli.package {
                run_install(&package, cli.install_deps)
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

fn run_install(package: &str, install_deps: bool) -> Result<(), String> {
    let (owner, repo) = parse_package(package)?;

    eprintln!("==> Finding latest release for {owner}/{repo}...");
    let release = github::latest_release(&owner, &repo)?;
    eprintln!("==> Found release: {}", release.tag_name);

    let target = platform::detect()?;
    eprintln!("==> Detected platform: {target}");

    // Try pre-built asset first.
    match github::pick_asset(&release, &target) {
        Ok(asset) => {
            eprintln!("==> Downloading {}...", asset.name);
            let data = github::download_asset(asset)?;
            eprintln!("==> Installing...");
            install::install(&repo, &asset.name, &data)?;
            install::save_manifest(&owner, &repo, &release.tag_name, &repo)?;
            eprintln!("==> Installed {repo} ({})", release.tag_name);
        }
        Err(_) => {
            eprintln!("==> No pre-built binary found, building from source...");
            build_from_source(&owner, &repo, &release.tag_name, install_deps)?;
        }
    }

    Ok(())
}

fn build_from_source(
    owner: &str,
    repo: &str,
    tag: &str,
    install_deps: bool,
) -> Result<(), String> {
    eprintln!("==> Downloading source for {tag}...");
    let tarball = build::download_source_tarball(owner, repo, tag)?;

    let tmp = tempfile::tempdir().map_err(|e| format!("Failed to create temp dir: {e}"))?;
    let source_dir = build::extract_source(&tarball, tmp.path())?;
    eprintln!("==> Extracted to {}", source_dir.display());

    let build_system = build::detect_build_system(&source_dir)?;
    eprintln!("==> Detected build system: {build_system}");

    eprintln!("==> Building...");
    let binaries = match build::run_build(&build_system, &source_dir, repo) {
        Ok(bins) => bins,
        Err(build_err) => {
            if build_err.missing_deps.is_empty() {
                return Err(build_err.message);
            }
            return handle_missing_deps(&build_err, install_deps, || {
                // Retry the build after installing deps.
                build_from_source(owner, repo, tag, false)
            });
        }
    };

    let binary = build::pick_binary(&binaries, repo)
        .map_err(|e| e.to_string())?;
    let binary_name = binary
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(repo);
    eprintln!("==> Found binary: {binary_name}");

    let bin_dir = install::bin_dir()?;
    let dest = bin_dir.join(binary_name);
    std::fs::copy(binary, &dest).map_err(|e| format!("Failed to copy binary: {e}"))?;

    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("Failed to set permissions: {e}"))?;

    install::save_manifest(owner, repo, tag, binary_name)?;
    eprintln!("==> Installed {repo} ({tag}) [built from source]");

    Ok(())
}

fn handle_missing_deps<F>(
    build_err: &build::BuildError,
    install_deps: bool,
    retry: F,
) -> Result<(), String>
where
    F: FnOnce() -> Result<(), String>,
{
    let pm = system::detect();

    eprintln!();
    eprintln!("[error] Build failed due to missing system libraries:");
    for dep in &build_err.missing_deps {
        eprintln!("  - {dep}");
    }

    // Try to resolve pkg-config names to system package names.
    let resolved: Vec<(String, Option<String>)> = build_err
        .missing_deps
        .iter()
        .map(|dep| {
            let pkg = pm.as_ref().ok().and_then(|pm| pm.find_provider(dep));
            (dep.clone(), pkg)
        })
        .collect();

    let has_all_resolved = resolved.iter().all(|(_, pkg)| pkg.is_some());
    let packages: Vec<String> = resolved
        .iter()
        .map(|(dep, pkg)| pkg.clone().unwrap_or_else(|| format!("???({dep})")))
        .collect();

    if let Ok(ref pm) = pm {
        eprintln!();
        if has_all_resolved {
            eprintln!(
                "[hint] Install them with:\n  {}",
                pm.install_hint(&packages)
            );
        } else {
            eprintln!("[hint] Some dependencies could not be resolved to system packages.");
            eprintln!("       Known packages: {}", pm.install_hint(&packages));
            eprintln!(
                "       Unresolved: {}",
                resolved
                    .iter()
                    .filter(|(_, pkg)| pkg.is_none())
                    .map(|(dep, _)| dep.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }

    if !install_deps {
        eprintln!();
        eprintln!("[hint] Re-run with --install-deps to have graft install them for you.");
        return Err("missing system dependencies".to_string());
    }

    if !has_all_resolved {
        return Err(
            "Cannot auto-install: some dependencies could not be resolved to system packages"
                .to_string(),
        );
    }

    let pm = pm.map_err(|e| format!("Cannot auto-install: {e}"))?;
    eprintln!();
    eprintln!("==> Installing system dependencies...");

    let (cmd_name, args) = pm.install_cmd();

    // Use sudo if we're not root and the package manager needs it.
    let need_sudo = !is_root() && !matches!(pm, system::PackageManager::Portage);

    let mut cmd = if need_sudo {
        let mut c = Command::new("sudo");
        c.arg(cmd_name);
        c
    } else {
        Command::new(cmd_name)
    };

    for arg in &args {
        cmd.arg(arg);
    }
    for pkg in &packages {
        cmd.arg(pkg);
    }

    eprintln!(
        "    $ {}",
        std::iter::once(if need_sudo { "sudo" } else { cmd_name })
            .chain(if need_sudo { vec![cmd_name] } else { vec![] })
            .chain(args.iter().copied())
            .chain(packages.iter().map(|s| s.as_str()))
            .collect::<Vec<_>>()
            .join(" ")
    );

    // Run interactively — inherit stdio so the user can respond to prompts.
    let status = cmd
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .map_err(|e| format!("Failed to run package manager: {e}"))?;

    if !status.success() {
        return Err(format!(
            "Package manager exited with status {}",
            status.code().unwrap_or(-1)
        ));
    }

    eprintln!("==> Dependencies installed, retrying build...");
    retry()
}

fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
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
