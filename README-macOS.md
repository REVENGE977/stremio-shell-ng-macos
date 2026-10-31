# Stremio Enhanced macOS Port

This repository contains the macOS-compatible version of **Stremio Shell NG (Enhanced)**, retaining the high-performance embedded MPV architecture without requiring a rewrite of the core player, streaming server, or Enhanced frontend subsystem.

---

## Architecture Overview

### 1. Embedded MPV Implementation on macOS

In the original Windows version:
- The shell attaches `libmpv` directly to the native `HWND` using mpv's `wid` (Window ID) option.
- An Edge WebView2 instance is layered directly on top of the window with an alpha channel of 0 (transparent background).
- When video playback starts, the WebUI route (`#/player`) triggers `transparencyBridge.ts`, which makes DOM containers transparent, allowing MPV's hardware-accelerated video frames to render directly beneath the HTML/CSS controls.

On macOS, the **exact same embedded architecture** is achieved natively:
1. **Native View Composition:**
   - The native `NSWindow`'s `contentView` hosts two synchronized subviews:
     - **Bottom View (`mpv_view`):** A native Cocoa `NSView` allocated with `[contentView addSubview:mpv_view positioned:NSWindowBelow relativeTo:nil]`.
     - **Top View (`WKWebView`):** A transparent native WebKit view (`wry` with `.with_transparent(true)`) that layers directly above `mpv_view`.
2. **MPV Window Embedding (`--wid`):**
   - The `libmpv` player instance is initialized with `set_property!("wid", mpv_view_ptr as i64)`.
   - On macOS, `libmpv` attaches directly to the provided `NSView` pointer and utilizes its modern **`gpu-next`** (Metal / libplacebo) or **`gpu`** video output backend with native **VideoToolbox (`hwdec = auto`)** hardware decoding.
3. **Transparent Web Overlay:**
   - Both `mpv_view` and `WKWebView` have `NSViewWidthSizable | NSViewHeightSizable` autoresizing masks, staying pixel-perfectly synchronized during window resizing and full-screen transitions.
   - All mouse, scroll, keyboard, and click interactions are intercepted by `WKWebView` on top.
   - All playback commands (`mpv-command`, subtitle additions, track selection, seeking, pause) route through the IPC channels to the background MPV client and subtitle worker threads.

---

## What Was Preserved vs. What Was Adapted

| Component | Status | Description |
| :--- | :--- | :--- |
| **`stremio_app/enhanced/`** (TypeScript/JS) | **100% Preserved** | All marketplace components, themes, plugins, modals, fetch interceptor, server manager, transparency bridge, and video bridge run unmodified in WebKit. |
| **`stremio_player/communication.rs`** | **100% Preserved** | Protocol message structures (`InMsg`, `CmdVal`, `PropVal`, `PlayerResponse`). |
| **`stremio_player/player.rs`** | **95% Preserved** | The subtitle thread, asynchronous worker queue, event thread, video readiness state tracking, and property observer logic are completely shared. Display output state and context setup are conditionally compiled (`d3d11` on Windows, native Cocoa/Metal on macOS). |
| **`stremio_server/`** | **95% Preserved** | Spawning `server.js` with Node.js runtime, stdout/stderr line streaming, and HTTP readiness checks are preserved. Windows Job Objects are replaced with Unix process groups on macOS. |
| **Window & Web Layer** | **Adapted** | Windows-only `native-windows-gui` and `webview2` are complemented with `tao` + `wry` + Cocoa view composition for macOS. |
| **Single-Instance IPC** | **Adapted** | Windows Named Pipes are complemented with Unix Domain Sockets (`/tmp/com.stremio5.<user>`). |

---

## Prerequisites (macOS)

Install dependencies via [Homebrew](https://brew.sh/):

```bash
# Install mpv (provides libmpv.dylib) and Node.js
brew install mpv node
```

Make sure Rust is installed:
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

---

## Building and Running

### Fast Build via Script:
```bash
chmod +x build-macos.sh
./build-macos.sh
```

### Manual Build:
```bash
# 1. Compile Enhanced frontend bundle
cd src/stremio_app/enhanced
npm install
node build.js
cd ../../../

# 2. Compile and run with Cargo
cargo run --release
```

The resulting application bundle will be created at:
`target/release/Stremio Enhanced.app`

