use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    setup_enhanced();

    let target = env::var("TARGET").unwrap_or_default();
    setup_macos(&target);
}

fn dylib_has_arch(dylib: &Path, required_arch: &str) -> bool {
    if !dylib.exists() {
        return false;
    }
    if let Ok(output) = Command::new("lipo").arg("-archs").arg(dylib).output() {
        if output.status.success() {
            let archs = String::from_utf8_lossy(&output.stdout);
            return archs.split_whitespace().any(|a| a == required_arch);
        }
    }
    false
}

fn find_mpv_dylib_in_dir(dir: &Path) -> Option<PathBuf> {
    for name in ["libmpv.2.dylib", "libmpv.dylib"] {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn generate_mpv_arch_stub(required_arch: &str, host_dylib: Option<&Path>) -> Option<PathBuf> {
    let out_dir = PathBuf::from(env::var("OUT_DIR").ok()?);
    let stub_dir = out_dir.join(format!("mpv-stub-{}", required_arch));
    fs::create_dir_all(&stub_dir).ok()?;

    // Baseline symbols required by libmpv2, libmpv2-sys, and stremio-enhanced-ng
    let mut symbols: BTreeSet<String> = [
        "mpv_client_api_version",
        "mpv_error_string",
        "mpv_free",
        "mpv_client_name",
        "mpv_client_id",
        "mpv_create",
        "mpv_initialize",
        "mpv_destroy",
        "mpv_terminate_destroy",
        "mpv_create_client",
        "mpv_create_weak_client",
        "mpv_load_config_file",
        "mpv_get_time_ns",
        "mpv_get_time_us",
        "mpv_free_node_contents",
        "mpv_set_option",
        "mpv_set_option_string",
        "mpv_command",
        "mpv_command_node",
        "mpv_command_ret",
        "mpv_command_string",
        "mpv_command_async",
        "mpv_command_node_async",
        "mpv_abort_async_command",
        "mpv_set_property",
        "mpv_set_property_string",
        "mpv_set_property_async",
        "mpv_get_property",
        "mpv_get_property_string",
        "mpv_get_property_osd_string",
        "mpv_get_property_async",
        "mpv_observe_property",
        "mpv_unobserve_property",
        "mpv_event_name",
        "mpv_event_to_node",
        "mpv_request_event",
        "mpv_request_log_messages",
        "mpv_wait_event",
        "mpv_wakeup",
        "mpv_set_wakeup_callback",
        "mpv_wait_async_requests",
        "mpv_hook_add",
        "mpv_hook_continue",
        "mpv_get_sub_api",
        "mpv_render_context_create",
        "mpv_render_context_set_parameter",
        "mpv_render_context_get_info",
        "mpv_render_context_set_update_callback",
        "mpv_render_context_update",
        "mpv_render_context_render",
        "mpv_render_context_report_swap",
        "mpv_render_context_free",
        "mpv_stream_cb_add_ro",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();

    // Also harvest all exported _mpv_* symbols from the existing host libmpv.dylib if present
    if let Some(dylib) = host_dylib {
        if let Ok(output) = Command::new("nm").arg("-gU").arg(dylib).output() {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                for line in text.lines() {
                    for token in line.split_whitespace() {
                        if let Some(sym) = token.strip_prefix("_mpv_") {
                            let full_sym = format!("mpv_{}", sym);
                            if full_sym
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || c == '_')
                            {
                                symbols.insert(full_sym);
                            }
                        }
                    }
                }
            }
        }
    }

    let mut c_source = String::from("// Auto-generated libmpv link stub for cross-compilation\n");
    for sym in &symbols {
        c_source.push_str(&format!("void {}(void) {{}}\n", sym));
    }

    let c_path = stub_dir.join("mpv_stub.c");
    fs::write(&c_path, c_source).ok()?;

    let dylib_v2 = stub_dir.join("libmpv.2.dylib");
    let status = Command::new("clang")
        .args([
            "-arch",
            required_arch,
            "-dynamiclib",
            c_path.to_str()?,
            "-install_name",
            "@rpath/libmpv.2.dylib",
            "-current_version",
            "2.5.0",
            "-compatibility_version",
            "2.0.0",
            "-o",
            dylib_v2.to_str()?,
        ])
        .status()
        .ok()?;

    if !status.success() {
        return None;
    }

    let dylib_link = stub_dir.join("libmpv.dylib");
    let _ = fs::remove_file(&dylib_link);
    #[cfg(unix)]
    {
        let _ = std::os::unix::fs::symlink("libmpv.2.dylib", &dylib_link);
    }
    if !dylib_link.exists() {
        let _ = fs::copy(&dylib_v2, &dylib_link);
    }

    Some(stub_dir)
}

fn setup_macos(target: &str) {
    let (arch, required_macho_arch) = match target {
        "aarch64-apple-darwin" => ("arm64", "arm64"),
        "x86_64-apple-darwin" => ("x64", "x86_64"),
        _ => ("universal", ""),
    };
    println!("cargo:rustc-env=ARCH={}", arch);
    println!("cargo:rerun-if-env-changed=MPV_LIB_DIR");
    println!("cargo:rerun-if-env-changed=MACPORTS_PREFIX");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");

    let mut candidate_dirs: Vec<PathBuf> = Vec::new();
    let mut push_unique_dir = |dir: PathBuf| {
        if dir.exists() && !candidate_dirs.contains(&dir) {
            candidate_dirs.push(dir);
        }
    };

    if let Ok(mpv_lib_dir) = env::var("MPV_LIB_DIR") {
        if !mpv_lib_dir.is_empty() {
            push_unique_dir(PathBuf::from(mpv_lib_dir));
        }
    }
    if let Ok(macports_prefix) = env::var("MACPORTS_PREFIX") {
        if !macports_prefix.is_empty() {
            push_unique_dir(PathBuf::from(format!(
                "{}/lib",
                macports_prefix.trim_end_matches('/')
            )));
        }
    }

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
                        push_unique_dir(PathBuf::from(dir));
                    }
                }
                break;
            }
        }
    }

    for dir in ["/opt/local/lib", "/opt/homebrew/lib", "/usr/local/lib"] {
        push_unique_dir(PathBuf::from(dir));
    }

    let mut found_matching_arch_lib = false;
    let mut fallback_host_dylib: Option<PathBuf> = None;

    for dir in &candidate_dirs {
        if let Some(dylib) = find_mpv_dylib_in_dir(dir) {
            if fallback_host_dylib.is_none() {
                fallback_host_dylib = Some(dylib.clone());
            }
            if required_macho_arch.is_empty() || dylib_has_arch(&dylib, required_macho_arch) {
                println!("cargo:rustc-link-search=native={}", dir.display());
                found_matching_arch_lib = true;
            }
        }
    }

    // If cross-compiling (e.g. building arm64 on an Intel Mac whose /opt/local/lib/libmpv.dylib
    // is x86_64-only), generate a thin arm64 link stub so the linker succeeds without
    // pulling in x86_64-only libraries from /opt/local/lib.
    if !found_matching_arch_lib && !required_macho_arch.is_empty() {
        if let Some(stub_dir) =
            generate_mpv_arch_stub(required_macho_arch, fallback_host_dylib.as_deref())
        {
            println!("cargo:rustc-link-search=native={}", stub_dir.display());
        } else {
            for dir in &candidate_dirs {
                println!("cargo:rustc-link-search=native={}", dir.display());
            }
        }
    }

    // Emit deduplicated runtime library search paths (@rpath)
    let mut rpaths: Vec<String> = vec!["@executable_path/../Frameworks".to_string()];
    for dir in &candidate_dirs {
        let s = dir.display().to_string();
        if !rpaths.contains(&s) {
            rpaths.push(s);
        }
    }
    for default_rpath in ["/opt/homebrew/lib", "/opt/local/lib", "/usr/local/lib"] {
        let s = default_rpath.to_string();
        if !rpaths.contains(&s) {
            rpaths.push(s);
        }
    }
    for rpath in rpaths {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", rpath);
    }

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
