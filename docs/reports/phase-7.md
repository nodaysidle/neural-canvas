# Phase 7 report: packaging

Built and checked 2026-10-25 on the reference machine (Arch, Hyprland 0.56, Intel Arc). Everything
is local: no remote, no push, no AUR upload, no tag, no release.

## What shipped (`packaging/`)

- `sh.nodaysidle.NeuralCanvas.desktop` (Categories `Graphics;VectorGraphics;2DGraphics;`) and
  `sh.nodaysidle.NeuralCanvas.metainfo.xml`.
- Icon: `icons/sh.nodaysidle.NeuralCanvas.svg` (a hand-drawn box turning into a wireframe card, the
  DESIGN.md accent on the dark tile) and hicolor PNGs at 16, 32, 48, 64, 128, 256 and 512 px
  (`icons/render.sh`). Preview: `phase-7/icon-256.png`.
- `arch/PKGBUILD` (source) and `arch/PKGBUILD-bin` (repackages the release binary tarball).
- `appimage/build.sh` and `appimage/AppRun`: the AppImage recipe, run in an ubuntu:24.04 container.
- `release.sh`: builds the three artefacts from `HEAD`, a `.sha256` for each, `sha256sum -c`, fills
  the PKGBUILDs' checksums and writes AUR-ready copies with `.SRCINFO` to `dist/aur/`.
- `check-arch.sh`, `check-appimage.sh`, `smoke.sh`: the checks below, rerunnable.
- `LICENSE` (MIT, as `Cargo.toml` already declared; there was no licence file). The OFL texts of the
  bundled fonts ship next to it in every package.
- `docs/RELEASING.md` names these scripts in the checklist; `docs/PLAN.md` has the decisions.

## Artefacts

Built from commit `e98707f` (the v0.1.0 release build; tagged at the commit that adds
`CHANGELOG.md`, which changes no artefact). The binary is compiled once, in ubuntu:24.04 with Rust 1.98.1, and shared
by the binary tarball, `neuralcanvas-bin` and the AppImage. It needs glibc 2.39 or newer (Ubuntu
24.04+, Fedora 40+, Debian 13, Arch).

| Artefact | Size | SHA-256 |
|---|---|---|
| `neuralcanvas-0.1.0.tar.gz` (source) | 0.95 MB | `29f21d68ef6cf844db3ba68ee63f6096ad87ae1f6c0ab63373cf17665702830a` |
| `neuralcanvas-0.1.0-x86_64.tar.zst` (binary) | 6.97 MB | `fbdb856cd370c7a98183c6918dd2a7cc3c4090f5d8365f763be4438009c85660` |
| `NeuralCanvas-0.1.0-x86_64.AppImage` | 9.67 MB | `80fd1df052b91f46692bbba8b78619a14f2195dd5558940c882caf18f49d2247` |

Budgets (PERFORMANCE-BUDGET.md): binary 21.50 MB stripped of 25 MB; AppImage 9.67 MB of 40 MB. The
packages makepkg built from these: `neuralcanvas` 8.74 MB, `neuralcanvas-bin` 8.88 MB.

`sha256sum -c` prints OK for all three (`phase-7/checksums.txt`). Two `release.sh` runs on a clean
tree gave the same three files byte for byte. A commit that only changes the PKGBUILD checksums or the
reports doesn't change them either: the source tarball is archived from the tree (an archived commit
carries its id) with its file times pinned to the last commit that changed what ships, and the binary
tarball and AppImage use the same time. So the checksums committed in the PKGBUILDs hold at `HEAD`.
Both builds reused the warm Cargo cache; a build from a cold cache was not compared.

## Validation

| Check | Result | Evidence |
|---|---|---|
| `desktop-file-validate` | No issues | `phase-7/metadata-validation.txt` |
| `appstreamcli validate --pedantic` (AppStream 1.2.1) | Passes; one pedantic note: the app id has capitals (`sh.nodaysidle.NeuralCanvas`, fixed by PLAN §8) | same |
| namcap, both PKGBUILDs | Clean | `phase-7/namcap-*.txt` |
| namcap, both packages | **0 errors.** Warnings: `wayland`, `libxkbcommon`, `vulkan-icd-loader` "may not be needed" (the binary loads them at run time, which namcap can't see; without them it doesn't start), and the usual unused `ld-linux` note on Rust binaries | same |
| makepkg in a clean archlinux container | Both build; the source package's `check()` ran 132 tests, 0 failures | `phase-7/check-arch.txt` |
| Clean install (`pacman -U` in a fresh archlinux container) and run | Both install with their dependencies and run against the host's Wayland session until stopped | same |
| AppImage on Arch (host) | Runs, using the host's Vulkan loader and libwayland | below |
| AppImage on Ubuntu 24.04 and Fedora 41 (containers, host Wayland) | Window opens on both, rendering through the host's Mesa driver | `phase-7/check-appimage.txt` |
| Scripted smoke test, release binary and AppImage | **PASS** both | `phase-7/smoke-*.txt`, `smoke-0*.png` |

