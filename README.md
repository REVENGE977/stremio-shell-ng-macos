# Stremio Enhanced Shell (`stremio-shell-ng`) — macOS Port

This repository contains the macOS port of `stremio-shell-ng` supporting both **Intel Macs (`x86_64-apple-darwin`, including MacPorts environments)** and **Apple Silicon Macs (`aarch64-apple-darwin`)**.

## Architecture on macOS

1. **Embedded MPV Player (`CAOpenGLLayer` + `libmpv` Render API)**:
   - Uses `libmpv`'s `mpv_render_context` API (`vo=libmpv`, `hwdec=auto-safe`) rendered into a custom `CAOpenGLLayer` (`StremioMpvOpenGLLayer`) attached to an `NSView` positioned directly underneath the `WKWebView` inside `WryWebViewParent`.
   - Unlike `--wid` (which fails or opens a separate window on MacPorts and modern Homebrew `libmpv` builds), `CAOpenGLLayer` + `MPV_RENDER_API_TYPE_OPENGL` works across **all** MacPorts and Homebrew `mpv` builds on both Intel and Apple Silicon Macs.
2. **Transparent `WKWebView` Overlay (`tao` + `wry`)**:
   - Renders the Stremio WebUI and Stremio Enhanced plugins/themes with a transparent background (`wry` `transparent` feature enabled) above the OpenGL video layer.
   - Provides full `window.qt.webChannelTransport` and `window.chrome.webview` shims on top of `wry`'s native `window.ipc` handler.
3. **Streaming Server & FFmpeg Discovery**:
   - Automatically searches MacPorts (`/opt/local/bin`), Homebrew (`/opt/homebrew/bin`, `/usr/local/bin`), and `.app` bundle resources for `node`, `ffmpeg`, `ffprobe`, and `server.js`.

---

## Building on an Intel Mac with MacPorts (No Homebrew)

1. Ensure **Xcode Command Line Tools**, **MacPorts**, and **Rust** are installed:
   ```bash
   xcode-select --install
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```
2. Run the MacPorts build script:
   ```bash
   chmod +x build-macos-macports.sh
   ./build-macos-macports.sh
   ```
   The script will:
   - Detect MacPorts in `/opt/local` (or `$MACPORTS_PREFIX` if overridden).
   - Install `mpv`, `pkgconfig`, and `nodejs` / `npm` via `sudo port install` if not already installed.
   - Configure linker flags (`-L /opt/local/lib`, `-Wl,-rpath,/opt/local/lib`, `-Wl,-rpath,@executable_path/../Frameworks`).
   - Build `enhanced.js` and compile the Rust binary for `x86_64-apple-darwin`.
   - Package `target/release/Stremio Enhanced.app` (including bundling `libmpv*.dylib` into `Contents/Frameworks`).

---

## Building with Homebrew (Apple Silicon or Intel)

```bash
chmod +x build-macos.sh
./build-macos.sh
```

---

## Running the App

```bash
open "target/release/Stremio Enhanced.app"
```
Or run the binary directly from Terminal to see logs:
```bash
./target/release/stremio-enhanced-ng
```
