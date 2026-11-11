#!/usr/bin/env bash
set -euo pipefail

echo "=================================================================="
echo "  Stremio Enhanced Shell - Cross-Compile for Apple Silicon (ARM64)"
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

# 1. Ensure rustup & aarch64-apple-darwin target are installed
# Note: MacPorts' standalone 'rust' port only ships the host stdlib (x86_64)
# and cannot cross-compile to aarch64-apple-darwin without rustup.
if ! command -v rustup &> /dev/null; then
    echo "==> 'rustup' not found (MacPorts rust only includes host stdlib)."
    echo "==> Installing rustup to provide the aarch64-apple-darwin standard library..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
    if [ -f "$HOME/.cargo/env" ]; then
        # shellcheck disable=SC1091
        source "$HOME/.cargo/env"
    fi
    export PATH="$HOME/.cargo/bin:$PATH"
fi

# Ensure a default rustup toolchain is configured
if ! rustup show active-toolchain &> /dev/null; then
    echo "==> Installing stable Rust toolchain via rustup..."
    rustup toolchain install stable
    rustup default stable
fi

echo "==> Ensuring aarch64-apple-darwin target is installed via rustup..."
rustup target add aarch64-apple-darwin

RUSTUP_CARGO="$(rustup which cargo)"
RUSTUP_RUSTC="$(rustup which rustc)"
export CARGO="$RUSTUP_CARGO"
export RUSTC="$RUSTUP_RUSTC"
echo "==> Using rustc: $RUSTUP_RUSTC ($("$RUSTUP_RUSTC" --version))"
echo "==> Using cargo: $RUSTUP_CARGO ($("$RUSTUP_CARGO" --version))"

# 2. Check for universal / arm64 libmpv
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

HAS_ARM64_MPV=false
if [ -f "$MPV_LIB" ]; then
    ARCHS=$(lipo -archs "$MPV_LIB" 2>/dev/null || file "$MPV_LIB")
    echo "==> Host libmpv architectures: $ARCHS"
    if [[ "$ARCHS" =~ "arm64" ]]; then
        HAS_ARM64_MPV=true
    else
        echo "==> Host libmpv is $ARCHS-only; build.rs will generate an arm64 @rpath/libmpv.2.dylib link stub."
    fi
fi

if [ "$HAS_ARM64_MPV" = true ]; then
    if ! command -v dylibbundler &> /dev/null && [ ! -x "$MACPORTS_PREFIX/bin/dylibbundler" ]; then
        echo "==> Installing dylibbundler via MacPorts..."
        sudo "$PORT_CMD" -N install dylibbundler || true
    fi
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

# 5. Compile for aarch64-apple-darwin
export MPV_LIB_DIR="$MACPORTS_PREFIX/lib"
export PKG_CONFIG_PATH="$MACPORTS_PREFIX/lib/pkgconfig:${PKG_CONFIG_PATH:-}"
if [ "$HAS_ARM64_MPV" = true ]; then
    export LIBRARY_PATH="$MACPORTS_PREFIX/lib:${LIBRARY_PATH:-}"
    export DYLD_FALLBACK_LIBRARY_PATH="$MACPORTS_PREFIX/lib:${DYLD_FALLBACK_LIBRARY_PATH:-}"
    export C_INCLUDE_PATH="$MACPORTS_PREFIX/include:${C_INCLUDE_PATH:-}"
fi

echo "==> Compiling for aarch64-apple-darwin (Apple Silicon)..."
"$RUSTUP_CARGO" build --release --target aarch64-apple-darwin

ARM_BIN="target/aarch64-apple-darwin/release/stremio-enhanced-ng"
if [ ! -f "$ARM_BIN" ]; then
    echo "Error: Target binary $ARM_BIN was not created."
    exit 1
fi

# 6. Package into .app bundle
APP_NAME="Stremio Enhanced (ARM64).app"
APP_DIR="target/release/$APP_NAME"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
FRAMEWORKS_DIR="$CONTENTS_DIR/Frameworks"
RESOURCES_DIR="$CONTENTS_DIR/Resources"

echo "==> Packaging into $APP_NAME..."
rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR" "$FRAMEWORKS_DIR" "$RESOURCES_DIR"

cp "$ARM_BIN" "$MACOS_DIR/stremio-enhanced-ng"
chmod +x "$MACOS_DIR/stremio-enhanced-ng"

echo "==> Binary arch verification:"
file "$MACOS_DIR/stremio-enhanced-ng"

# 7. Bundle Node.js runtime into Contents/Resources
NODE_BIN="$(command -v node || true)"
if [ -n "$NODE_BIN" ] && [ -x "$NODE_BIN" ]; then
    echo "==> Bundling Node runtime ($NODE_BIN)..."
    cp -L "$NODE_BIN" "$RESOURCES_DIR/node"
    chmod +x "$RESOURCES_DIR/node"
fi

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

# 8. Bundle dynamic dependencies (if host has arm64 / universal dylibs)
if [ "$HAS_ARM64_MPV" = true ]; then
    echo "==> Bundling arm64 dynamic dependencies into $FRAMEWORKS_DIR..."
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
        "$BUNDLER_CMD" -b \
            -x "$MACOS_DIR/stremio-enhanced-ng" \
            -d "$FRAMEWORKS_DIR" \
            -p "@executable_path/../Frameworks" \
            -s "$MACPORTS_PREFIX/lib" \
            -of || true
    else
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
else
    echo "==> Skipping x86_64-only dylib bundling for ARM64 target."
    echo "    Binary is linked with @rpath/libmpv.2.dylib (searches Frameworks/, /opt/homebrew/lib, /opt/local/lib)."
fi

install_name_tool -add_rpath "@executable_path/../Frameworks" "$MACOS_DIR/stremio-enhanced-ng" 2>/dev/null || true
install_name_tool -add_rpath "/opt/homebrew/lib" "$MACOS_DIR/stremio-enhanced-ng" 2>/dev/null || true
install_name_tool -add_rpath "/opt/local/lib" "$MACOS_DIR/stremio-enhanced-ng" 2>/dev/null || true

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
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key>
    <true/>
</dict>
</plist>
EOF

if command -v codesign &> /dev/null; then
    codesign --force --deep --sign - "$APP_DIR" 2>/dev/null || true
fi

echo "=================================================================="
echo "Apple Silicon (ARM64) build succeeded!"
echo "App bundle created at: $APP_DIR"
echo "=================================================================="

