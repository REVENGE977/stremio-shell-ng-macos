use std::{env, fs, path::Path, process::Command};

fn main() {
    setup_enhanced();

    let target = env::var("TARGET").unwrap_or_default();
    setup_macos(&target);
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
