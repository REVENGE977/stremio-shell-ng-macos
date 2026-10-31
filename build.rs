use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    setup_enhanced();

    let target = env::var("TARGET").unwrap_or_default();
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    if target_os == "windows" {
        #[cfg(windows)]
        setup_windows(&target);
    } else if target_os == "macos" {
        setup_macos(&target);
    } else {
        panic!("Unsupported target: {}", target);
    }
}

#[cfg(windows)]
fn setup_windows(target: &str) {
    use chrono::{Datelike, Local};
    use std::io::Cursor;

    let now = Local::now();
    let copyright = format!("Copyright (c) {} REVENGE977 & Smart Code OOD", now.year());
    let pkg_name = env::var("CARGO_PKG_NAME").unwrap_or_else(|_| "stremio-enhanced-ng".to_string());
    let pkg_version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "5.0.27".to_string());
    let exe_name = format!("{pkg_name}.exe");
    let mut res = winres::WindowsResource::new();
    res.set_manifest(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
    <assemblyIdentity
        type="win32"
        name="stremio-enhanced-ng"
        version="5.0.27.0"
        processorArchitecture="*"
    />
    <dependency>
        <dependentAssembly>
            <assemblyIdentity
                type="win32"
                name="Microsoft.Windows.Common-Controls"
                version="6.0.0.0"
                processorArchitecture="*"
                publicKeyToken="6595b64144ccf1df"
                language="*"
            />
        </dependentAssembly>
    </dependency>
    <application xmlns="urn:schemas-microsoft-com:asm.v3">
        <windowsSettings>
            <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
            <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2, PerMonitor</dpiAwareness>
            <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
        </windowsSettings>
    </application>
    <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
        <application>
            <!-- Windows 10 and Windows 11 -->
            <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
            <!-- Windows 8.1 -->
            <supportedOS Id="{1f676c76-80e1-4239-95bb-83d0f6d0da78}"/>
            <!-- Windows 8 -->
            <supportedOS Id="{4a2f28e3-53b9-4441-ba9c-d69d4a4a6e38}"/>
            <!-- Windows 7 -->
            <supportedOS Id="{35138b9a-5d96-4fbd-8e2d-a2440225f93a}"/>
        </application>
    </compatibility>
    <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
        <security>
            <requestedPrivileges>
                <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
            </requestedPrivileges>
        </security>
    </trustInfo>
</assembly>"#,
    );
    res.set("FileDescription", "Stremio Enhanced - Freedom to Stream");
    res.set("ProductName", "Stremio Enhanced");
    res.set("ProductVersion", &pkg_version);
    res.set("FileVersion", &pkg_version);
    res.set("CompanyName", "Smart Code OOD & REVENGE977");
    res.set("InternalName", "stremio-enhanced-ng");
    res.set("LegalCopyright", &copyright);
    res.set("OriginalFilename", &exe_name);
    res.set_icon_with_id("images/stremio.ico", "MAINICON");
    res.append_rc_content(r##"SPLASHIMAGE IMAGE "images/stremio.png""##);
    res.compile().unwrap();

    let (arch, archive, flags) = match target {
        "x86_64-pc-windows-msvc" => ("x64", "libmpv-2_x64.zip", "/LIBPATH:.\\mpv-x64"),
        "aarch64-pc-windows-msvc" => ("arm64", "libmpv-2_arm64.zip", "/LIBPATH:.\\mpv-arm64"),
        _ => panic!("Unsupported Windows target {}", target),
    };
    println!("cargo:rustc-env=ARCH={}", arch);
    println!("cargo:rustc-link-arg={}", flags);
    let archive = fs::read(archive).unwrap();
    let target_dir = PathBuf::from(".");
    zip_extract::extract(Cursor::new(archive), &target_dir, true).ok();
}

