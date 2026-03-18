# graft

**Trust maintainers.** Install software directly from GitHub.

```
graft sharkdp/bat
```

graft finds the latest release, picks the right binary for your platform, and installs it. No formulas. No recipes. No middleman between you and the maintainer.

If there's no pre-built binary, graft downloads the source and builds it. If the build needs system libraries, graft tells you exactly which ones — or installs them for you.

## Install

```
cargo install rectifier
```

Then add graft's bin directory to your PATH:

```bash
# Add to your shell profile (.bashrc, .zshrc, etc.)
export PATH="$HOME/.graft/bin:$PATH"
```

> The crate is `rectifier` — current only flows forward, from upstream to you.

## Usage

```bash
graft org/repo                       # install latest release
graft install org/repo               # same thing, explicit
graft list                           # show installed packages
graft remove org/repo                # uninstall
graft --install-deps org/repo        # also install missing system libraries
```

## How it works

graft has two paths, tried in order:

**1. Pre-built binary (fast path)**

Checks the latest GitHub Release for assets matching your OS and architecture. Handles `.tar.gz`, `.zip`, and bare binaries. This is the common case for Rust/Go/C++ tools that ship release artifacts.

**2. Build from source (fallback)**

Downloads the source tarball for the tagged release and auto-detects the build system:

| File | Build system |
|------|-------------|
| `Cargo.toml` | `cargo build --release` |
| `meson.build` | meson setup / compile / install |
| `CMakeLists.txt` | cmake configure and build |
| `Makefile` | make |

For Python projects, graft detects `requirements.txt` and installs dependencies into a shared virtual environment at `~/.graft/python/` (created with `--system-site-packages` so system bindings like GTK/GStreamer remain accessible).

## Philosophy

Why yet another package manager?

Most package managers insert a layer between upstream and the user: a recipe, a formula, a PKGBUILD. Someone other than the maintainer has to write and maintain it, and it lags behind releases, or dies when that person moves on.

graft skips that layer. **The maintainer is the packager.** If they tag a release and upload a binary, we install it. If they tag a release with source only, we build it. The GitHub release *is* the package.

This means:

- **New releases are available immediately.** No waiting for a downstream packager to update a recipe.
- **Abandoned formulas can't block you.** There's nothing to abandon — the source of truth is the repo itself.
- **It won't work for everything.** A complex GTK app with 15 system dependencies will fail at build time. But that's an honest failure, not a packaging abstraction hiding the problem.

## System dependencies

When a source build fails because of missing libraries, graft parses the error and resolves the dependency to your system's package manager:

```
[error] Build failed due to missing system libraries:
  - libadwaita-1

[hint] Install them with:
  sudo emerge --ask gui-libs/libadwaita

[hint] Re-run with --install-deps to have graft install them for you.
```

Supported package managers: portage (Gentoo), apt (Debian/Ubuntu), dnf (Fedora), pacman (Arch), zypper (openSUSE), apk (Alpine).

With `--install-deps`, graft installs the missing libraries and retries the build automatically, looping until everything resolves.

## Python projects

Python projects that build with meson (like [Cozy](https://github.com/geigi/cozy)) get special handling:

- A shared venv at `~/.graft/python/` with `--system-site-packages`
- PyPI dependencies installed via `uv` (or pip) into the venv
- Meson configured with the venv as its prefix and Python
- A wrapper script in `~/.graft/bin/` that runs the app through the venv's interpreter

This means Python apps have access to both their PyPI dependencies and system-level bindings (gi, cairo, GStreamer) without fighting PEP 668 or polluting your system Python.

## Platform support

- Linux (x86_64, aarch64)
- macOS (x86_64, aarch64)
- Windows (x86_64) — pre-built binaries only

## State

Everything lives under `~/.graft/`:

```
~/.graft/
  bin/           installed binaries and wrapper scripts
  manifests/     JSON metadata (what's installed, from where, which version)
  python/        shared Python venv (created on first Python package install)
```

## License

MIT
