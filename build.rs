// Embeds the icons and version info (wackdown.rc) into the executable.
// wackdown.rc is a template: @MAJOR@/@MINOR@/@PATCH@/@VERSION@ and @ROOT@ are filled in here,
// because rc.exe cannot take quoted string macros on its command line.
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=wackdown.rc");
    println!("cargo:rerun-if-changed=assets/icon/wackdown.ico");
    println!("cargo:rerun-if-changed=assets/icon/document.ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let env = |name: &str| std::env::var(name).unwrap();
    let root = env("CARGO_MANIFEST_DIR").replace('\\', "/");
    let rc = std::fs::read_to_string("wackdown.rc")
        .unwrap()
        .replace("@ROOT@", &root)
        .replace("@MAJOR@", &env("CARGO_PKG_VERSION_MAJOR"))
        .replace("@MINOR@", &env("CARGO_PKG_VERSION_MINOR"))
        .replace("@PATCH@", &env("CARGO_PKG_VERSION_PATCH"))
        .replace("@VERSION@", &env("CARGO_PKG_VERSION"));
    let out = PathBuf::from(env("OUT_DIR")).join("wackdown.rc");
    std::fs::write(&out, rc).unwrap();
    embed_resource::compile(&out, embed_resource::NONE).manifest_optional().unwrap();
}
