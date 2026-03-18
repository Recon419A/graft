# graft

**Trust maintainers.** Install software from GitHub with one command.

```
graft sharkdp/bat
```

That's it. graft finds the latest GitHub release, picks the right binary for your platform, and installs it to `~/.graft/bin/`.

If there's no pre-built binary, graft downloads the source and builds it.

## Install

```
cargo install rectifier
```

The crate is called `rectifier` because `graft` was taken on crates.io. The binary is still `graft`.

Add `~/.graft/bin` to your `PATH`:

```bash
export PATH="$HOME/.graft/bin:$PATH"
```

## Usage

```bash
# Install the latest release
graft org/repo

# Explicit install subcommand
graft install org/repo

# List installed packages
graft list

# Remove a package
graft remove org/repo
```

## How it works

1. **Pre-built binaries (fast path):** graft checks the latest GitHub Release for assets matching your OS and architecture. It handles `.tar.gz`, `.zip`, and bare binaries.

2. **Source builds (fallback):** If no matching binary is found, graft downloads the source tarball for the tag and auto-detects the build system:
   - `Cargo.toml` → `cargo build --release`
   - `meson.build` → `meson setup` / `meson compile` / `meson install`
   - `CMakeLists.txt` → `cmake` configure and build
   - `Makefile` → `make`

## Philosophy

Why yet another package manager? Because we do something insane: **trust maintainers**.

If a maintainer tags a release and uploads a binary, we install it. If they tag a release with source only, we build it. No recipes, no formulas, no maintainer-separate-from-upstream. The maintainer *is* the packager.

This won't work for everything. Complex projects with system dependencies (GTK apps, things that need specific libraries) will fail at build time — and that's an honest failure, not a packaging abstraction hiding the problem. But graft will tell you exactly what's missing and how to install it.

## System dependencies

When a source build fails because of missing system libraries, graft parses the build output and tells you what you need:

```
[error] Build failed due to missing system libraries:
  - libadwaita-1

[hint] Install them with:
  emerge --ask gui-libs/libadwaita

[hint] Re-run with --install-deps to have graft install them for you.
```

graft auto-detects your system package manager (portage, apt, dnf, pacman, zypper, apk) and resolves pkg-config names to real package names.

Pass `--install-deps` to let graft install the missing libraries and retry the build automatically:

```bash
graft --install-deps geigi/cozy
```

## Platform support

- Linux (x86_64, aarch64)
- macOS (x86_64, aarch64)
- Windows (x86_64) — binary installs only, source builds are Linux/macOS for now

## State

graft stores everything under `~/.graft/`:

- `bin/` — installed binaries
- `manifests/` — JSON manifests tracking what's installed, from where, and which version

## License

MIT
