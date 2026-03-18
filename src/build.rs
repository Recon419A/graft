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

#[derive(Debug)]
pub enum BuildSystem {
    Cargo,
    Meson,
    CMake,
    Make,
    // TODO: Python (setup.py / pyproject.toml), autotools, etc.
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

pub fn run_build(
    build_system: &BuildSystem,
    source_dir: &Path,
    repo: &str,
) -> Result<Vec<PathBuf>, BuildError> {
    match build_system {
        BuildSystem::Cargo => build_cargo(source_dir, repo),
        BuildSystem::Meson => build_meson(source_dir, repo),
        BuildSystem::CMake => build_cmake(source_dir, repo),
        BuildSystem::Make => build_make(source_dir, repo),
    }
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

fn build_meson(source_dir: &Path, _repo: &str) -> Result<Vec<PathBuf>, BuildError> {
    let build_dir = source_dir.join("builddir");

    run_cmd(
        Command::new("meson")
            .arg("setup")
            .arg(&build_dir)
            .arg(source_dir)
            .current_dir(source_dir),
        "meson setup",
    )?;

    run_cmd(
        Command::new("meson")
            .arg("compile")
            .arg("-C")
            .arg(&build_dir),
        "meson compile",
    )?;

    // Meson install to a staging directory so we can pick out binaries.
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

    find_built_binaries_recursive(&staging)
        .map_err(|e| BuildError { message: e, missing_deps: Vec::new() })
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

    // Exact repo name match.
    if let Some(b) = binaries.iter().find(|b| {
        b.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n == repo)
    }) {
        return Ok(b);
    }

    // Repo name with hyphens replaced by underscores (common in Cargo).
    let alt_name = repo.replace('-', "_");
    if let Some(b) = binaries.iter().find(|b| {
        b.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n == alt_name)
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
