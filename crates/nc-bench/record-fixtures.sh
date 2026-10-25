#!/bin/sh
# Records the recognition fixtures by drawing them in the real app through a virtual pointer, one
# fresh app (and database) per page, then extracting what it saved. Hyprland only: draws on
# workspace 9 and returns to the current workspace. Run from the repository root after
# `cargo build --release -p neuralcanvas -p nc-bench`.
# Usage: record-fixtures.sh [OUT_DIR] [SEED]  (seed 4: tuning set, seed 5: held-out set)
set -eu
out=${1:-crates/nc-core/tests/fixtures}
seed=${2:-4}
bin=target/release
work=$(mktemp -d)
back=$(hyprctl activeworkspace -j | jq .id)
hyprctl dispatch 'hl.dsp.focus({workspace = 9})' >/dev/null
pages=$("$bin/nc-bench" fixtures pages)
page=0
while [ "$page" -lt "$pages" ]; do
    data="$work/page$page"
    mkdir -p "$data"
    XDG_DATA_HOME="$data" "$bin/neuralcanvas" &
    app=$!
    sleep 1.5
    at=$(hyprctl clients -j | jq -r '.[] | select(.class=="sh.nodaysidle.NeuralCanvas") | "\(.at[0]) \(.at[1])"')
    # shellcheck disable=SC2086 # $at is "x y" on purpose
    "$bin/nc-bench" fixtures script "$seed" "$page" $at "$data/manifest.json" | "$bin/nc-bench" drive script
    kill -TERM "$app"
    wait "$app"
    "$bin/nc-bench" fixtures extract "$data/manifest.json" "$data/neuralcanvas/neuralcanvas.db" "$out"
    page=$((page + 1))
done
hyprctl dispatch "hl.dsp.focus({workspace = $back})" >/dev/null
rm -rf "$work"
