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
    // The unrar library asks to link pthread, which on Android is part of libc and has no library
    // of its own, so the linker gets an empty one to find.
    if target.contains("android") {
        let out = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
        std::fs::write(std::path::Path::new(&out).join("libpthread.a"), "!<arch>\n").expect("write libpthread.a");
        println!("cargo:rustc-link-search=native={out}");
        // Its C++ needs the C++ library, which Android phones do not provide: built in, so the app
        // can load without shipping another file. (Link arguments, so they follow unrar itself.)
        println!("cargo:rustc-link-arg=-lc++_static");
        println!("cargo:rustc-link-arg=-lc++abi");
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
