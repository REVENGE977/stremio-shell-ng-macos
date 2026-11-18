#!/usr/bin/env bash
set -euo pipefail

echo "=========================================================="
echo "    Stremio Enhanced Shell - macOS (Homebrew) Build Script"
echo "=========================================================="

# If Homebrew is not available but MacPorts is, automatically delegate to build-macos-macports.sh
if ! command -v brew &> /dev/null; then
    if command -v port &> /dev/null || [ -x "/opt/local/bin/port" ]; then
        echo "Homebrew not found, but MacPorts was detected. Running build-macos-macports.sh..."
        exec "$(dirname "$0")/build-macos-macports.sh" "$@"
    fi
    echo "Error: Homebrew is required for this script (or use ./build-macos-macports.sh for MacPorts)."
    exit 1
fi

BREW_PREFIX="$(brew --prefix)"
echo "==> Using Homebrew at: $BREW_PREFIX"

# Check for mpv
if ! brew list mpv &> /dev/null; then
    echo "==> Installing mpv via Homebrew..."
    brew install mpv
fi

# Check for dylibbundler so Frameworks/ receives a complete standalone dependency tree
if ! command -v dylibbundler &> /dev/null; then
    echo "==> Installing dylibbundler via Homebrew for standalone .app packaging..."
    brew install dylibbundler || true
fi

# Check for Node.js
if ! command -v node &> /dev/null; then
    echo "==> Installing node via Homebrew..."
    brew install node
fi

# Export pkg-config and library search paths
export MPV_LIB_DIR="$BREW_PREFIX/lib"
export PKG_CONFIG_PATH="$BREW_PREFIX/lib/pkgconfig:${PKG_CONFIG_PATH:-}"
export LIBRARY_PATH="$BREW_PREFIX/lib:${LIBRARY_PATH:-}"
export DYLD_FALLBACK_LIBRARY_PATH="$BREW_PREFIX/lib:${DYLD_FALLBACK_LIBRARY_PATH:-}"
export RUSTFLAGS="-L native=$BREW_PREFIX/lib -C link-arg=-Wl,-rpath,$BREW_PREFIX/lib -C link-arg=-Wl,-rpath,@executable_path/../Frameworks ${RUSTFLAGS:-}"

# 1. Build TypeScript / Enhanced frontend bundle
echo "==> Building Stremio Enhanced frontend bundle..."
pushd src/stremio_app/enhanced > /dev/null
if [ -f "package.json" ]; then
    npm install --no-audit --no-fund
    node build.js
fi
popd > /dev/null

# 2. Build Rust release binary
echo "==> Compiling Stremio Enhanced native shell..."
cargo build --release

# 3. Create macOS .app bundle
APP_NAME="Stremio Enhanced.app"
APP_DIR="target/release/$APP_NAME"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
FRAMEWORKS_DIR="$CONTENTS_DIR/Frameworks"
RESOURCES_DIR="$CONTENTS_DIR/Resources"

echo "==> Packaging into $APP_NAME bundle..."
rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR" "$FRAMEWORKS_DIR" "$RESOURCES_DIR"

cp target/release/stremio-enhanced-ng "$MACOS_DIR/"
chmod +x "$MACOS_DIR/stremio-enhanced-ng"

# Bundle node executable into Contents/Resources so recipients do not need Node installed
NODE_BIN="$(command -v node || true)"
if [ -n "$NODE_BIN" ] && [ -x "$NODE_BIN" ]; then
    echo "==> Bundling standalone Node runtime ($NODE_BIN)..."
    cp -L "$NODE_BIN" "$RESOURCES_DIR/node"
    chmod +x "$RESOURCES_DIR/node"
fi

# Bundle ffmpeg / ffprobe if available
FFMPEG_BIN="$(command -v ffmpeg || true)"
if [ -n "$FFMPEG_BIN" ] && [ -x "$FFMPEG_BIN" ]; then
    echo "==> Bundling ffmpeg ($FFMPEG_BIN)..."
    cp -L "$FFMPEG_BIN" "$RESOURCES_DIR/ffmpeg"
    chmod +x "$RESOURCES_DIR/ffmpeg"
