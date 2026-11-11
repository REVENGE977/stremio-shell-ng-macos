#!/usr/bin/env bash
set -euo pipefail

echo "=================================================================="
echo "  Stremio Enhanced Shell - Universal (Intel + Apple Silicon) Build"
echo "=================================================================="

# Allow overriding MacPorts prefix (defaults to /opt/local)
MACPORTS_PREFIX="${MACPORTS_PREFIX:-/opt/local}"
export MACPORTS_PREFIX
# Ensure ~/.cargo/bin takes precedence over MacPorts so rustup's multi-target
# toolchain is used instead of MacPorts' host-only rust/cargo package.
if [ -f "$HOME/.cargo/env" ]; then
    # shellcheck disable=SC1091
    source "$HOME/.cargo/env"
fi
export PATH="$HOME/.cargo/bin:$MACPORTS_PREFIX/bin:$MACPORTS_PREFIX/sbin:$PATH"

PORT_BIN="$MACPORTS_PREFIX/bin/port"
if ! command -v port &> /dev/null && [ ! -x "$PORT_BIN" ]; then
    echo "Error: MacPorts ('port') was not found at $PORT_BIN or in PATH."
    echo "Please install MacPorts from https://www.macports.org/install.php"
    exit 1
fi
PORT_CMD="${PORT_BIN}"
if [ ! -x "$PORT_CMD" ]; then
    PORT_CMD="$(command -v port)"
fi

echo "==> Using MacPorts at: $MACPORTS_PREFIX ($PORT_CMD)"

# 1. Ensure rustup & targets for both x86_64 and aarch64
if ! command -v rustup &> /dev/null; then
    echo "==> 'rustup' not found (MacPorts rust only includes host stdlib)."
    echo "==> Installing rustup to provide both x86_64 and aarch64 standard libraries..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
    if [ -f "$HOME/.cargo/env" ]; then
        # shellcheck disable=SC1091
        source "$HOME/.cargo/env"
    fi
    export PATH="$HOME/.cargo/bin:$PATH"
fi

if ! rustup show active-toolchain &> /dev/null; then
    echo "==> Installing stable Rust toolchain via rustup..."
    rustup toolchain install stable
    rustup default stable
fi

echo "==> Ensuring Rust targets for both architectures..."
rustup target add x86_64-apple-darwin
rustup target add aarch64-apple-darwin

RUSTUP_CARGO="$(rustup which cargo)"
RUSTUP_RUSTC="$(rustup which rustc)"
export CARGO="$RUSTUP_CARGO"
export RUSTC="$RUSTUP_RUSTC"
echo "==> Using rustc: $RUSTUP_RUSTC ($("$RUSTUP_RUSTC" --version))"
echo "==> Using cargo: $RUSTUP_CARGO ($("$RUSTUP_CARGO" --version))"

# 2. Check or install mpv
MPV_LIB="$MACPORTS_PREFIX/lib/libmpv.dylib"
if [ ! -f "$MPV_LIB" ]; then
    MPV_LIB="$MACPORTS_PREFIX/lib/libmpv.2.dylib"
fi

if [ ! -f "$MPV_LIB" ]; then
    echo "==> Installing mpv via MacPorts..."
    sudo "$PORT_CMD" -N install mpv || true
    if [ -f "$MACPORTS_PREFIX/lib/libmpv.dylib" ]; then
        MPV_LIB="$MACPORTS_PREFIX/lib/libmpv.dylib"
    elif [ -f "$MACPORTS_PREFIX/lib/libmpv.2.dylib" ]; then
        MPV_LIB="$MACPORTS_PREFIX/lib/libmpv.2.dylib"
    fi
fi

if [ -f "$MPV_LIB" ]; then
    ARCHS=$(lipo -archs "$MPV_LIB" 2>/dev/null || file "$MPV_LIB")
    echo "==> Host libmpv architectures: $ARCHS"
    echo "    (build.rs will automatically generate a cross-arch link stub for any missing slice)"
fi

if ! command -v dylibbundler &> /dev/null && [ ! -x "$MACPORTS_PREFIX/bin/dylibbundler" ]; then
    echo "==> Installing dylibbundler via MacPorts..."
    sudo "$PORT_CMD" -N install dylibbundler || true
