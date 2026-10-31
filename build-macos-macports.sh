#!/usr/bin/env bash
set -euo pipefail

echo "=================================================================="
echo "  Stremio Enhanced Shell - Intel macOS (MacPorts) Build Script"
echo "=================================================================="

# Allow overriding MacPorts prefix (defaults to /opt/local)
MACPORTS_PREFIX="${MACPORTS_PREFIX:-/opt/local}"
export MACPORTS_PREFIX
export PATH="$MACPORTS_PREFIX/bin:$MACPORTS_PREFIX/sbin:$PATH"

PORT_BIN="$MACPORTS_PREFIX/bin/port"
if ! command -v port &> /dev/null && [ ! -x "$PORT_BIN" ]; then
    echo "Error: MacPorts ('port') was not found at $PORT_BIN or in PATH."
    echo "Please install MacPorts from https://www.macports.org/install.php"
    echo "or set MACPORTS_PREFIX=/path/to/macports if installed in a custom prefix."
    exit 1
fi
PORT_CMD="${PORT_BIN}"
if [ ! -x "$PORT_CMD" ]; then
    PORT_CMD="$(command -v port)"
fi

echo "==> Using MacPorts at: $MACPORTS_PREFIX ($PORT_CMD)"

# 1. Ensure pkgconfig, mpv, and dylibbundler are installed via MacPorts
if [ ! -f "$MACPORTS_PREFIX/lib/libmpv.dylib" ] && [ ! -f "$MACPORTS_PREFIX/lib/libmpv.2.dylib" ]; then
    echo "==> libmpv not found in $MACPORTS_PREFIX/lib. Installing mpv + pkgconfig via MacPorts..."
    echo "    (You may be prompted for your sudo password)"
    sudo "$PORT_CMD" -N install pkgconfig mpv
else
    echo "==> Found libmpv in $MACPORTS_PREFIX/lib"
fi

if ! command -v dylibbundler &> /dev/null && [ ! -x "$MACPORTS_PREFIX/bin/dylibbundler" ]; then
    echo "==> Installing dylibbundler via MacPorts to package standalone dynamic libraries..."
    sudo "$PORT_CMD" -N install dylibbundler || true
fi

# 2. Ensure Node.js and npm are installed
if ! command -v node &> /dev/null || ! command -v npm &> /dev/null; then
    echo "==> node/npm not found. Installing Node.js and npm via MacPorts..."
    if "$PORT_CMD" info nodejs22 &> /dev/null; then
        sudo "$PORT_CMD" -N install nodejs22 npm10 || sudo "$PORT_CMD" -N install nodejs20 npm9
    elif "$PORT_CMD" info nodejs20 &> /dev/null; then
        sudo "$PORT_CMD" -N install nodejs20 npm9
    else
        sudo "$PORT_CMD" -N install nodejs18 npm8
    fi
fi

echo "==> Using Node: $(command -v node) ($(node -v))"
echo "==> Using npm:  $(command -v npm) ($(npm -v))"

# 3. Ensure Rust / Cargo is installed
if ! command -v cargo &> /dev/null; then
    if [ -f "$HOME/.cargo/env" ]; then
        # shellcheck disable=SC1091
        source "$HOME/.cargo/env"
    fi
fi
if ! command -v cargo &> /dev/null; then
    echo "Error: Rust (cargo) is not installed."
    echo "Install it via: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    echo "Or via MacPorts: sudo port install rust cargo"
    exit 1
fi

# 4. Export compiler, pkg-config, and linker environment variables for MacPorts on Intel Mac
export MPV_LIB_DIR="$MACPORTS_PREFIX/lib"
export PKG_CONFIG_PATH="$MACPORTS_PREFIX/lib/pkgconfig:${PKG_CONFIG_PATH:-}"
export LIBRARY_PATH="$MACPORTS_PREFIX/lib:${LIBRARY_PATH:-}"
export DYLD_FALLBACK_LIBRARY_PATH="$MACPORTS_PREFIX/lib:${DYLD_FALLBACK_LIBRARY_PATH:-}"
export C_INCLUDE_PATH="$MACPORTS_PREFIX/include:${C_INCLUDE_PATH:-}"
export RUSTFLAGS="-L native=$MACPORTS_PREFIX/lib -C link-arg=-Wl,-rpath,$MACPORTS_PREFIX/lib -C link-arg=-Wl,-rpath,@executable_path/../Frameworks ${RUSTFLAGS:-}"

# Determine target triple (defaults to x86_64-apple-darwin for Intel Macs)
HOST_ARCH="$(uname -m)"
if [ "$HOST_ARCH" = "x86_64" ]; then
    RUST_TARGET="x86_64-apple-darwin"
else
    RUST_TARGET="${RUST_TARGET:-x86_64-apple-darwin}"
fi
echo "==> Target architecture: $RUST_TARGET (host: $HOST_ARCH)"

# If rustup is present, make sure the target is installed
if command -v rustup &> /dev/null; then
    rustup target add "$RUST_TARGET" || true
fi

# 5. Build TypeScript / Enhanced frontend bundle
echo "==> Building Stremio Enhanced frontend bundle..."
pushd src/stremio_app/enhanced > /dev/null
if [ -f "package.json" ]; then
    npm install --no-audit --no-fund
    node build.js
fi
popd > /dev/null

# 6. Build Rust release binary
echo "==> Compiling Stremio Enhanced native shell ($RUST_TARGET)..."
cargo build --release --target "$RUST_TARGET"

