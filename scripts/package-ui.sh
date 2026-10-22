#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
bash scripts/build-icons.sh
cargo build --release -p dbide-ui-spike
app='outputs/Rust Database IDE.app'
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/dbide-ui-spike "$app/Contents/MacOS/dbide-ui-spike"
cp outputs/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>Rust Database IDE</string>
<key>CFBundleDisplayName</key><string>Rust Database IDE</string>
<key>CFBundleIdentifier</key><string>com.local.rust-database-ide</string>
<key>CFBundleExecutable</key><string>dbide-ui-spike</string>
<key>CFBundleIconFile</key><string>AppIcon.icns</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>CFBundleVersion</key><string>1</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
codesign --force --sign - "$app"
touch "$app"
printf '%s\n' "$PWD/$app"
