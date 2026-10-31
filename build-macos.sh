#!/usr/bin/env bash
set -euo pipefail

echo "=========================================================="
echo "    Stremio Enhanced Shell - macOS Build Script"
echo "=========================================================="

# Check for Homebrew
if ! command -v brew &> /dev/null; then
    echo "Error: Homebrew is required to install mpv. Please install it from https://brew.sh"
    exit 1
fi

# Check for mpv
if ! brew list mpv &> /dev/null; then
    echo "Installing mpv via Homebrew..."
    brew install mpv
fi

# Check for Node.js
if ! command -v node &> /dev/null; then
    echo "Installing node via Homebrew..."
    brew install node
fi

# Detect architecture
ARCH=$(uname -m)
echo "Target architecture: $ARCH"

# Export pkg-config and library search paths for Apple Silicon and Intel
if [ "$ARCH" = "arm64" ]; then
    export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:${PKG_CONFIG_PATH:-}"
    export LIBRARY_PATH="/opt/homebrew/lib:${LIBRARY_PATH:-}"
    export DYLD_LIBRARY_PATH="/opt/homebrew/lib:${DYLD_LIBRARY_PATH:-}"
else
    export PKG_CONFIG_PATH="/usr/local/lib/pkgconfig:${PKG_CONFIG_PATH:-}"
    export LIBRARY_PATH="/usr/local/lib:${LIBRARY_PATH:-}"
    export DYLD_LIBRARY_PATH="/usr/local/lib:${DYLD_LIBRARY_PATH:-}"
fi

# 1. Build TypeScript / Enhanced frontend bundle
echo "==> Building Stremio Enhanced frontend bundle..."
cd src/stremio_app/enhanced
if [ -f "package.json" ]; then
    npm install --silent
    node build.js
fi
cd - > /dev/null

# 2. Build Rust release binary
echo "==> Compiling Stremio Enhanced native shell..."
cargo build --release

# 3. Create macOS .app bundle
APP_NAME="Stremio Enhanced.app"
APP_DIR="target/release/$APP_NAME"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
RESOURCES_DIR="$CONTENTS_DIR/Resources"

echo "==> Packaging into $APP_NAME bundle..."
rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"

# Copy binary
cp target/release/stremio-enhanced-ng "$MACOS_DIR/"

# Create Info.plist
cat > "$CONTENTS_DIR/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>stremio-enhanced-ng</string>
    <key>CFBundleIdentifier</key>
    <string>com.stremio.stremio-enhanced</string>
    <key>CFBundleName</key>
    <string>Stremio Enhanced</string>
    <key>CFBundleDisplayName</key>
    <string>Stremio Enhanced</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>5.0.27</string>
    <key>CFBundleVersion</key>
    <string>5.0.27</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key>
    <true/>
</dict>
</plist>
EOF

# Copy app icon if available
if [ -f "images/stremio.png" ]; then
    cp images/stremio.png "$RESOURCES_DIR/icon.png"
fi

echo "=========================================================="
echo "Build succeeded!"
echo "App bundle created at: $APP_DIR"
echo "You can launch it with: open \"$APP_DIR\""
echo "=========================================================="