fi

# 3. Ensure Node.js & npm are installed
if ! command -v node &> /dev/null || ! command -v npm &> /dev/null; then
    echo "==> Installing Node.js via MacPorts..."
    if "$PORT_CMD" info nodejs22 &> /dev/null; then
        sudo "$PORT_CMD" -N install nodejs22 npm10 || sudo "$PORT_CMD" -N install nodejs20 npm9
    elif "$PORT_CMD" info nodejs20 &> /dev/null; then
        sudo "$PORT_CMD" -N install nodejs20 npm9
    else
        sudo "$PORT_CMD" -N install nodejs18 npm8
    fi
fi

# 4. Build TypeScript / Enhanced frontend bundle
echo "==> Building Stremio Enhanced frontend bundle..."
pushd src/stremio_app/enhanced > /dev/null
if [ -f "package.json" ]; then
    npm install --no-audit --no-fund
    node build.js
fi
popd > /dev/null

# 5. Compile both architectures
export MPV_LIB_DIR="$MACPORTS_PREFIX/lib"
export PKG_CONFIG_PATH="$MACPORTS_PREFIX/lib/pkgconfig:${PKG_CONFIG_PATH:-}"

echo "==> [1/2] Compiling for Intel (x86_64-apple-darwin)..."
"$RUSTUP_CARGO" build --release --target x86_64-apple-darwin

echo "==> [2/2] Compiling for Apple Silicon (aarch64-apple-darwin)..."
"$RUSTUP_CARGO" build --release --target aarch64-apple-darwin

X86_BIN="target/x86_64-apple-darwin/release/stremio-enhanced-ng"
ARM_BIN="target/aarch64-apple-darwin/release/stremio-enhanced-ng"

if [ ! -f "$X86_BIN" ] || [ ! -f "$ARM_BIN" ]; then
    echo "Error: One or both target binaries failed to compile."
    exit 1
fi

# 6. Package into a Universal 2 macOS .app bundle
APP_NAME="Stremio Enhanced.app"
APP_DIR="target/release/$APP_NAME"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
FRAMEWORKS_DIR="$CONTENTS_DIR/Frameworks"
RESOURCES_DIR="$CONTENTS_DIR/Resources"

echo "==> Creating Universal binary using lipo..."
rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR" "$FRAMEWORKS_DIR" "$RESOURCES_DIR"

lipo -create "$X86_BIN" "$ARM_BIN" -output "$MACOS_DIR/stremio-enhanced-ng"
chmod +x "$MACOS_DIR/stremio-enhanced-ng"

echo "==> Verified Universal executable slices:"
lipo -info "$MACOS_DIR/stremio-enhanced-ng"

# 7. Bundle Node.js runtime into Contents/Resources
NODE_BIN="$(command -v node || true)"
if [ -n "$NODE_BIN" ] && [ -x "$NODE_BIN" ]; then
    echo "==> Bundling Node runtime ($NODE_BIN)..."
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

if [ -f "server.js" ]; then
    cp "server.js" "$RESOURCES_DIR/server.js"
fi

# 8. Bundle and rewrite dynamic libraries
echo "==> Bundling dynamic dependencies into $FRAMEWORKS_DIR..."
BUNDLER_CMD="$(command -v dylibbundler || true)"
if [ -z "$BUNDLER_CMD" ] && [ -x "$MACPORTS_PREFIX/bin/dylibbundler" ]; then
    BUNDLER_CMD="$MACPORTS_PREFIX/bin/dylibbundler"
fi

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

# 9. App icon generation
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

# 10. Info.plist
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

# 11. Ad-hoc codesign
if command -v codesign &> /dev/null; then
    codesign --force --deep --sign - "$APP_DIR" 2>/dev/null || true
fi

echo "=================================================================="
echo "Universal build succeeded!"
echo "Universal App bundle created at: $APP_DIR"
echo "Binary info:"
lipo -info "$MACOS_DIR/stremio-enhanced-ng"
echo "You can launch it with: open \"$APP_DIR\""
echo "=================================================================="

