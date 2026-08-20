use std::env;
use std::path::PathBuf;
use std::process::Command;

fn zig_target(target: &str) -> &str {
    match target {
        "x86_64-apple-darwin" => "x86_64-macos",
        "aarch64-apple-darwin" => "aarch64-macos",
        other => panic!("OcHerdr currently supports libghostty-vt on macOS only: {other}"),
    }
}

fn main() {
    println!("cargo:rerun-if-changed=native/terminal_shim.c");
    println!("cargo:rerun-if-env-changed=ZIG");
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_OPTIMIZE");

    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = crate_dir.join("../..");
    let vendor = root.join("vendor/libghostty-vt");
    println!(
        "cargo:rerun-if-changed={}",
        vendor.join("VERSION").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        vendor.join("include").display()
    );
    println!("cargo:rerun-if-changed={}", vendor.join("src").display());

    let target = env::var("TARGET").unwrap();
    let zig = env::var("ZIG").unwrap_or_else(|_| {
        let homebrew = "/opt/homebrew/opt/zig@0.15/bin/zig";
        if PathBuf::from(homebrew).is_file() {
            homebrew.into()
        } else {
            "zig".into()
        }
    });
    let optimize = env::var("LIBGHOSTTY_VT_OPTIMIZE").unwrap_or_else(|_| "ReleaseFast".into());
    let version = std::fs::read_to_string(vendor.join("VERSION"))
        .expect("read vendored libghostty-vt VERSION");
    let status = Command::new(zig)
        .current_dir(&vendor)
        .args([
            "build",
            "-Demit-lib-vt",
            &format!("-Doptimize={optimize}"),
            "-Dsimd=true",
            &format!("-Dtarget={}", zig_target(&target)),
            &format!("-Dversion-string={}", version.trim()),
            "-Demit-xcframework=false",
        ])
        .status()
        .expect("execute Zig for libghostty-vt");
    assert!(status.success(), "libghostty-vt Zig build failed: {status}");

    cc::Build::new()
        .file(crate_dir.join("native/terminal_shim.c"))
        .include(vendor.join("include"))
        .define("GHOSTTY_STATIC", None)
        .flag_if_supported("-mmacosx-version-min=11.0")
        .warnings(true)
        .compile("ocherdr-terminal-shim");

    // Zig emits a static archive and a dylib with the same linker name. Copy
    // the archive to an unambiguous name so rustc cannot resolve `-l` to the
    // dylib when it links test binaries or downstream applications.
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let archive_name = "libocherdr-ghostty-vt.a";
    std::fs::copy(
        vendor.join("zig-out/lib/libghostty-vt.a"),
        out_dir.join(archive_name),
    )
    .expect("copy static libghostty-vt archive");
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=ocherdr-ghostty-vt");
}
