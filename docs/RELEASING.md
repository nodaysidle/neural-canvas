# Releasing

**Every release artefact ships with a `.sha256` next to it.** No exceptions: AppImage, binary tarball,
source tarball.

## Artefacts (per tag `vX.Y.Z`)

| File | Checksum |
|---|---|
| `NeuralCanvas-X.Y.Z-x86_64.AppImage` | `NeuralCanvas-X.Y.Z-x86_64.AppImage.sha256` |
| `neuralcanvas-X.Y.Z-x86_64.tar.zst` (binary + desktop file + icons) | `neuralcanvas-X.Y.Z-x86_64.tar.zst.sha256` |
| `neuralcanvas-X.Y.Z.tar.gz` (source) | `neuralcanvas-X.Y.Z.tar.gz.sha256` |

Generate from inside `dist/` so the file names in the checksum are bare:

```sh
cd dist && for f in *.AppImage *.tar.zst *.tar.gz; do sha256sum "$f" > "$f.sha256"; done
sha256sum -c *.sha256   # must print OK for every file before upload
```

Users verify with `sha256sum -c NeuralCanvas-X.Y.Z-x86_64.AppImage.sha256`.
Optional, recommended: a minisign `.minisig` per artefact with the public key in the README.

## Checklist

1. `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` green.
2. Perf run on the reference machine meets `docs/PERFORMANCE-BUDGET.md`; numbers attached to the release PR.
3. Metadata valid: `desktop-file-validate packaging/sh.nodaysidle.NeuralCanvas.desktop` and
   `appstreamcli validate packaging/sh.nodaysidle.NeuralCanvas.metainfo.xml`.
4. `packaging/release.sh` (needs Docker): builds the three artefacts into `dist/` from `HEAD` (the binary
   in an ubuntu:24.04 container, glibc 2.39), writes each `.sha256`, runs `sha256sum -c`, fills the
   PKGBUILDs' checksums and writes AUR-ready copies with `.SRCINFO` to `dist/aur/`. Commit the
   updated `packaging/arch/` checksums (they are left out of the source tarball, so it stays the same).
5. `packaging/check-arch.sh`: makepkg and namcap for both packages, then each installed with pacman
   into a clean archlinux container and started against the host's Wayland session.
6. Scripted flow on clean Arch + one non-Arch system: `packaging/smoke.sh <binary or AppImage> <dir>`
   (fresh data folder: new project, draw, style from a screenshot, export, close, relaunch, store
   unchanged). Run the AppImage on Arch and on Ubuntu or Fedora.
7. Draft GitHub Release with artefacts **and** their `.sha256` files. A human publishes.
8. AUR `pkgver` / `sha256sums` bumped from the published artefacts (`dist/aur/`). Pushing to the AUR
   is outward-facing: ask Alan first.

## AppImage contents

The binary, desktop file, metainfo, icons and licences, plus the Vulkan loader and xkbcommon as a
fallback that `AppRun` uses only when the host has neither copy. libwayland is never bundled: the
host's Vulkan driver is built against the host's libwayland and fails to load with an older one
(measured in Phase 7: Ubuntu's 1.22 lacks `wl_display_create_queue_with_name`, which Mesa on Arch
needs). The GPU driver and glibc always come from the host.
