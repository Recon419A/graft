use std::path::Path;
use std::process::Command;

#[derive(Debug)]
pub enum PackageManager {
    Portage,  // Gentoo
    Apt,      // Debian/Ubuntu
    Dnf,      // Fedora/RHEL
    Pacman,   // Arch
    Zypper,   // openSUSE
    Apk,      // Alpine
}

impl std::fmt::Display for PackageManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackageManager::Portage => write!(f, "portage"),
            PackageManager::Apt => write!(f, "apt"),
            PackageManager::Dnf => write!(f, "dnf"),
            PackageManager::Pacman => write!(f, "pacman"),
            PackageManager::Zypper => write!(f, "zypper"),
            PackageManager::Apk => write!(f, "apk"),
        }
    }
}

impl PackageManager {
    pub fn install_cmd(&self) -> (&str, Vec<&str>) {
        match self {
            PackageManager::Portage => ("emerge", vec!["--ask"]),
            PackageManager::Apt => ("apt", vec!["install"]),
            PackageManager::Dnf => ("dnf", vec!["install"]),
            PackageManager::Pacman => ("pacman", vec!["-S"]),
            PackageManager::Zypper => ("zypper", vec!["install"]),
            PackageManager::Apk => ("apk", vec!["add"]),
        }
    }

    /// Try to find which system package provides a given pkg-config name.
    pub fn find_provider(&self, pkg_config_name: &str) -> Option<String> {
        match self {
            PackageManager::Portage => find_provider_portage(pkg_config_name),
            PackageManager::Apt => find_provider_apt(pkg_config_name),
            PackageManager::Dnf => find_provider_dnf(pkg_config_name),
            PackageManager::Pacman => find_provider_pacman(pkg_config_name),
            _ => None,
        }
    }

    /// Format a human-readable install command for the given packages.
    pub fn install_hint(&self, packages: &[String]) -> String {
        let (cmd, args) = self.install_cmd();
        format!(
            "sudo {cmd} {} {}",
            args.join(" "),
            packages.join(" ")
        )
    }
}

pub fn detect() -> Result<PackageManager, String> {
    // Check for package manager binaries in priority order.
    // Portage first since we're dogfooding on Gentoo.
    if Path::new("/usr/bin/emerge").exists() || which("emerge") {
        return Ok(PackageManager::Portage);
    }
    if which("apt") {
        return Ok(PackageManager::Apt);
    }
    if which("dnf") {
        return Ok(PackageManager::Dnf);
    }
    if which("pacman") {
        return Ok(PackageManager::Pacman);
    }
    if which("zypper") {
        return Ok(PackageManager::Zypper);
    }
    if which("apk") {
        return Ok(PackageManager::Apk);
    }

    Err("Could not detect system package manager".to_string())
}

fn which(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .output()
        .is_ok_and(|o| o.status.success())
}

fn find_provider_portage(pkg_config_name: &str) -> Option<String> {
    // Try equery first (from gentoolkit) — works for installed packages.
    let pc_file = format!("{pkg_config_name}.pc");
    if let Ok(output) = Command::new("equery")
        .args(["belongs", "-e", &pc_file])
        .output()
        && output.status.success()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Some(line) = stdout.lines().next() {
            let pkg = line.trim();
            if !pkg.is_empty() {
                if let Some(cat_name) = strip_portage_version(pkg) {
                    return Some(cat_name);
                }
                return Some(pkg.to_string());
            }
        }
    }

    // Try qfile (from portage-utils) — also only installed packages.
    if let Ok(output) = Command::new("qfile")
        .args(["-qC", &pc_file])
        .output()
        && output.status.success()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Some(line) = stdout.lines().next() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }

    // Search the portage tree directly — works even for uninstalled packages.
    // The pkg-config name often maps to the ebuild name. Strip the version
    // suffix from the pc name (e.g., "libadwaita-1" → "libadwaita").
    let search_name = strip_pc_version(pkg_config_name);

    for repo_path in ["/var/db/repos/gentoo", "/var/db/repos"] {
        let repo = std::path::Path::new(repo_path);
        if !repo.exists() {
            continue;
        }

        // Walk category dirs.
        let Ok(categories) = std::fs::read_dir(repo) else {
            continue;
        };
        for cat_entry in categories.filter_map(|e| e.ok()) {
            let cat_path = cat_entry.path();
            if !cat_path.is_dir() {
                continue;
            }
            let cat_name = cat_entry.file_name();
            let cat_str = cat_name.to_string_lossy();
            // Skip non-category dirs.
            if !cat_str.contains('-') && cat_str != "virtual" {
                continue;
            }
            // Check if a package dir matching our search name exists.
            let pkg_dir = cat_path.join(&search_name);
            if pkg_dir.is_dir() {
                return Some(format!("{cat_str}/{search_name}"));
            }
        }
    }

    None
}

