fn main() {
    #[cfg(feature = "desktop")]
    {
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
            use std::{path::PathBuf, process::Command};
            println!("cargo:rerun-if-changed=src/package_picker.m");
            let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
            let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
                Ok("aarch64") => "arm64",
                Ok("x86_64") => "x86_64",
                _ => panic!("Unsupported macOS architecture"),
            };
            let compiled = Command::new("clang")
                .args([
                    "-arch",
                    arch,
                    "-fobjc-arc",
                    "-fblocks",
                    "-Wall",
                    "-Wextra",
                    "-Werror",
                    "-mmacosx-version-min=15.4",
                    "-c",
                    "src/package_picker.m",
                    "-o",
                ])
                .arg(out.join("package_picker.o"))
                .status()
                .expect("Compile native package picker");
            assert!(
                compiled.success(),
                "Native package picker compilation failed"
            );
            let archived = Command::new("ar")
                .arg("rcs")
                .arg(out.join("libpackage_picker.a"))
                .arg(out.join("package_picker.o"))
                .status()
                .expect("Archive native package picker");
            assert!(archived.success(), "Native package picker archive failed");
            println!("cargo:rustc-link-search=native={}", out.display());
            println!("cargo:rustc-link-lib=static=package_picker");
            println!("cargo:rustc-link-lib=framework=AppKit");
            println!("cargo:rustc-link-lib=framework=UniformTypeIdentifiers");
        }
        tauri_build::build();
    }
}
