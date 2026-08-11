use std::{env, path::PathBuf};

fn main() {
    let manifest =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir")).join("app.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-link-arg-bin=codex-pet-native=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bin=codex-pet-native=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }
}
