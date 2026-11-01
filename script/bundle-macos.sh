#!/bin/sh
# Builds target/release/PhpCraft.app (and a zip of it) from a release build.
set -e
cd "$(dirname "$0")/.."
cargo build --release -p phpcraft
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
APP=target/release/PhpCraft.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/phpcraft "$APP/Contents/MacOS/phpcraft"

# Icon: SVG -> 1024px PNG (Quick Look) -> iconset -> icns.
ICONSET=target/release/PhpCraft.iconset
rm -rf "$ICONSET" && mkdir -p "$ICONSET"
qlmanage -t -s 1024 -o target/release assets/icon/phpcraft.svg >/dev/null 2>&1
for size in 16 32 64 128 256 512; do
  sips -z $size $size target/release/phpcraft.svg.png --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z $double $double target/release/phpcraft.svg.png --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/PhpCraft.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>PhpCraft</string>
  <key>CFBundleDisplayName</key><string>PhpCraft</string>
  <key>CFBundleIdentifier</key><string>dev.phpcraft.PhpCraft</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleExecutable</key><string>phpcraft</string>
  <key>CFBundleIconFile</key><string>PhpCraft</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
</dict>
</plist>
PLIST

# Ad-hoc signature so Gatekeeper lets the local build run.
codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || true
(cd target/release && rm -f PhpCraft-macos.zip && ditto -c -k --keepParent PhpCraft.app PhpCraft-macos.zip)
echo "Built $APP"

# Disk image: the app plus a link to /Applications for drag-and-drop install.
DMG_STAGE=target/release/dmg
rm -rf "$DMG_STAGE" && mkdir -p "$DMG_STAGE"
cp -R "$APP" "$DMG_STAGE/"
ln -s /Applications "$DMG_STAGE/Applications"
rm -f "target/release/PhpCraft-$VERSION.dmg"
hdiutil create -volname "PhpCraft" -srcfolder "$DMG_STAGE" -ov -format UDZO "target/release/PhpCraft-$VERSION.dmg" >/dev/null
echo "Built target/release/PhpCraft-$VERSION.dmg"
