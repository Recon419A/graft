use serde::Deserialize;

use crate::platform::Target;

#[derive(Debug, Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
}

pub fn latest_release(owner: &str, repo: &str) -> Result<Release, String> {
    let url = format!("https://api.github.com/repos/{owner}/{repo}/releases/latest");

    let client = reqwest::blocking::Client::new();
    let resp = client
        .get(&url)
        .header("User-Agent", "graft-pm")
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("Failed to fetch release: {e}"))?;

    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(format!("No releases found for {owner}/{repo}"));
    }

    if !resp.status().is_success() {
        return Err(format!(
            "GitHub API error: {} {}",
            resp.status(),
            resp.text().unwrap_or_default()
        ));
    }

    resp.json::<Release>()
        .map_err(|e| format!("Failed to parse release JSON: {e}"))
}

pub fn pick_asset<'a>(release: &'a Release, target: &Target) -> Result<&'a Asset, String> {
    if release.assets.is_empty() {
        return Err("Release has no assets".to_string());
    }

    // Build a list of candidate name fragments to match against, in priority order.
    let os_patterns = target.os_patterns();
    let arch_patterns = target.arch_patterns();

    // First pass: find assets that match both OS and arch.
    let mut scored: Vec<(&Asset, usize)> = Vec::new();

    for asset in &release.assets {
        let name_lower = asset.name.to_lowercase();

        // Skip obvious non-binary assets.
        if name_lower.ends_with(".sha256")
            || name_lower.ends_with(".sha512")
            || name_lower.ends_with(".sig")
            || name_lower.ends_with(".asc")
            || name_lower.ends_with(".sbom")
            || name_lower.ends_with(".txt")
            || name_lower.ends_with(".deb")
            || name_lower.ends_with(".rpm")
            || name_lower.ends_with(".msi")
            || name_lower.ends_with(".dmg")
            || name_lower.ends_with(".pkg")
        {
            continue;
        }

        let os_match = os_patterns.iter().any(|p| name_lower.contains(p));
        let arch_match = arch_patterns.iter().any(|p| name_lower.contains(p));

        if os_match && arch_match {
            // Prefer .tar.gz > .zip > bare binary.
            let format_score = if name_lower.ends_with(".tar.gz") || name_lower.ends_with(".tgz") {
                3
            } else if name_lower.ends_with(".zip") {
                2
            } else {
                1
            };
            scored.push((asset, format_score));
        }
    }

    scored.sort_by(|a, b| b.1.cmp(&a.1));

    if let Some((asset, _)) = scored.first() {
        return Ok(asset);
    }

    // Helpful error: list what we found.
    let available: Vec<&str> = release.assets.iter().map(|a| a.name.as_str()).collect();
    Err(format!(
        "No matching asset for platform {}-{}. Available assets:\n  {}",
        target.os,
        target.arch,
        available.join("\n  ")
    ))
}

pub fn download_asset(asset: &Asset) -> Result<Vec<u8>, String> {
    let client = reqwest::blocking::Client::new();
    let resp = client
        .get(&asset.browser_download_url)
        .header("User-Agent", "graft-pm")
        .send()
        .map_err(|e| format!("Failed to download asset: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Download failed: {}", resp.status()));
    }

    resp.bytes()
        .map(|b| b.to_vec())
        .map_err(|e| format!("Failed to read download body: {e}"))
}