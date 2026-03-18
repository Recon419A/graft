use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Cursor, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub owner: String,
    pub repo: String,
    pub version: String,
    pub binary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python_env: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python_modules: Option<Vec<String>>,
}

pub fn bin_dir() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("Could not determine home directory")?;
    let dir = home.join(".graft").join("bin");
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create bin dir: {e}"))?;
    Ok(dir)
}

pub fn manifest_dir() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("Could not determine home directory")?;
    let dir = home.join(".graft").join("manifests");
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create manifest dir: {e}"))?;
    Ok(dir)
}

pub fn python_venv_dir() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("Could not determine home directory")?;
    Ok(home.join(".graft").join("python"))
}

pub fn ensure_python_venv() -> Result<PathBuf, String> {
    let venv_dir = python_venv_dir()?;

    if venv_dir.join("bin").join("python3").exists() {
        return Ok(venv_dir);
    }

    eprintln!("==> Creating shared Python venv at {}...", venv_dir.display());
    fs::create_dir_all(venv_dir.parent().unwrap())
        .map_err(|e| format!("Failed to create graft dir: {e}"))?;

    let output = Command::new("python3")
        .args(["-m", "venv", "--system-site-packages"])
        .arg(&venv_dir)
        .output()
        .map_err(|e| format!("Failed to create Python venv: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Failed to create Python venv: {stderr}"));
    }

    Ok(venv_dir)
}

pub fn install_python_wrapper(
    script_name: &str,
    venv_path: &Path,
) -> Result<(), String> {
    let bin_dir = bin_dir()?;
    let wrapper_path = bin_dir.join(script_name);
    let venv_str = venv_path.display();

    let wrapper = format!(
        "#!/bin/bash\nVIRTUAL_ENV=\"{venv_str}\" exec \"{venv_str}/bin/python3\" \"{venv_str}/bin/{script_name}\" \"$@\"\n"
    );

    fs::write(&wrapper_path, wrapper)
        .map_err(|e| format!("Failed to write wrapper script: {e}"))?;
    fs::set_permissions(&wrapper_path, fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("Failed to set wrapper permissions: {e}"))?;

    Ok(())
}

pub fn install(repo: &str, asset_name: &str, data: &[u8]) -> Result<(), String> {
    let bin_dir = bin_dir()?;
    let name_lower = asset_name.to_lowercase();

    if name_lower.ends_with(".tar.gz") || name_lower.ends_with(".tgz") {
        install_from_tar_gz(repo, data, &bin_dir)
    } else if name_lower.ends_with(".zip") {
        install_from_zip(repo, data, &bin_dir)
    } else {
        // Assume it's a bare binary.
        install_bare_binary(repo, data, &bin_dir)
    }
}

fn install_from_tar_gz(repo: &str, data: &[u8], bin_dir: &Path) -> Result<(), String> {
    let decoder = flate2::read::GzDecoder::new(Cursor::new(data));
    let mut archive = tar::Archive::new(decoder);

    let entries = archive
        .entries()
        .map_err(|e| format!("Failed to read tar entries: {e}"))?;

    for entry in entries {
        let mut entry = entry.map_err(|e| format!("Failed to read tar entry: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("Failed to read entry path: {e}"))?
            .to_path_buf();

        // Look for executable files — either the repo name or any file in a bin/ directory.
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();

        if file_name.is_empty() || entry.header().entry_type().is_dir() {
            continue;
        }

        let is_target = file_name == repo
            || file_name == format!("{repo}.exe")
            || path.components().any(|c| c.as_os_str() == "bin");

        // Also accept if the entry is marked executable.
        let mode = entry.header().mode().unwrap_or(0);
        let is_executable = mode & 0o111 != 0;

        if is_target || is_executable {
            let dest = bin_dir.join(file_name);
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .map_err(|e| format!("Failed to read entry: {e}"))?;

            // Skip if this looks like a directory or empty.
            if buf.is_empty() {
                continue;
            }

            fs::write(&dest, &buf).map_err(|e| format!("Failed to write binary: {e}"))?;
            fs::set_permissions(&dest, fs::Permissions::from_mode(0o755))
                .map_err(|e| format!("Failed to set permissions: {e}"))?;

            // If we found something matching the repo name, we're done.
            if file_name == repo || file_name == format!("{repo}.exe") {
                return Ok(());
            }
        }
    }

    // If we didn't find an exact repo-name match, that's still OK — we extracted executables.
    Ok(())
}

fn install_from_zip(repo: &str, data: &[u8], bin_dir: &Path) -> Result<(), String> {
    let cursor = Cursor::new(data);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|e| format!("Failed to read zip: {e}"))?;

    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| format!("Failed to read zip entry: {e}"))?;

        if file.is_dir() {
            continue;
        }

        let file_name = file
            .enclosed_name()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_default();

        if file_name.is_empty() {
            continue;
        }

        let is_target = file_name == repo
            || file_name == format!("{repo}.exe")
            || file
                .enclosed_name()
                .map(|p| p.components().any(|c| c.as_os_str() == "bin"))
                .unwrap_or(false);

        let mode = file.unix_mode().unwrap_or(0);
        let is_executable = mode & 0o111 != 0;

        if is_target || is_executable {
            let dest = bin_dir.join(&file_name);
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)
                .map_err(|e| format!("Failed to read zip entry: {e}"))?;

            if buf.is_empty() {
                continue;
            }

            fs::write(&dest, &buf).map_err(|e| format!("Failed to write binary: {e}"))?;
            fs::set_permissions(&dest, fs::Permissions::from_mode(0o755))
                .map_err(|e| format!("Failed to set permissions: {e}"))?;

            if file_name == repo || file_name == format!("{repo}.exe") {
                return Ok(());
            }
        }
    }

    Ok(())
}

