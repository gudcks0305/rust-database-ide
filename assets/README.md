# App icon

`app-icon.png` is the source artwork for the macOS application icon. It combines
a copper database stack and an ivory query chevron on a midnight-blue tile.
The exterior is transparent. Created for this project with ImageGen on 2026-10-23.

Run `bash scripts/build-icons.sh` on macOS to produce the standard 16–1024 pixel
iconset and `outputs/AppIcon.icns` using `sips` and `iconutil`. The packaging script
does this automatically, copies the ICNS into the application's Resources, and
declares `CFBundleIconFile` before signing. Generated sizes are not source assets.

Keep the source square and preserve transparency. Check the 32/64 pixel sizes and
the packaged application's Finder icon when replacing it. A running application
may retain its previous Dock icon until it is relaunched.
