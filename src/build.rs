use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A build failure that may contain parseable information about missing dependencies.
#[derive(Debug)]
pub struct BuildError {
    pub message: String,
    pub missing_deps: Vec<String>,
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// Metadata about what a build produced, beyond just binaries.
#[derive(Debug, Default)]
pub struct BuildResult {
    pub binaries: Vec<PathBuf>,
    pub is_python_project: bool,
    pub python_modules: Vec<String>,
}

#[derive(Debug)]
pub enum BuildSystem {
    Cargo,
    Meson,
    CMake,
    Make,
}

impl std::fmt::Display for BuildSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildSystem::Cargo => write!(f, "cargo"),
            BuildSystem::Meson => write!(f, "meson"),
            BuildSystem::CMake => write!(f, "cmake"),
            BuildSystem::Make => write!(f, "make"),
        }
    }
}

pub fn download_source_tarball(owner: &str, repo: &str, tag: &str) -> Result<Vec<u8>, String> {
    let url = format!("https://github.com/{owner}/{repo}/archive/refs/tags/{tag}.tar.gz");

    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))?;

    let resp = client
        .get(&url)
        .header("User-Agent", "graft-pm")
        .send()
        .map_err(|e| format!("Failed to download source tarball: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!(
            "Failed to download source tarball ({}): {}",
            resp.status(),
            url
        ));
    }

    resp.bytes()
        .map(|b| b.to_vec())
        .map_err(|e| format!("Failed to read source tarball: {e}"))
}

pub fn extract_source(data: &[u8], dest: &Path) -> Result<PathBuf, String> {
    let decoder = flate2::read::GzDecoder::new(Cursor::new(data));
    let mut archive = tar::Archive::new(decoder);

    archive
        .unpack(dest)
        .map_err(|e| format!("Failed to extract source tarball: {e}"))?;

    // GitHub tarballs extract to a single top-level directory like "repo-tag/"
    let entries: Vec<_> = fs::read_dir(dest)
        .map_err(|e| format!("Failed to read extracted dir: {e}"))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();

    if entries.len() == 1 {
        Ok(entries[0].path())
    } else {
        Ok(dest.to_path_buf())
    }
}

pub fn detect_build_system(source_dir: &Path) -> Result<BuildSystem, String> {
    if source_dir.join("Cargo.toml").exists() {
        Ok(BuildSystem::Cargo)
    } else if source_dir.join("meson.build").exists() {
        Ok(BuildSystem::Meson)
    } else if source_dir.join("CMakeLists.txt").exists() {
        Ok(BuildSystem::CMake)
    } else if source_dir.join("Makefile").exists() || source_dir.join("makefile").exists() {
        Ok(BuildSystem::Make)
    } else {
        let contents: Vec<String> = fs::read_dir(source_dir)
            .map_err(|e| format!("Failed to list source dir: {e}"))?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        Err(format!(
            "Could not detect build system. Files found:\n  {}",
            contents.join("\n  ")
        ))
    }
}

/// Check if the source tree is a Python project (has requirements.txt or Python sources).
pub fn is_python_project(source_dir: &Path) -> bool {
    source_dir.join("requirements.txt").exists()
        || source_dir.join("setup.py").exists()
        || source_dir.join("pyproject.toml").exists()
}

pub fn run_build(
    build_system: &BuildSystem,
    source_dir: &Path,
    repo: &str,
    venv_path: Option<&Path>,
) -> Result<BuildResult, BuildError> {
    // Install Python dependencies before building, if a venv is provided.
    if let Some(venv) = venv_path {
        install_python_deps(source_dir, venv)?;
    }

    let binaries = match build_system {
        BuildSystem::Cargo => build_cargo(source_dir, repo)?,
        BuildSystem::Meson => return build_meson(source_dir, repo, venv_path),
        BuildSystem::CMake => build_cmake(source_dir, repo)?,
        BuildSystem::Make => build_make(source_dir, repo)?,
    };

    Ok(BuildResult {
        binaries,
        is_python_project: venv_path.is_some(),
        python_modules: Vec::new(),
    })
}