fi

FFPROBE_BIN="$(command -v ffprobe || true)"
if [ -n "$FFPROBE_BIN" ] && [ -x "$FFPROBE_BIN" ]; then
    echo "==> Bundling ffprobe ($FFPROBE_BIN)..."
    cp -L "$FFPROBE_BIN" "$RESOURCES_DIR/ffprobe"
    chmod +x "$RESOURCES_DIR/ffprobe"
fi

# Note: server.js is never distributed in the app bundle due to licensing.

# Bundle dynamic dependencies for stremio-enhanced-ng AND bundled node/ffmpeg/ffprobe tools
BUNDLE_MODE="x86_64"
if [ "$(uname -m)" = "arm64" ]; then
    BUNDLE_MODE="arm64"
fi
echo "==> Bundling dynamic dependencies into $FRAMEWORKS_DIR..."
python3 "$(dirname "$0")/scripts/prepare-macos-deps.py" bundle-app \
    --app-dir "$APP_DIR" \
    --macports-prefix "$BREW_PREFIX" \
    --mode "$BUNDLE_MODE"

install_name_tool -add_rpath "@executable_path/../Frameworks" "$MACOS_DIR/stremio-enhanced-ng" 2>/dev/null || true

if [ -f "images/stremio.png" ]; then
    cp "images/stremio.png" "$RESOURCES_DIR/icon.png"
    if command -v sips &> /dev/null && command -v iconutil &> /dev/null; then
        ICONSET_DIR="$(mktemp -d)/AppIcon.iconset"
        mkdir -p "$ICONSET_DIR"
        sips -z 16 16     "images/stremio.png" --out "$ICONSET_DIR/icon_16x16.png"      > /dev/null 2>&1 || true
        sips -z 32 32     "images/stremio.png" --out "$ICONSET_DIR/icon_16x16@2x.png"   > /dev/null 2>&1 || true
        sips -z 32 32     "images/stremio.png" --out "$ICONSET_DIR/icon_32x32.png"      > /dev/null 2>&1 || true
        sips -z 64 64     "images/stremio.png" --out "$ICONSET_DIR/icon_32x32@2x.png"   > /dev/null 2>&1 || true
        sips -z 128 128   "images/stremio.png" --out "$ICONSET_DIR/icon_128x128.png"    > /dev/null 2>&1 || true
        sips -z 256 256   "images/stremio.png" --out "$ICONSET_DIR/icon_128x128@2x.png" > /dev/null 2>&1 || true
        sips -z 256 256   "images/stremio.png" --out "$ICONSET_DIR/icon_256x256.png"    > /dev/null 2>&1 || true
        sips -z 512 512   "images/stremio.png" --out "$ICONSET_DIR/icon_256x256@2x.png" > /dev/null 2>&1 || true
        sips -z 512 512   "images/stremio.png" --out "$ICONSET_DIR/icon_512x512.png"    > /dev/null 2>&1 || true
        sips -z 1024 1024 "images/stremio.png" --out "$ICONSET_DIR/icon_512x512@2x.png" > /dev/null 2>&1 || true
        iconutil -c icns "$ICONSET_DIR" -o "$RESOURCES_DIR/AppIcon.icns" 2>/dev/null || true
        rm -rf "$(dirname "$ICONSET_DIR")"
    fi
fi

cat > "$CONTENTS_DIR/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>stremio-enhanced-ng</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
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
    <key>NSAppTransportSecurity</key>
    <dict>
        <key>NSAllowsArbitraryLoads</key>
        <true/>
        <key>NSAllowsLocalNetworking</key>
        <true/>
    </dict>
    <key>NSLocalNetworkUsageDescription</key>
    <string>Stremio Enhanced connects to local and network streaming servers.</string>
</dict>
</plist>
EOF

codesign --force --deep --sign - "$APP_DIR"
codesign --verify --deep --strict "$APP_DIR"

echo "=========================================================="
echo "Build succeeded!"
echo "App bundle created at: $APP_DIR"
echo "You can launch it with: open \"$APP_DIR\""
echo "=========================================================="
