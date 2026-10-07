#!/bin/bash
# Builds the game as a Mac app: target/app/Baseball.app.
#
#   scripts/bundle-mac.sh              the app with the art inside it
#   scripts/bundle-mac.sh --no-art     the app alone; it then looks for the art in
#                                      ~/Library/Application Support/<id>/extracted
#   scripts/bundle-mac.sh --universal  for both Apple and Intel Macs; needs
#                                      `rustup target add x86_64-apple-darwin aarch64-apple-darwin`
#
# The art is whatever is in extracted/.

set -euo pipefail
cd "$(dirname "$0")/.."

APP_NAME="Baseball"
APP_ID="io.github.tousifhabib.baseball"   # must match APP_ID in crates/game/src/locate.rs
PROGRAM="baseball"
MINIMUM_MACOS="11.0"

with_art=true
universal=false
for option in "$@"; do
    case "$option" in
        --no-art) with_art=false ;;
        --universal) universal=true ;;
        *) echo "unknown option: $option" >&2; exit 2 ;;
    esac
done

cargo="$(command -v cargo || true)"
[ -n "$cargo" ] || cargo="$HOME/.cargo/bin/cargo"
version="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)"
[ -n "$version" ] || { echo "no version found in Cargo.toml" >&2; exit 1; }

if $with_art && [ ! -f extracted/manifest.json ]; then
    echo "There is no extracted art in extracted/. Make it with bb-extract, or pass --no-art." >&2
    exit 1
fi

echo "Building version $version..."
if $universal; then
    for target in aarch64-apple-darwin x86_64-apple-darwin; do
        "$cargo" build --profile dist -p bb-game --target "$target"
    done
else
    "$cargo" build --profile dist -p bb-game
fi
"$cargo" build --release -p bb-devtools --bin svg-png

app="target/app/$APP_NAME.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

if $universal; then
    lipo -create -output "$app/Contents/MacOS/$PROGRAM" \
        target/aarch64-apple-darwin/dist/bb-game target/x86_64-apple-darwin/dist/bb-game
else
    cp target/dist/bb-game "$app/Contents/MacOS/$PROGRAM"
fi

# The icon, in every size the system asks for.
iconset="target/app/AppIcon.iconset"
rm -rf "$iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    target/release/svg-png assets/icon.svg --width "$size" --out "$iconset/icon_${size}x${size}.png"
    target/release/svg-png assets/icon.svg --width "$((size * 2))" --out "$iconset/icon_${size}x${size}@2x.png"
done
iconutil --convert icns --output "$app/Contents/Resources/AppIcon.icns" "$iconset"
rm -rf "$iconset"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>$APP_NAME</string>
    <key>CFBundleDisplayName</key>
    <string>$APP_NAME</string>
    <key>CFBundleIdentifier</key>
    <string>$APP_ID</string>
    <key>CFBundleExecutable</key>
    <string>$PROGRAM</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleShortVersionString</key>
    <string>$version</string>
    <key>CFBundleVersion</key>
    <string>$version</string>
    <key>LSMinimumSystemVersion</key>
    <string>$MINIMUM_MACOS</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.sports-games</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSPrincipalClass</key>
    <string>NSApplication</string>
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist" > /dev/null

if $with_art; then
    cp -R extracted "$app/Contents/Resources/extracted"
fi

# Signed for this Mac only. Giving the app to anyone else needs a Developer
# ID signature and notarising instead.
codesign --force --deep --sign - "$app"
codesign --verify --deep --strict "$app"

echo "Made $app ($(du -sh "$app" | cut -f1))"
