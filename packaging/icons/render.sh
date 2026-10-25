#!/bin/sh
# Renders the hicolor PNG icons from the SVG source (needs rsvg-convert, from librsvg).
set -eu
cd "$(dirname "$0")"
for n in 16 32 48 64 128 256 512; do
    mkdir -p "hicolor/${n}x${n}/apps"
    rsvg-convert -w "$n" -h "$n" sh.nodaysidle.NeuralCanvas.svg -o "hicolor/${n}x${n}/apps/sh.nodaysidle.NeuralCanvas.png"
done
