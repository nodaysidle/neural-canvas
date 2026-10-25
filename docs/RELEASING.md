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
3. Build artefacts, write `.sha256` files, `sha256sum -c` passes.
4. Scripted flow on clean Arch + one non-Arch VM: create project → draw → erase → switch sketch → quit →
   relaunch → nothing lost.
5. Draft GitHub Release with artefacts **and** their `.sha256` files. A human publishes.
6. AUR `pkgver` / `sha256sums` bumped from the published artefacts. Pushing to the AUR is outward-facing:
   ask Alan first.
