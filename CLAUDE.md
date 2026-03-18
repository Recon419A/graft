# graft — Development Guide

## What is this?

A package manager that trusts maintainers. `graft org/repo` installs software directly from GitHub releases or by building from source. The crate is published as `rectifier` on crates.io (binary name is `graft`).

## Build & Run

```bash
cargo build              # dev build
cargo build --release    # release build
cargo run -- org/repo    # run directly
```

## Project structure

- `src/main.rs` — CLI entry point (clap), install/list/remove commands
- `src/github.rs` — GitHub API: latest release (with tag fallback), asset selection, download
- `src/platform.rs` — OS/arch detection and pattern matching for asset names
- `src/install.rs` — Binary installation from archives, manifest management, uninstall
- `src/build.rs` — Source build pipeline: download tarball, extract, detect build system, build, parse errors for missing deps
- `src/system.rs` — Distro/package manager detection, pkg-config → system package resolution

## Install flow

1. Query GitHub releases API → fall back to tags API if no releases
2. Try to match a release asset to the current platform (OS + arch)
3. If a matching asset is found → download and extract binary
4. If no asset matches → download source tarball → detect build system → build → install binary
5. If build fails with missing deps → parse errors, resolve to system packages, show hint (or auto-install with `--install-deps`)
6. Save a manifest to `~/.graft/manifests/`

## Key design decisions

- **No recipes or formulas.** The GitHub release *is* the package.
- **Auto-detect build systems** rather than requiring config from maintainers.
- **Blocking HTTP** via `reqwest::blocking` — no async runtime needed for a CLI tool.
- **`~/.graft/`** is the sole state directory. Binaries go in `bin/`, manifests in `manifests/`.

## Testing

Currently tested by dogfooding: `cargo run -- sharkdp/bat` (pre-built path) and `cargo run -- geigi/cozy` (source build path, requires system deps).

## Known limitations

- System dep resolution only catches the *first* missing dep (meson/cmake bail on the first failure). The `--install-deps` retry loop handles this iteratively.
- Gentoo provider search scans the portage tree on disk — works for uninstalled packages but is a directory walk.
- Windows source builds not yet supported.
- No version pinning or lockfile yet.
