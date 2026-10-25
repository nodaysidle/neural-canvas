#!/bin/bash
# Runs the AppImage on other distributions (docs/RELEASING.md step 6): in ubuntu:24.04 and fedora:41
# containers with only what any desktop has added (a Mesa Vulkan driver, the XKB keyboard data), against the host's Wayland session (Hyprland:
# the window is looked up by its app id). Records whether a window appeared and which libraries the
# process loaded. Run packaging/release.sh first. Logs go to dist/check-appimage/.
set -euo pipefail

root=$(git rev-parse --show-toplevel)
dist=$root/dist
out=$dist/check-appimage
ver=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n1)
appimage=NeuralCanvas-$ver-x86_64.AppImage
rm -rf "$out"
mkdir -p "$out"

declare -A setup=(
    [ubuntu:24.04]="apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends mesa-vulkan-drivers xkb-data procps >/dev/null"
    [fedora:41]="dnf install -y -q mesa-vulkan-drivers xkeyboard-config procps-ng util-linux >/dev/null"
)
for image in ubuntu:24.04 fedora:41; do
    name=${image//:/-}
    docker pull -q "$image" >/dev/null
    # The app runs until stopped; while it does, look for its window and its loaded libraries.
    docker run --rm --name "nc-check-$name" -v "$dist:/dist:ro" -v "$out:/out" \
        --device /dev/dri \
        -v "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY:/run/wayland/$WAYLAND_DISPLAY" \
        -e WAYLAND_DISPLAY="$WAYLAND_DISPLAY" -e XDG_RUNTIME_DIR=/run/wayland \
        -e APPIMAGE_EXTRACT_AND_RUN=1 \
        "$image" bash -euc "
        ${setup[$image]}
        user=\$(id -nu $(id -u) 2>/dev/null || { useradd -m -u $(id -u) user && echo user; })
        chown \$user /run/wayland
        cd /tmp && runuser -u \$user -- /dist/$appimage > /out/run-$name.log 2>&1 &
        for i in \$(seq 60); do
            sleep 0.5
            pid=\$(pgrep -x neuralcanvas || true)
            [ -n \"\$pid\" ] && sleep 4 && break
        done
        if [ -n \"\$pid\" ]; then
            # As the app's own user: root in a container may not read another user's maps.
            runuser -u \$user -- cat /proc/\$pid/maps \
                | grep -oE '/[^ ]*lib(vulkan|xkbcommon|wayland-client)[^ ]*\\.so[^ ]*' | sort -u > /out/libs-$name.txt
            kill -TERM \$pid
            sleep 2
        fi
        grep -E '^(PRETTY_NAME)=' /etc/os-release > /out/os-$name.txt
        chown -R $(id -u):$(id -g) /out
    " > "$out/container-$name.log" 2>&1 &
    container=$!
    window=no
    while kill -0 "$container" 2>/dev/null; do
        sleep 0.5
        if hyprctl clients -j | grep -q '"class": "sh.nodaysidle.NeuralCanvas"'; then
            window=yes
            break
        fi
    done
    wait "$container" || true
    echo "window appeared: $window" >> "$out/run-$name.log"
    echo "== $image ($(cat "$out/os-$name.txt" 2>/dev/null))"
    cat "$out/run-$name.log"
    echo "libraries:"; cat "$out/libs-$name.txt" 2>/dev/null || echo "(none: the app did not start)"
done
