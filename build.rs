// ONNX Runtime (linked statically for the Gallery's smart features) uses clang's
// `__isPlatformVersionAtLeast` on macOS, which lives in clang's runtime library; Rust does not
// link that by default, so point the linker at it.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target = std::env::var("TARGET").unwrap_or_default();
    // Smart search and faces need ONNX Runtime, which has no prebuilt library for Android or
    // Intel Macs; those builds leave the features out (keep in step with Cargo.toml).
    println!("cargo::rustc-check-cfg=cfg(ownnas_ml)");
    if !target.contains("android") && target != "x86_64-apple-darwin" {
        println!("cargo:rustc-cfg=ownnas_ml");
    }
    if !target.ends_with("apple-darwin") {
        return;
    }
    let output = std::process::Command::new("xcrun").args(["clang", "--print-runtime-dir"]).output();
    if let Ok(output) = output {
        let dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !dir.is_empty() && std::path::Path::new(&dir).join("libclang_rt.osx.a").is_file() {
            // A link argument (not `rustc-link-lib`) so the binary gets it too, not only the library.
            println!("cargo:rustc-link-arg={dir}/libclang_rt.osx.a");
        }
    }
}
