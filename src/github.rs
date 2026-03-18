use serde::Deserialize;
use std::process::Command;

use crate::platform::Target;

fn gh_token() -> Option<String> {
    Command::new("gh")
        .args(["auth", "token"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|t| !t.is_empty())
}

fn api_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .unwrap_or_else(|_| reqwest::blocking::Client::new())
}

fn api_get(client: &reqwest::blocking::Client, url: &str) -> Result<reqwest::blocking::Response, String> {
    let mut req = client
        .get(url)
        .header("User-Agent", "graft-pm")
        .header("Accept", "application/vnd.github+json");

    if let Some(token) = gh_token() {
        req = req.header("Authorization", format!("Bearer {token}"));
    }

    req.send().map_err(|e| format!("Request failed: {e}"))
}

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

#[derive(Debug, Deserialize)]
struct Tag {
    name: String,
}

pub fn latest_release(owner: &str, repo: &str) -> Result<Release, String> {
    let client = api_client();

    // Try the releases API first.
    let url = format!("https://api.github.com/repos/{owner}/{repo}/releases/latest");
    let resp = api_get(&client, &url)?;

    if resp.status().is_success() {
        return resp
            .json::<Release>()
            .map_err(|e| format!("Failed to parse release JSON: {e}"));
    }

    // Fall back to tags API — the repo may have tags but no formal releases.
    let tags_url = format!("https://api.github.com/repos/{owner}/{repo}/tags?per_page=1");
    let resp = api_get(&client, &tags_url)?;

    if !resp.status().is_success() {
        return Err(format!("No releases or tags found for {owner}/{repo}"));
    }

    let tags: Vec<Tag> = resp
        .json()
        .map_err(|e| format!("Failed to parse tags JSON: {e}"))?;

    if let Some(tag) = tags.first() {
        Ok(Release {
            tag_name: tag.name.clone(),
            assets: Vec::new(),
        })
    } else {
        Err(format!("No releases or tags found for {owner}/{repo}"))
    }
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
    let client = api_client();
    let resp = api_get(&client, &asset.browser_download_url)?;

    if !resp.status().is_success() {
        return Err(format!("Download failed: {}", resp.status()));
    }

    resp.bytes()
        .map(|b| b.to_vec())
        .map_err(|e| format!("Failed to read download body: {e}"))
}