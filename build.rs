//! Gives the .exe its icon (the same logo the app draws in code).

#[path = "src/icon.rs"]
mod icon;

fn main() {
    println!("cargo:rerun-if-changed=src/icon.rs");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let ico = out.join("app.ico");
    std::fs::write(&ico, icon::ico()).unwrap();
    let rc = out.join("app.rc");
    let path = ico.display().to_string().replace('\\', "/");
    std::fs::write(&rc, format!("1 ICON \"{path}\"\n")).unwrap();
    // an icon is nice to have, never worth failing the build over
    let _ = embed_resource::compile(&rc, embed_resource::NONE).manifest_optional();
}
