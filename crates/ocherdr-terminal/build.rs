use std::env;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=GHOSTTYKIT_XCFRAMEWORK");

    let target = env::var("TARGET").expect("Cargo must provide TARGET");
    assert!(
        target.ends_with("apple-darwin"),
        "OcHerdr's native Ghostty renderer currently supports macOS only: {target}"
    );

    let crate_directory = PathBuf::from(
        env::var("CARGO_MANIFEST_DIR").expect("Cargo must provide CARGO_MANIFEST_DIR"),
    );
    let repository_root = crate_directory.join("../..");
    let xcframework = env::var_os("GHOSTTYKIT_XCFRAMEWORK")
        .map(PathBuf::from)
        .unwrap_or_else(|| repository_root.join("vendor/ghosttykit/GhosttyKit.xcframework"));
    let macos_slice = xcframework.join("macos-arm64_x86_64");
    let header = macos_slice.join("Headers/ghostty.h");
    let archive = macos_slice.join("ghostty-internal.a");

    require_file(&header, &repository_root);
    require_file(&archive, &repository_root);
    println!("cargo:rerun-if-changed={}", header.display());
    println!("cargo:rerun-if-changed={}", archive.display());

    let out_directory = PathBuf::from(env::var("OUT_DIR").expect("Cargo must provide OUT_DIR"));
    let bindings = bindgen::Builder::default()
        .header(header.to_string_lossy())
        .allowlist_function("ghostty_.*")
        .allowlist_type("ghostty_.*")
        .allowlist_var("GHOSTTY_.*")
        .clang_arg("-DGHOSTTY_STATIC")
        .derive_default(false)
        .generate_comments(false)
        .layout_tests(false)
        .generate()
        .expect("generate GhosttyKit bindings");
    bindings
        .write_to_file(out_directory.join("ghostty_bindings.rs"))
        .expect("write GhosttyKit bindings");

    let linked_archive = out_directory.join("libocherdr-ghostty.a");
    std::fs::copy(&archive, &linked_archive).expect("copy GhosttyKit static archive");
    println!("cargo:rustc-link-search=native={}", out_directory.display());
    println!("cargo:rustc-link-lib=static=ocherdr-ghostty");
    println!("cargo:rustc-link-lib=c++");
    for framework in [
        "ApplicationServices",
        "Carbon",
        "Foundation",
        "IOSurface",
        "Metal",
        "QuartzCore",
        "UniformTypeIdentifiers",
    ] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}

fn require_file(path: &Path, repository_root: &Path) {
    assert!(
        path.is_file(),
        "missing GhosttyKit artifact at {}; run {}/scripts/bootstrap-ghosttykit.sh",
        path.display(),
        repository_root.display()
    );
}