fn install_python_deps(source_dir: &Path, venv_path: &Path) -> Result<(), BuildError> {
    let requirements = source_dir.join("requirements.txt");
    if !requirements.exists() {
        return Ok(());
    }

    eprintln!("==> Installing Python dependencies into shared venv...");

    let venv_python = venv_path.join("bin").join("python3");

    // Try uv first, fall back to the venv's pip.
    let venv_pip = venv_path.join("bin").join("pip");

    let mut cmd = if which_exists("uv") {
        let mut c = Command::new("uv");
        c.args(["pip", "install", "--python"]);
        c.arg(&venv_python);
        c.arg("-r");
        c
    } else {
        let mut c = Command::new(&venv_pip);
        c.args(["install", "-r"]);
        c
    };

    cmd.arg(&requirements);
    cmd.current_dir(source_dir);

    run_cmd(&mut cmd, "install Python dependencies")?;

    Ok(())
}

fn which_exists(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .output()
        .is_ok_and(|o| o.status.success())
}

struct CmdOutput {
    #[allow(dead_code)]
    stdout: String,
    #[allow(dead_code)]
    stderr: String,
}

fn run_cmd(cmd: &mut Command, description: &str) -> Result<CmdOutput, BuildError> {
    eprintln!("    $ {} {}", cmd.get_program().to_string_lossy(),
        cmd.get_args().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" "));

    let output = cmd
        .output()
        .map_err(|e| BuildError {
            message: format!("Failed to run {description}: {e}"),
            missing_deps: Vec::new(),
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        let combined = format!("{stdout}\n{stderr}");
        let missing_deps = parse_missing_deps(&combined);
        return Err(BuildError {
            message: format!(
                "{description} failed (exit {}):\n{combined}",
                output.status.code().unwrap_or(-1)
            ),
            missing_deps,
        });
    }

    Ok(CmdOutput { stdout, stderr })
}

/// Parse build output for missing dependency names (pkg-config names, libraries, etc.)
fn parse_missing_deps(output: &str) -> Vec<String> {
    let mut deps = Vec::new();

    for line in output.lines() {
        // Meson: Dependency "libadwaita-1" not found
        if let Some(rest) = line.strip_prefix("meson.build:") {
            if rest.contains("not found") {
                if let Some(dep) = extract_quoted(rest) {
                    deps.push(dep);
                }
            }
        }
        // Also match the ERROR: line form
        if line.contains("ERROR: Dependency") && line.contains("not found") {
            if let Some(dep) = extract_quoted(line) {
                deps.push(dep);
            }
        }
        // CMake: Could not find a package configuration file provided by "..."
        if line.contains("Could not find a package configuration file provided by") {
            if let Some(dep) = extract_quoted(line) {
                deps.push(dep);
            }
        }
        // CMake: -- Could NOT find PkgName
        if line.contains("Could NOT find") {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("-- Could NOT find ") {
                let name = rest.split_whitespace().next().unwrap_or(rest);
                deps.push(name.to_string());
            }
        }
        // pkg-config: Package 'foo' not found
        if line.contains("Package '") && line.contains("' not found") {
            if let Some(start) = line.find("Package '") {
                let rest = &line[start + 9..];
                if let Some(end) = rest.find('\'') {
                    deps.push(rest[..end].to_string());
                }
            }
        }
    }

    deps.sort();
    deps.dedup();
    deps
}

