fn main() {
    // 注意：不嵌自定义 manifest——Tauri 已自带资源与 manifest，
    // 且 tao 运行时会调用 SetProcessDpiAwarenessContext 设置 DPI 感知，
    // 重复 MANIFEST 资源会导致链接失败（CVT1100/LNK1123）。
    tauri_build::build()
}
