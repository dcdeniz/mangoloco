#!/bin/sh
# Builds dist/Mango Loco.app (release) with the app icon and the `mango` CLI.
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/dist/Mango Loco.app"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/crates/app/Cargo.toml" | head -1)"

(cd "$ROOT/crates/app" && cargo build --release --bin mangoloco --bin mango)

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/bin"
cp "$ROOT/crates/app/target/release/mangoloco" "$APP/Contents/MacOS/mangoloco"
cp "$ROOT/crates/app/target/release/mango" "$APP/Contents/Resources/bin/mango"
cp "$ROOT/assets/brand/MangoLoco.icns" "$APP/Contents/Resources/AppIcon.icns"

# Bundle ffmpeg/ffprobe only when MANGO_BUNDLE_FFMPEG points at a folder of
# self-contained builds; Homebrew's are linked against Homebrew libraries and
# would not run on a Mac without Homebrew.
if [ -n "${MANGO_BUNDLE_FFMPEG:-}" ]; then
  cp "$MANGO_BUNDLE_FFMPEG/ffmpeg" "$MANGO_BUNDLE_FFMPEG/ffprobe" "$APP/Contents/Resources/bin/"
fi

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Mango Loco</string>
  <key>CFBundleDisplayName</key><string>Mango Loco</string>
  <key>CFBundleIdentifier</key><string>is.neat.mangoloco</string>
  <key>CFBundleExecutable</key><string>mangoloco</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSPrincipalClass</key><string>NSApplication</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.video</string>
  <key>NSMicrophoneUsageDescription</key><string>Mango Loco records your microphone when you start a recording.</string>
  <key>NSCameraUsageDescription</key><string>Mango Loco records your camera when you start a recording.</string>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>Mango Loco Project</string>
      <key>CFBundleTypeExtensions</key><array><string>mango</string></array>
      <key>CFBundleTypeRole</key><string>Editor</string>
    </dict>
  </array>
</dict>
</plist>
PLIST

codesign --force --deep -s - "$APP"
echo "Built $APP"
du -sh "$APP"
