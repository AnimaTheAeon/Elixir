fn main() {
    // The Windows app's icon and version info.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=windows/elixir.rc");
        println!("cargo:rerun-if-changed=windows/elixir.ico");
        let _ = embed_resource::compile("windows/elixir.rc", embed_resource::NONE).manifest_optional();
    }
}
