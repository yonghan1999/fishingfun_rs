fn main() {
    // 嵌入 DPI 感知 manifest（docs/06-risks.md R2 对策）。
    // 用链接器 /MANIFESTINPUT 合并，避免依赖 rc.exe。
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let manifest = std::path::Path::new("app.manifest")
            .canonicalize()
            .expect("app.manifest 不存在");
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
        println!("cargo:rerun-if-changed=app.manifest");
    }
}
