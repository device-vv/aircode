fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=../../assets/windows/aircode.rc");
        println!("cargo:rerun-if-changed=../../assets/icons/aircode.ico");
        let _ = embed_resource::compile("../../assets/windows/aircode.rc", embed_resource::NONE);
    }
}
