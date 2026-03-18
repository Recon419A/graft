#[derive(Debug)]
pub struct Target {
    pub os: String,
    pub arch: String,
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-{}", self.os, self.arch)
    }
}

impl Target {
    pub fn os_patterns(&self) -> Vec<&str> {
        match self.os.as_str() {
            "linux" => vec!["linux"],
            "macos" => vec!["darwin", "macos", "apple"],
            "windows" => vec!["windows", "win64", "win"],
            _ => vec![self.os.as_str()],
        }
    }

    pub fn arch_patterns(&self) -> Vec<&str> {
        match self.arch.as_str() {
            "x86_64" => vec!["x86_64", "amd64", "x64"],
            "aarch64" => vec!["aarch64", "arm64"],
            _ => vec![self.arch.as_str()],
        }
    }
}

pub fn detect() -> Result<Target, String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    let os = match os {
        "linux" => "linux",
        "macos" => "macos",
        "windows" => "windows",
        other => return Err(format!("Unsupported OS: {other}")),
    };

    let arch = match arch {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => return Err(format!("Unsupported architecture: {other}")),
    };

    Ok(Target {
        os: os.to_string(),
        arch: arch.to_string(),
    })
}