/// Strip the version suffix from a pkg-config name.
/// e.g., "libadwaita-1" → "libadwaita", "gtk4" → "gtk4", "glib-2.0" → "glib"
fn strip_pc_version(name: &str) -> String {
    // Walk backwards: if the name ends with "-N" or "-N.M" where N is a digit, strip it.
    if let Some(idx) = name.rfind('-') {
        let suffix = &name[idx + 1..];
        if suffix.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return name[..idx].to_string();
        }
    }
    name.to_string()
}

fn strip_portage_version(pkg: &str) -> Option<String> {
    // "gui-libs/libadwaita-1.6.0" → split on '/' to get "gui-libs" and "libadwaita-1.6.0"
    // Then strip the version from the package name.
    let (cat, name_ver) = pkg.split_once('/')?;
    // Portage versions start after the last hyphen followed by a digit.
    // Walk backwards to find it.
    let bytes = name_ver.as_bytes();
    let mut last_hyphen_before_digit = None;
    for i in (0..bytes.len().saturating_sub(1)).rev() {
        if bytes[i] == b'-' && bytes.get(i + 1).is_some_and(|b| b.is_ascii_digit()) {
            last_hyphen_before_digit = Some(i);
            break;
        }
    }

    if let Some(idx) = last_hyphen_before_digit {
        Some(format!("{cat}/{}", &name_ver[..idx]))
    } else {
        Some(format!("{cat}/{name_ver}"))
    }
}

fn find_provider_apt(pkg_config_name: &str) -> Option<String> {
    // apt-file search looks for files in packages (installed or not).
    let pc_file = format!("{pkg_config_name}.pc");
    if let Ok(output) = Command::new("apt-file")
        .args(["search", &pc_file])
        .output()
        && output.status.success()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Output: "libadwaita-1-dev: /usr/lib/x86_64-linux-gnu/pkgconfig/libadwaita-1.pc"
        if let Some(line) = stdout.lines().next()
            && let Some(pkg) = line.split(':').next()
        {
            return Some(pkg.trim().to_string());
        }
    }

    // Fallback: guess the -dev package name.
    Some(format!("{pkg_config_name}-dev"))
}

fn find_provider_dnf(pkg_config_name: &str) -> Option<String> {
    let pc_pattern = format!("*/{pkg_config_name}.pc");
    if let Ok(output) = Command::new("dnf")
        .args(["provides", &pc_pattern])
        .output()
        && output.status.success()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Output lines like: "libadwaita-devel-1.4.0-1.fc39.x86_64 : ..."
        for line in stdout.lines() {
            if line.contains("-devel")
                && let Some(pkg) = line.split_whitespace().next()
            {
                // Strip the version/arch suffix.
                if let Some(name) = pkg.rsplit_once('-') {
                    return Some(name.0.to_string());
                }
                return Some(pkg.to_string());
            }
        }
    }

    Some(format!("{pkg_config_name}-devel"))
}

fn find_provider_pacman(pkg_config_name: &str) -> Option<String> {
    let pc_pattern = format!("{pkg_config_name}.pc");
    if let Ok(output) = Command::new("pkgfile")
        .args(["-s", &pc_pattern])
        .output()
        && output.status.success()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Some(line) = stdout.lines().next() {
            // pkgfile output: "extra/libadwaita"
            if let Some(pkg) = line.split('/').nth(1) {
                return Some(pkg.trim().to_string());
            }
            return Some(line.trim().to_string());
        }
    }

    None
}