fn install_bare_binary(repo: &str, data: &[u8], bin_dir: &Path) -> Result<(), String> {
    let dest = bin_dir.join(repo);
    fs::write(&dest, data).map_err(|e| format!("Failed to write binary: {e}"))?;
    fs::set_permissions(&dest, fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("Failed to set permissions: {e}"))?;
    Ok(())
}

pub fn save_manifest(
    owner: &str,
    repo: &str,
    version: &str,
    binary: &str,
    python_env: bool,
    python_modules: Vec<String>,
) -> Result<(), String> {
    let dir = manifest_dir()?;
    let manifest = Manifest {
        owner: owner.to_string(),
        repo: repo.to_string(),
        version: version.to_string(),
        binary: binary.to_string(),
        python_env: if python_env { Some(true) } else { None },
        python_modules: if python_modules.is_empty() {
            None
        } else {
            Some(python_modules)
        },
    };
    let json =
        serde_json::to_string_pretty(&manifest).map_err(|e| format!("Failed to serialize: {e}"))?;
    let path = dir.join(format!("{repo}.json"));
    fs::write(&path, json).map_err(|e| format!("Failed to write manifest: {e}"))?;
    Ok(())
}

pub fn uninstall(owner: &str, repo: &str) -> Result<(), String> {
    let manifest_path = manifest_dir()?.join(format!("{repo}.json"));

    if manifest_path.exists() {
        let content =
            fs::read_to_string(&manifest_path).map_err(|e| format!("Failed to read manifest: {e}"))?;
        let manifest: Manifest =
            serde_json::from_str(&content).map_err(|e| format!("Failed to parse manifest: {e}"))?;

        if manifest.owner != owner {
            return Err(format!(
                "Manifest owner mismatch: expected {owner}, found {}",
                manifest.owner
            ));
        }

        // Remove the binary/wrapper from bin dir.
        let bin_path = bin_dir()?.join(&manifest.binary);
        if bin_path.exists() {
            fs::remove_file(&bin_path).map_err(|e| format!("Failed to remove binary: {e}"))?;
        }

        // Clean up Python modules from the shared venv if applicable.
        if manifest.python_env.unwrap_or(false)
            && let Some(modules) = &manifest.python_modules
            && let Ok(venv_dir) = python_venv_dir()
        {
            cleanup_python_modules(&venv_dir, modules);
        }

        fs::remove_file(&manifest_path)
            .map_err(|e| format!("Failed to remove manifest: {e}"))?;
    } else {
        // Best effort: just remove binary with repo name.
        let bin_path = bin_dir()?.join(repo);
        if bin_path.exists() {
            fs::remove_file(&bin_path).map_err(|e| format!("Failed to remove binary: {e}"))?;
        } else {
            return Err(format!("Package {owner}/{repo} is not installed"));
        }
    }

    Ok(())
}

fn cleanup_python_modules(venv_dir: &Path, modules: &[String]) {
    // Find the site-packages directory inside the venv.
    let lib_dir = venv_dir.join("lib");
    if !lib_dir.exists() {
        return;
    }

    let Ok(entries) = fs::read_dir(&lib_dir) else {
        return;
    };

    for entry in entries.filter_map(|e| e.ok()) {
        let site_packages = entry.path().join("site-packages");
        if !site_packages.exists() {
            continue;
        }

        for module in modules {
            let module_dir = site_packages.join(module);
            if module_dir.exists() {
                eprintln!("==> Removing Python module: {module}");
                let _ = fs::remove_dir_all(&module_dir);
            }
        }
    }
}
