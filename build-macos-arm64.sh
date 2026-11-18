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
    if [ "$(uname -m)" = "arm64" ] && command -v brew &> /dev/null; then
        echo "==> MacPorts not found, but Homebrew on Apple Silicon was detected. Delegating to build-macos.sh..."
        exec "$(dirname "$0")/build-macos.sh" "$@"
    fi
    echo "Error: MacPorts ('port') was not found at $PORT_BIN or in PATH."
    echo "Please install MacPorts from https://www.macports.org/install.php"
    echo "(On Apple Silicon Macs with Homebrew, run ./build-macos.sh directly.)"
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
    fi
fi

if ! command -v python3 &> /dev/null; then
    echo "==> Installing python3 via MacPorts..."
    sudo "$PORT_CMD" -N install python312 || true
fi

ARM64_STAGE_DIR=""
if [ "$HAS_ARM64_MPV" != true ]; then
    echo "==> Staging prebuilt Apple Silicon (arm64) libmpv & runtime dependencies..."
    ARM64_STAGE_DIR="$(pwd)/target/arm64-macports-stage"
    ARM64_CACHE_DIR="$(pwd)/target/arm64-macports-cache"
    python3 "$(dirname "$0")/scripts/prepare-macos-deps.py" stage-arm64 \
        --macports-prefix "$MACPORTS_PREFIX" \
        --stage-dir "$ARM64_STAGE_DIR" \
        --cache-dir "$ARM64_CACHE_DIR"
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
if [ "$HAS_ARM64_MPV" = true ]; then
    export MPV_LIB_DIR="$MACPORTS_PREFIX/lib"
    export PKG_CONFIG_PATH="$MACPORTS_PREFIX/lib/pkgconfig:${PKG_CONFIG_PATH:-}"
    export LIBRARY_PATH="$MACPORTS_PREFIX/lib:${LIBRARY_PATH:-}"
    export DYLD_FALLBACK_LIBRARY_PATH="$MACPORTS_PREFIX/lib:${DYLD_FALLBACK_LIBRARY_PATH:-}"
    export C_INCLUDE_PATH="$MACPORTS_PREFIX/include:${C_INCLUDE_PATH:-}"
else
    export MPV_LIB_DIR="$ARM64_STAGE_DIR$MACPORTS_PREFIX/lib"
    export PKG_CONFIG_PATH="$ARM64_STAGE_DIR$MACPORTS_PREFIX/lib/pkgconfig"
    export LIBRARY_PATH="$ARM64_STAGE_DIR$MACPORTS_PREFIX/lib"
    export C_INCLUDE_PATH="$ARM64_STAGE_DIR$MACPORTS_PREFIX/include:${C_INCLUDE_PATH:-}"
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

# 7. App icon generation and Info.plist (must exist before codesigning the .app bundle)
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

# 8. Bundle arm64 resources, Frameworks/libmpv.2.dylib, codesign, and verify bundle
# Note: server.js is never distributed in the app bundle due to licensing.
echo "==> Packaging arm64 resources, Frameworks/libmpv.2.dylib, and verifying bundle..."
python3 "$(dirname "$0")/scripts/prepare-macos-deps.py" bundle-app \
    --app-dir "$APP_DIR" \
    --macports-prefix "$MACPORTS_PREFIX" \
    --arm64-stage "$ARM64_STAGE_DIR" \
    --mode arm64 \
    --stage-resources

codesign --force --deep --sign - "$APP_DIR"
codesign --verify --deep --strict "$APP_DIR"

echo "=================================================================="
echo "Apple Silicon (ARM64) build succeeded!"
echo "Standalone verified .app bundle created at: $APP_DIR"
echo "=================================================================="

