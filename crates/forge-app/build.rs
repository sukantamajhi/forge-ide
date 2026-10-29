//! Embeds the Forge icon in the Windows executable (file icon, taskbar, window).

fn main() {
    println!("cargo:rerun-if-changed=forge.rc");
    println!("cargo:rerun-if-changed=../../assets/forge.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
        embed_resource::compile("forge.rc", embed_resource::NONE).manifest_required().unwrap();
    }
}
