#!/bin/bash
# Release smoke test (docs/RELEASING.md step 4), driven like a person would: fresh data and config
# folders -> new project -> draw a card with a field and a button -> wireframe -> paste a screenshot
# to read its style, apply it, save it as a preset -> export sketch and
# wireframe -> close the window -> relaunch -> close again. Passes when the store holds the same
# elements, style and preset after the relaunch as before it, and the eight export files exist.
#
# Needs Hyprland on a 2560x1440 output (the virtual pointer maps onto it), no other output, the
# release nc-bench (`cargo build --release -p nc-bench`), sqlite3, grim, wl-clipboard. Usage:
#   packaging/smoke.sh <neuralcanvas binary or AppImage> <output folder>
set -euo pipefail

root=$(git rev-parse --show-toplevel)
app=$(readlink -f "$1")
out=$(mkdir -p "$2" && cd "$2" && pwd)
drive=$root/target/release/nc-bench
# A screenshot whose style reads with no warnings or notes, so Apply sits where the script clicks.
screenshot=$root/crates/nc-core/tests/style-fixtures/neuralcanvas.png
home=$(mktemp -d)
data=$home/data config=$home/config pictures=$home/pictures
mkdir -p "$data" "$config" "$pictures"
echo "XDG_PICTURES_DIR=\"$pictures\"" > "$config/user-dirs.dirs"
db=$data/neuralcanvas/neuralcanvas.db
step() { echo "== $*"; }
fail() { echo "FAIL: $*" >&2; exit 1; }
drive() { "$drive" drive script; }

launch() {
    XDG_DATA_HOME=$data XDG_CONFIG_HOME=$config "$app" >> "$out/app.log" 2>&1 &
    for _ in $(seq 50); do
        sleep 0.2
        read -r wx wy ww wh < <(hyprctl clients -j | python3 -c "
import json, sys
for c in json.load(sys.stdin):
    if c['class'] == 'sh.nodaysidle.NeuralCanvas':
        print(*c['at'], *c['size'])") && [ -n "${wh:-}" ] && break
    done
    [ -n "${wh:-}" ] || fail "no window"
    sleep 1.5
}
shot() { grim -g "$wx,$wy ${ww}x$wh" "$out/$1.png"; }
# A press held 100 ms: GTK's file chooser ignores a press and release in the same instant.
click() { printf 'move %d %d 150\nwait 100\ndown\nwait 100\nup\nwait %d\n' "$((wx + $1))" "$((wy + $2))" "${3:-400}" | drive; }
close() {
    hyprctl dispatch 'hl.dsp.window.close()' >/dev/null
    for _ in $(seq 50); do pgrep -x neuralcanvas >/dev/null || return 0; sleep 0.2; done
    fail "the app did not quit"
}
# Everything the store holds that a person made: elements, the sketch's style, presets, projects.
digest() {
    sqlite3 "$db" "SELECT hex(sketch_id), z, hex(data) FROM elements ORDER BY sketch_id, z;
                   SELECT hex(id), name, hex(style) FROM sketches ORDER BY created_at;
                   SELECT name, hex(style) FROM style_presets ORDER BY created_at;
                   SELECT name FROM projects ORDER BY created_at;" | sha256sum | cut -d' ' -f1
}

step "launch on a fresh data folder"
launch
shot 01-first-launch

step "new project"
printf 'key -M ctrl b -m ctrl\nwait 400\n' | drive
click 216 66 1200
printf 'key -M ctrl b -m ctrl\nwait 300\n' | drive

step "draw a card, a field and a button"
rect() { printf 'move %d %d 80\ndown\nmove %d %d 300\nmove %d %d 200\nmove %d %d 300\nmove %d %d 200\nup\nwait 150\n' \
    "$((wx + $1))" "$((wy + $2))" "$((wx + $3))" "$((wy + $2))" "$((wx + $3))" "$((wy + $4))" \
    "$((wx + $1))" "$((wy + $4))" "$((wx + $1))" "$((wy + $2 + 2))"; }
{
    printf 'move %d %d 100\nkey p\n' "$((wx + 1200))" "$((wy + 700))"
    rect 787 362 1387 862
    rect 837 602 1337 662
    rect 837 742 1037 802
    printf 'key w\nwait 300\nkey w\nwait 800\n' # sketch -> hybrid -> wireframe
} | drive
shot 02-drawn-wireframe

step "style mirror: paste a screenshot, apply, save as preset"
# Ctrl+V with an image on the clipboard reads its style. The clipboard's text is put back afterwards
# (a clipboard history, if one runs, keeps the screenshot).
# (The file chooser route is the same reader; GTK's chooser ignores synthetic input too often to script.)
saved=$home/clipboard.txt
wl-paste --no-newline --type text/plain > "$saved" 2>/dev/null || rm -f "$saved"
# wl-copy stays behind to serve the clipboard: keep it off this script's output, or a pipe never ends.
wl-copy --type image/png < "$screenshot" >/dev/null 2>&1
printf 'key -M ctrl i -m ctrl\nwait 400\n' | drive
printf 'key -M ctrl v -m ctrl\nwait 3000\n' | drive
if [ -f "$saved" ]; then wl-copy < "$saved" >/dev/null 2>&1; else wl-copy --clear; fi
shot 03-style-read
click $((ww - 236)) 320 800 # Apply
click $((ww - 210)) 320 800 # Save as preset
shot 04-style-applied

step "export sketch and wireframe"
printf 'key -M ctrl e -m ctrl\nwait 2500\nkey -M ctrl -M shift e -m shift -m ctrl\nwait 3000\n' | drive
shot 05-exported

step "close, then check the store"
close
before=$(digest)
counts=$(sqlite3 "$db" "SELECT (SELECT count(*) FROM projects), (SELECT count(*) FROM elements),
                               (SELECT count(*) FROM sketches WHERE style IS NOT NULL),
                               (SELECT count(*) FROM style_presets);")
echo "projects|elements|styled sketches|presets: $counts"
IFS='|' read -r projects elements styled presets <<< "$counts"
[ "$projects" -eq 2 ] || fail "expected 2 projects, found $projects"
[ "$elements" -eq 3 ] || fail "expected 3 elements, found $elements"
[ "$styled" -eq 1 ] || fail "expected 1 styled sketch, found $styled"
[ "$presets" -eq 1 ] || fail "expected 1 preset, found $presets"
exports=$(find "$pictures/NeuralCanvas" -type f | sort)
echo "$exports" > "$out/exports.txt"
[ "$(echo "$exports" | grep -c .)" -eq 8 ] || fail "expected 8 export files: $exports"

step "relaunch"
launch
printf 'key w\nwait 300\nkey w\nwait 800\n' | drive
shot 06-after-relaunch
close
after=$(digest)
echo "store digest before relaunch: $before"
echo "store digest after relaunch:  $after"
[ "$before" = "$after" ] || fail "the store changed across the relaunch"
cp "$db" "$out/neuralcanvas.db"
rm -rf "$home"
echo "PASS"
