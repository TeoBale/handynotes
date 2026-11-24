use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=native/input.m");
    println!("cargo:rerun-if-env-changed=MACOSX_DEPLOYMENT_TARGET");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    let target = env::var("TARGET").expect("Cargo sets TARGET");
    let rustc = env::var_os("RUSTC").expect("Cargo sets RUSTC");
    let deployment = Command::new(rustc)
        .args(["--target", &target, "--print", "deployment-target"])
        .output()
        .expect("query the Rust macOS deployment target");
    assert!(
        deployment.status.success(),
        "query the Rust macOS deployment target"
    );
    let deployment = std::str::from_utf8(&deployment.stdout)
        .expect("deployment target is UTF-8")
        .trim()
        .strip_prefix("MACOSX_DEPLOYMENT_TARGET=")
        .expect("Rust reports MACOSX_DEPLOYMENT_TARGET");
    let object = output.join("input.o");
    let archive = output.join("libhandynotes_input.a");

    let compile = Command::new("xcrun")
        .env("MACOSX_DEPLOYMENT_TARGET", deployment)
        .args(["--sdk", "macosx", "clang", "-target", &target])
        .args([
            "-fobjc-arc",
            "-Wall",
            "-Wextra",
            "-c",
            "native/input.m",
            "-o",
        ])
        .arg(&object)
        .status()
        .expect("run the macOS Objective-C compiler");
    assert!(compile.success(), "compile native/input.m");

    let archive_status = Command::new("xcrun")
        .args(["--sdk", "macosx", "ar", "crs"])
        .arg(&archive)
        .arg(&object)
        .status()
        .expect("run the macOS archiver");
    assert!(
        archive_status.success(),
        "archive native macOS input support"
    );

    println!("cargo:rustc-link-search=native={}", output.display());
    println!("cargo:rustc-link-lib=static=handynotes_input");
    println!("cargo:rustc-link-lib=framework=AppKit");
    println!("cargo:rustc-link-lib=framework=Carbon");
    println!("cargo:rustc-link-lib=objc");
}