fn extract_quoted(s: &str) -> Option<String> {
    // Try double quotes first, then single quotes.
    for quote in ['"', '\''] {
        if let Some(start) = s.find(quote) {
            let rest = &s[start + 1..];
            if let Some(end) = rest.find(quote) {
                let val = &rest[..end];
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}

fn build_cargo(source_dir: &Path, _repo: &str) -> Result<Vec<PathBuf>, BuildError> {
    run_cmd(
        Command::new("cargo")
            .arg("build")
            .arg("--release")
            .current_dir(source_dir),
        "cargo build",
    )?;

    find_built_binaries(&source_dir.join("target").join("release"))
        .map_err(|e| BuildError { message: e, missing_deps: Vec::new() })
}

fn build_meson(
    source_dir: &Path,
    repo: &str,
    venv_path: Option<&Path>,
) -> Result<BuildResult, BuildError> {
    let build_dir = source_dir.join("builddir");

    let mut setup_cmd = Command::new("meson");
    setup_cmd
        .arg("setup")
        .arg(&build_dir)
        .arg(source_dir)
        .current_dir(source_dir);

    // If we have a venv, tell meson to use it as prefix and make its Python visible.
    if let Some(venv) = venv_path {
        let venv_bin = venv.join("bin");
        let current_path = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{current_path}", venv_bin.display());

        setup_cmd
            .arg(format!("--prefix={}", venv.display()))
            .env("VIRTUAL_ENV", venv)
            .env("PATH", &new_path);
    }

    run_cmd(&mut setup_cmd, "meson setup")?;

    let mut compile_cmd = Command::new("meson");
    compile_cmd.arg("compile").arg("-C").arg(&build_dir);
    run_cmd(&mut compile_cmd, "meson compile")?;

    // If we have a venv, install directly into it (no staging).
    if let Some(venv) = venv_path {
        let venv_bin = venv.join("bin");
        let current_path = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{current_path}", venv_bin.display());

        // Snapshot the venv bin/ mtimes before meson install so we can diff.
        let before: std::collections::HashMap<String, std::time::SystemTime> =
            fs::read_dir(&venv_bin)
                .map(|entries| {
                    entries
                        .filter_map(|e| e.ok())
                        .filter_map(|e| {
                            let name = e.file_name().to_string_lossy().to_string();
                            let mtime = e.metadata().ok()?.modified().ok()?;
                            Some((name, mtime))
                        })
                        .collect()
                })
                .unwrap_or_default();

        let mut install_cmd = Command::new("meson");
        install_cmd.arg("install").arg("-C").arg(&build_dir);
        install_cmd
            .env("VIRTUAL_ENV", venv)
            .env("PATH", &new_path);

        run_cmd(&mut install_cmd, "meson install")?;

        // Find files that are new or modified since our snapshot.
        let binaries: Vec<PathBuf> = fs::read_dir(&venv_bin)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| {
                        if !e.path().is_file() {
                            return false;
                        }
                        let name = e.file_name().to_string_lossy().to_string();
                        let current_mtime = e.metadata().ok().and_then(|m| m.modified().ok());
                        match (before.get(&name), current_mtime) {
                            (None, _) => true,            // new file
                            (Some(old), Some(new)) => new > *old,  // modified
                            _ => false,
                        }
                    })
                    .map(|e| e.path())
                    .collect()
            })
            .unwrap_or_default();

        eprintln!("==> Meson installed {} file(s) to bin/", binaries.len());
        for b in &binaries {
            if let Some(name) = b.file_name() {
                eprintln!("    {}", name.to_string_lossy());
            }
        }

        // Detect which Python modules were installed into the venv's site-packages.
        let python_modules = find_installed_python_modules(venv, repo);

        return Ok(BuildResult {
            binaries,
            is_python_project: true,
            python_modules,
        });
    }

    // No venv: stage to a temp dir and pick out binaries (original behavior).
    let staging = source_dir.join("_graft_staging");
    fs::create_dir_all(&staging)
        .map_err(|e| BuildError { message: format!("Failed to create staging dir: {e}"), missing_deps: Vec::new() })?;

    run_cmd(
        Command::new("meson")
            .arg("install")
            .arg("-C")
            .arg(&build_dir)
            .arg("--destdir")
            .arg(&staging),
        "meson install",
    )?;

    let binaries = find_built_binaries_recursive(&staging)
        .map_err(|e| BuildError { message: e, missing_deps: Vec::new() })?;

    Ok(BuildResult {
        binaries,
        is_python_project: false,
        python_modules: Vec::new(),
    })
}

/// Scan the venv's site-packages for directories matching the repo name.
fn find_installed_python_modules(venv_path: &Path, repo: &str) -> Vec<String> {
    let lib_dir = venv_path.join("lib");
    let Ok(entries) = fs::read_dir(&lib_dir) else {
        return Vec::new();
    };

    let mut modules = Vec::new();
    // Normalize: cozy, com.github.geigi.cozy, etc.
    let repo_lower = repo.to_lowercase();

    for entry in entries.filter_map(|e| e.ok()) {
        let site_packages = entry.path().join("site-packages");
        if !site_packages.exists() {
            continue;
        }

        let Ok(sp_entries) = fs::read_dir(&site_packages) else {
            continue;
        };

        for sp_entry in sp_entries.filter_map(|e| e.ok()) {
            if !sp_entry.path().is_dir() {
                continue;
            }
            let name = sp_entry.file_name().to_string_lossy().to_string();
            // Skip standard venv/pip internals.
            if name.starts_with('_') || name == "pip" || name == "pkg_resources"
                || name.ends_with(".dist-info") || name.ends_with(".egg-info")
            {
                continue;
            }
            // Match the repo name (or a reasonable variant).
            if name.to_lowercase() == repo_lower
                || name.to_lowercase() == repo_lower.replace('-', "_")
                || name.to_lowercase().contains(&repo_lower)
            {
                modules.push(name);
            }
        }
    }

    modules
}