fn setup_macos(target: &str) {
    let arch = match target {
        "aarch64-apple-darwin" => "arm64",
        "x86_64-apple-darwin" => "x64",
        _ => "universal",
    };
    println!("cargo:rustc-env=ARCH={}", arch);
    println!("cargo:rerun-if-env-changed=MPV_LIB_DIR");
    println!("cargo:rerun-if-env-changed=MACPORTS_PREFIX");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");

    // Allow explicit override via MPV_LIB_DIR or MACPORTS_PREFIX
    if let Ok(mpv_lib_dir) = env::var("MPV_LIB_DIR") {
        if !mpv_lib_dir.is_empty() {
            println!("cargo:rustc-link-search=native={}", mpv_lib_dir);
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", mpv_lib_dir);
        }
    }
    if let Ok(macports_prefix) = env::var("MACPORTS_PREFIX") {
        let lib_dir = format!("{}/lib", macports_prefix.trim_end_matches('/'));
        println!("cargo:rustc-link-search=native={}", lib_dir);
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_dir);
    }

    // Try pkg-config (checks Homebrew and MacPorts if PKG_CONFIG_PATH or pkg-config is available)
    for pkg_cfg_bin in [
        "pkg-config",
        "/opt/local/bin/pkg-config",
        "/opt/homebrew/bin/pkg-config",
        "/usr/local/bin/pkg-config",
    ] {
        if let Ok(output) = Command::new(pkg_cfg_bin)
            .args(["--libs-only-L", "mpv"])
            .output()
        {
            if output.status.success() {
                let flags = String::from_utf8_lossy(&output.stdout);
                for token in flags.split_whitespace() {
                    if let Some(dir) = token.strip_prefix("-L") {
                        println!("cargo:rustc-link-search=native={}", dir);
                        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir);
                    }
                }
                break;
            }
        }
    }

    // Standard library locations for MacPorts (/opt/local/lib), Homebrew Apple Silicon (/opt/homebrew/lib),
    // and Homebrew Intel (/usr/local/lib)
    for dir in ["/opt/local/lib", "/opt/homebrew/lib", "/usr/local/lib"] {
        if Path::new(dir).exists() {
            println!("cargo:rustc-link-search=native={}", dir);
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir);
        }
    }

    // Support bundled libmpv.dylib inside Stremio Enhanced.app/Contents/Frameworks
    println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");

    println!("cargo:rustc-link-lib=dylib=mpv");
    println!("cargo:rustc-link-lib=framework=Cocoa");
    println!("cargo:rustc-link-lib=framework=QuartzCore");
    println!("cargo:rustc-link-lib=framework=OpenGL");
}

fn watch_dir_recursively(dir: &Path) {
    if !dir.exists() {
        return;
    }
    println!("cargo:rerun-if-changed={}", dir.display());
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                watch_dir_recursively(&path);
            } else {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
}

fn setup_enhanced() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let enhanced_dir = manifest_dir.join("src").join("stremio_app").join("enhanced");
    let enhanced_src = enhanced_dir.join("src");
    let enhanced_build_js = enhanced_dir.join("build.js");
    let enhanced_package_json = enhanced_dir.join("package.json");
    let enhanced_tsconfig = enhanced_dir.join("tsconfig.json");
    let enhanced_output = enhanced_dir.join("enhanced.js");

    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=build.rs");
    watch_dir_recursively(&enhanced_src);
    println!("cargo:rerun-if-changed={}", enhanced_build_js.display());
    println!("cargo:rerun-if-changed={}", enhanced_package_json.display());
    println!("cargo:rerun-if-changed={}", enhanced_tsconfig.display());

    let node_candidates = [
        "node",
        "/opt/local/bin/node",
        "/opt/homebrew/bin/node",
        "/usr/local/bin/node",
    ];

    let mut last_err = None;
    let mut ran_successfully = false;

    for node_bin in node_candidates {
        match Command::new(node_bin)
            .arg(&enhanced_build_js)
            .current_dir(&enhanced_dir)
            .status()
        {
            Ok(s) if s.success() => {
                ran_successfully = true;
                break;
            }
            Ok(s) => {
                last_err = Some(format!("{} exited with {:?}", node_bin, s.code()));
            }
            Err(e) => {
                last_err = Some(format!("{}: {}", node_bin, e));
            }
        }
    }

    if !ran_successfully {
        if !enhanced_output.exists() {
            panic!(
                "Failed to compile enhanced.js ({:?}), and no existing bundle was found. Please install Node.js.",
                last_err
            );
        } else {
            eprintln!(
                "Warning: Failed to rebuild enhanced.js ({:?}); using existing bundle.",
                last_err
            );
        }
    }
}