BIN_PATH="target/$RUST_TARGET/release/stremio-enhanced-ng"
if [ ! -f "$BIN_PATH" ]; then
    BIN_PATH="target/release/stremio-enhanced-ng"
fi

# 7. Package into macOS .app bundle
APP_NAME="Stremio Enhanced.app"
APP_DIR="target/release/$APP_NAME"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
FRAMEWORKS_DIR="$CONTENTS_DIR/Frameworks"
RESOURCES_DIR="$CONTENTS_DIR/Resources"

echo "==> Packaging into $APP_NAME..."
rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR" "$FRAMEWORKS_DIR" "$RESOURCES_DIR"

cp "$BIN_PATH" "$MACOS_DIR/stremio-enhanced-ng"
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
if [ -z "$FFMPEG_BIN" ] && [ -x "$MACPORTS_PREFIX/bin/ffmpeg" ]; then
    FFMPEG_BIN="$MACPORTS_PREFIX/bin/ffmpeg"
fi
if [ -n "$FFMPEG_BIN" ] && [ -x "$FFMPEG_BIN" ]; then
    echo "==> Bundling ffmpeg ($FFMPEG_BIN)..."
    cp -L "$FFMPEG_BIN" "$RESOURCES_DIR/ffmpeg"
    chmod +x "$RESOURCES_DIR/ffmpeg"
fi

FFPROBE_BIN="$(command -v ffprobe || true)"
if [ -z "$FFPROBE_BIN" ] && [ -x "$MACPORTS_PREFIX/bin/ffprobe" ]; then
    FFPROBE_BIN="$MACPORTS_PREFIX/bin/ffprobe"
fi
if [ -n "$FFPROBE_BIN" ] && [ -x "$FFPROBE_BIN" ]; then
    echo "==> Bundling ffprobe ($FFPROBE_BIN)..."
    cp -L "$FFPROBE_BIN" "$RESOURCES_DIR/ffprobe"
    chmod +x "$RESOURCES_DIR/ffprobe"
fi

# Copy server.js if present in workspace root or download/fetch default
if [ -f "server.js" ]; then
    cp "server.js" "$RESOURCES_DIR/server.js"
fi

# Bundle and fix all dynamic libraries using dylibbundler or recursive otool/install_name_tool
echo "==> Bundling dynamic dependencies into $FRAMEWORKS_DIR..."
BUNDLER_CMD="$(command -v dylibbundler || true)"
if [ -z "$BUNDLER_CMD" ] && [ -x "$MACPORTS_PREFIX/bin/dylibbundler" ]; then
    BUNDLER_CMD="$MACPORTS_PREFIX/bin/dylibbundler"
fi

# First copy initial libmpv dylib
for dylib in "$MACPORTS_PREFIX"/lib/libmpv*.dylib; do
    if [ -e "$dylib" ]; then
        cp -L "$dylib" "$FRAMEWORKS_DIR/"
    fi
done

if [ -n "$BUNDLER_CMD" ] && [ -x "$BUNDLER_CMD" ]; then
    echo "==> Resolving recursive dylib tree via dylibbundler..."
    "$BUNDLER_CMD" -b \
        -x "$MACOS_DIR/stremio-enhanced-ng" \
        -d "$FRAMEWORKS_DIR" \
        -p "@executable_path/../Frameworks" \
        -s "$MACPORTS_PREFIX/lib" \
        -of || true
else
    echo "==> Resolving dependencies recursively using otool/install_name_tool..."
    # Fallback pure bash recursive collector
    bundle_dylibs() {
        local target="$1"
        otool -L "$target" | awk 'NR>1 {print $1}' | while read -r dep; do
            case "$dep" in
                "$MACPORTS_PREFIX"/*|/usr/local/*|/opt/homebrew/*)
                    local base
                    base="$(basename "$dep")"
                    if [ ! -f "$FRAMEWORKS_DIR/$base" ] && [ -f "$dep" ]; then
                        cp -L "$dep" "$FRAMEWORKS_DIR/$base"
                        chmod 755 "$FRAMEWORKS_DIR/$base"
                        install_name_tool -id "@executable_path/../Frameworks/$base" "$FRAMEWORKS_DIR/$base" 2>/dev/null || true
                        bundle_dylibs "$FRAMEWORKS_DIR/$base"
                    fi
                    install_name_tool -change "$dep" "@executable_path/../Frameworks/$base" "$target" 2>/dev/null || true
                    ;;
            esac
        done
    }
    bundle_dylibs "$MACOS_DIR/stremio-enhanced-ng"
    for fw in "$FRAMEWORKS_DIR"/*.dylib; do
        if [ -f "$fw" ]; then
            bundle_dylibs "$fw"
        fi
    done
fi

install_name_tool -add_rpath "@executable_path/../Frameworks" "$MACOS_DIR/stremio-enhanced-ng" 2>/dev/null || true

# Generate AppIcon.icns from images/stremio.png using built-in macOS sips + iconutil
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

# Create Info.plist
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
    <string>10.15</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key>
    <true/>
</dict>
</plist>
EOF

# Ad-hoc codesign so macOS Gatekeeper allows local execution cleanly
if command -v codesign &> /dev/null; then
    codesign --force --deep --sign - "$APP_DIR" 2>/dev/null || true
fi

echo "=================================================================="
echo "Build succeeded!"
echo "App bundle created at: $APP_DIR"
echo "You can launch it with: open \"$APP_DIR\""
echo "=================================================================="

