#!/bin/bash
# Builds every release artefact from the committed tree into dist/ and writes a .sha256 next to each
# (docs/RELEASING.md):
#   neuralcanvas-VERSION.tar.gz               source (git archive of HEAD)
#   neuralcanvas-VERSION-x86_64.tar.zst       binary + desktop file + metainfo + icons + licences
#   NeuralCanvas-VERSION-x86_64.AppImage      the same binary, Vulkan loader and xkbcommon as fallbacks
# The binary is built in an ubuntu:24.04 container (Docker) for glibc reach. The PKGBUILDs' checksums
# are filled in from the artefacts, and AUR-ready copies (with .SRCINFO) go to dist/aur/.
# Local only: nothing here pushes, tags or uploads.
set -euo pipefail

root=$(git rev-parse --show-toplevel)
cd "$root"
ver=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
rust=$(rustc -V | cut -d' ' -f2)
# What ships: everything but the PKGBUILDs (they carry the artefacts' checksums) and the reports.
shipped=(. ':!packaging/arch' ':!docs/reports')
# File times in the archives: the last commit that changed what ships, so a commit that only updates
# the PKGBUILD checksums or the reports leaves every artefact byte for byte the same.
mtime=$(git log -1 --format=%ct -- "${shipped[@]}")
dist=$root/dist

# NC_RELEASE_TREE=index builds from what is staged instead, for trying the packaging before committing.
if [ "${NC_RELEASE_TREE:-HEAD}" = index ]; then
    tree=$(git write-tree)
elif [ -n "$(git status --porcelain --untracked-files=no -- "${shipped[@]}")" ]; then
    echo "release.sh: commit your changes first; artefacts are built from HEAD" >&2
    exit 1
else
    tree=HEAD
fi

rm -rf "$dist"
mkdir -p "$dist"

echo "== source"
# The tree, not the commit (an archived commit carries its id in the tar header), with the file times
# given (a tree is otherwise stamped with the current time).
git archive --format=tar --mtime="$mtime +0000" --prefix="neuralcanvas-$ver/" "$tree^{tree}" | gzip -n -9 > "$dist/neuralcanvas-$ver.tar.gz"

echo "== binary tarball and AppImage (ubuntu:24.04, Rust $rust)"
docker run --rm \
    -v "$dist:/dist" \
    -v "$root/packaging/appimage/build.sh:/build.sh:ro" \
    -v nc-release-cargo:/cargo \
    -v nc-release-target:/target \
    ubuntu:24.04 bash /build.sh "$ver" "$rust" "$(id -u):$(id -g)" "$mtime"

echo "== checksums"
(
    cd "$dist"
    for f in *.AppImage *.tar.zst *.tar.gz; do
        sha256sum "$f" > "$f.sha256"
    done
    sha256sum -c ./*.sha256
)

echo "== PKGBUILDs"
sum() { cut -d' ' -f1 "$dist/$1.sha256"; }
sed -i "s/^sha256sums=.*/sha256sums=('$(sum "neuralcanvas-$ver.tar.gz")')/" packaging/arch/PKGBUILD
sed -i "s/^sha256sums_x86_64=.*/sha256sums_x86_64=('$(sum "neuralcanvas-$ver-x86_64.tar.zst")')/" packaging/arch/PKGBUILD-bin
for pkg in neuralcanvas neuralcanvas-bin; do
    mkdir -p "$dist/aur/$pkg"
done
cp packaging/arch/PKGBUILD "$dist/aur/neuralcanvas/PKGBUILD"
cp packaging/arch/PKGBUILD-bin "$dist/aur/neuralcanvas-bin/PKGBUILD"
for pkg in neuralcanvas neuralcanvas-bin; do
    (cd "$dist/aur/$pkg" && makepkg --printsrcinfo > .SRCINFO)
done

ls -l "$dist"
