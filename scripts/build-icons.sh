#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

source='assets/app-icon.png'
iconset='outputs/app-icon.iconset'
if [[ ! -f "$source" ]]; then
    printf 'Missing app icon source: %s\n' "$source" >&2
    exit 1
fi
for tool in sips iconutil; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        printf 'App icon generation requires macOS tool: %s\n' "$tool" >&2
        exit 1
    fi
done

metadata=$(sips -g pixelWidth -g pixelHeight -g format "$source")
width=$(awk '/pixelWidth:/ { print $2 }' <<< "$metadata")
height=$(awk '/pixelHeight:/ { print $2 }' <<< "$metadata")
format=$(awk '/format:/ { print $2 }' <<< "$metadata")
if [[ "$format" != 'png' || ! "$width" =~ ^[1-9][0-9]*$ || "$width" != "$height" ]]; then
    printf 'App icon source must be a square PNG: %s (format=%s, %sx%s)\n' \
        "$source" "$format" "$width" "$height" >&2
    exit 1
fi

mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$source" --out "$iconset/icon_${size}x${size}.png" >/dev/null
    doubled=$((size * 2))
    sips -z "$doubled" "$doubled" "$source" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil --convert icns --output outputs/AppIcon.icns "$iconset"
printf '%s\n' "$PWD/outputs/AppIcon.icns"