fn build_cmake(source_dir: &Path, _repo: &str) -> Result<Vec<PathBuf>, BuildError> {
    let build_dir = source_dir.join("build");

    run_cmd(
        Command::new("cmake")
            .arg("-B")
            .arg(&build_dir)
            .arg("-DCMAKE_BUILD_TYPE=Release")
            .current_dir(source_dir),
        "cmake configure",
    )?;

    run_cmd(
        Command::new("cmake")
            .arg("--build")
            .arg(&build_dir)
            .arg("--config")
            .arg("Release"),
        "cmake build",
    )?;

    find_built_binaries_recursive(&build_dir)
        .map_err(|e| BuildError { message: e, missing_deps: Vec::new() })
}

fn build_make(source_dir: &Path, _repo: &str) -> Result<Vec<PathBuf>, BuildError> {
    run_cmd(
        Command::new("make")
            .arg("-j")
            .arg(num_cpus().to_string())
            .current_dir(source_dir),
        "make",
    )?;

    // For make, look in the source dir itself and common output locations.
    let mut binaries = Vec::new();
    for subdir in [".", "bin", "build", "out"] {
        let dir = source_dir.join(subdir);
        if dir.exists() {
            binaries.extend(
                find_built_binaries(&dir)
                    .map_err(|e| BuildError { message: e, missing_deps: Vec::new() })?,
            );
        }
    }
    Ok(binaries)
}

fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

fn find_built_binaries(dir: &Path) -> Result<Vec<PathBuf>, String> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut binaries = Vec::new();
    let entries =
        fs::read_dir(dir).map_err(|e| format!("Failed to read build output dir: {e}"))?;

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_file() && is_executable(&path) && !is_build_artifact(&path) {
            binaries.push(path);
        }
    }

    Ok(binaries)
}

fn find_built_binaries_recursive(dir: &Path) -> Result<Vec<PathBuf>, String> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut binaries = Vec::new();
    walk_dir(dir, &mut binaries)?;
    Ok(binaries)
}

fn walk_dir(dir: &Path, binaries: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("Failed to read dir: {e}"))?;

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            walk_dir(&path, binaries)?;
        } else if path.is_file() && is_executable(&path) && !is_build_artifact(&path) {
            binaries.push(path);
        }
    }

    Ok(())
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::metadata(path) {
        meta.permissions().mode() & 0o111 != 0
    } else {
        false
    }
}

fn is_build_artifact(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();

    // Skip common build system artifacts that aren't the actual binary.
    name.ends_with(".o")
        || name.ends_with(".a")
        || name.ends_with(".so")
        || name.ends_with(".dylib")
        || name.ends_with(".d")
        || name.ends_with(".rmeta")
        || name.starts_with("lib")
        || name.starts_with("build-script-")
        || name.contains(".dSYM")
        || name == "build"
        || name == "deps"
}

pub fn pick_binary<'a>(binaries: &'a [PathBuf], repo: &str) -> Result<&'a PathBuf, String> {
    if binaries.is_empty() {
        return Err("Build produced no binaries".to_string());
    }

    let get_name = |b: &&PathBuf| -> Option<String> {
        b.file_name().and_then(|n| n.to_str()).map(|s| s.to_string())
    };

    // Exact repo name match.
    if let Some(b) = binaries.iter().find(|b| get_name(b).is_some_and(|n| n == repo)) {
        return Ok(b);
    }

    // Repo name with hyphens replaced by underscores (common in Cargo).
    let alt_name = repo.replace('-', "_");
    if let Some(b) = binaries.iter().find(|b| get_name(b).is_some_and(|n| n == alt_name)) {
        return Ok(b);
    }

    // Match dotted names like "com.github.geigi.cozy" — check if the last segment matches repo.
    if let Some(b) = binaries.iter().find(|b| {
        get_name(b).is_some_and(|n| {
            n.rsplit('.').next().is_some_and(|last| last.eq_ignore_ascii_case(repo))
        })
    }) {
        return Ok(b);
    }

    // Match if the binary name contains the repo name.
    if let Some(b) = binaries.iter().find(|b| {
        get_name(b).is_some_and(|n| n.to_lowercase().contains(&repo.to_lowercase()))
    }) {
        return Ok(b);
    }

    // If there's exactly one binary, use it.
    if binaries.len() == 1 {
        return Ok(&binaries[0]);
    }

    let names: Vec<&str> = binaries
        .iter()
        .filter_map(|b| b.file_name().and_then(|n| n.to_str()))
        .collect();
    Err(format!(
        "Multiple binaries found, couldn't determine which to install:\n  {}",
        names.join("\n  ")
    ))
}