The containers stand in for VMs: each starts clean, so "installs on a clean system" is tested, but on
the host's kernel and GPU.

## The smoke test (`packaging/smoke.sh`)

On empty data and config folders: open a new project, draw a card with a field and a button, switch
to the wireframe, paste a screenshot (`Ctrl+V`), apply the style it reads and save it as a preset,
export the sketch and the wireframe, close the window. The store must then hold 2 projects, 3
elements, 1 styled sketch and 1 preset, and 8 export files must exist. Relaunch, close again, and the
store's digest (every element row, sketch style, preset and project) must match the first one.

| Run | Store after first session | Digest before and after relaunch |
|---|---|---|
| Release binary | 2 projects, 3 elements, 1 styled sketch, 1 preset; 8 exports | `8da80249…` = `8da80249…` |
| AppImage | same | `8ce8a3c5…` = `8ce8a3c5…` |

The same flow also passed on each earlier build this phase; the two runs above are on the final
artefacts.

## Decisions and departures from the brief

- **libwayland is not bundled in the AppImage.** The brief asked for Wayland libraries inside. I built
  that first, and on Arch it fails before drawing anything:
  `libvulkan_intel.so: undefined symbol: wl_display_create_queue_with_name`, then
  `vkCreateInstance: Found no drivers!`. A Vulkan driver is built against its own system's
  libwayland; Ubuntu's 1.22 is older than the one Mesa on Arch needs. Every system with a Vulkan
  driver has its libwayland, so the AppImage uses the host's. The Vulkan loader and xkbcommon are
  bundled as a fallback that `AppRun` uses only when the host has no copy (the Ubuntu and Fedora
  containers took this path; Arch uses its own).
- **One binary for all artefacts,** built on Ubuntu 24.04 for glibc reach (PLAN §8).
- **Source PKGBUILD:** `options=('!lto' '!debug')`. makepkg's `-flto=auto` turns the bundled SQLite C
  objects into GCC LTO bitcode the Rust link can't read (the first build failed on `undefined symbol:
  sqlite3_column_type` and the rest); Cargo's profile does its own LTO and strips, so a debug package
  would only hold a broken link (namcap flagged it).
- **Dependencies** `hicolor-icon-theme` (namcap) and `libgcc` (Arch split it out of `gcc-libs`) added.
- **Homepage** in the metainfo and PKGBUILDs: `https://github.com/nodaysidle/neural-canvas`, the
  repository the owner created for the release (settles PLAN §9 q2).
- **Three fixes after the first release build:** `smoke.sh` left `wl-copy` holding its output pipe
  (a piped run never ended); the source tarball embedded the commit id, and archived from a tree it
  took the current time instead, so its checksum changed with every commit and every build. Fixed by
  archiving the tree with `--mtime` pinned. An earlier "identical rebuild" result of mine was wrong:
  the rebuild had refused to run on a dirty tree and I compared the old checksums with themselves.
  Every check above was rerun on the build from the final commit.
- **Tools pinned by checksum:** appimagetool 1.9.1 and type2-runtime 20251108, verified by SHA-256 in
  `build.sh`. The pins are from my first download, not from an upstream checksum list.
- **Docker** was started for this session at your go-ahead; nothing was installed on the host.

## Not done, and risks

- **Pasting into the smoke test uses your clipboard.** The script saves the clipboard's text and puts
  it back, but Omarchy's clipboard history watcher records the pasted test screenshot. The file
  chooser route can't be scripted reliably: GTK's chooser ignored synthetic clicks and Return in
  about half the runs (a held click helped but didn't cure it). It was checked by hand in Phase 6 and
  again in this phase, and both routes share the same reader.
- **The smoke test is tied to this setup:** Hyprland, a 2560×1440 output (the virtual pointer maps onto
  it), the inspector at its default width. On other machines it needs its coordinates adjusted.
- **X11 was not tried.** The packages list libx11, libxcursor and libxi as optional for X11 sessions.
- **Not tried on real hardware other than this machine:** the Ubuntu and Fedora runs use this
  machine's Intel GPU through containers.
- **AUR-ready files exist only in `dist/aur/`** (git-ignored); the committed PKGBUILDs carry the same
  checksums. Publishing them is your call.
- **Release CI** (RELEASING.md: CI drafts the GitHub Release) is not set up: there is no remote.

## How to rerun

```sh
packaging/release.sh                     # artefacts, checksums, PKGBUILD sums, dist/aur/
packaging/check-arch.sh                  # makepkg, namcap, clean install and run
packaging/check-appimage.sh              # Ubuntu 24.04 and Fedora 41
cargo build --release -p nc-bench
packaging/smoke.sh dist/NeuralCanvas-0.1.0-x86_64.AppImage /tmp/smoke
```
