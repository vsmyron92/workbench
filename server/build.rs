// rust-embed resolves `../web/dist` when the crate compiles. If the folder does not
// exist yet (fresh checkout, UI not built), debug builds bake in a path that never
// matches and every page answers 503 even after `npm run build`. Create it first.
fn main() {
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../web/dist");
    let _ = std::fs::create_dir_all(&dist);
    println!("cargo:rerun-if-changed=build.rs");
    windows_resource();
}

/// Windows: the icon (drawn by `web/scripts/icons.mjs`) and the version resource of
/// workbench.exe and workbenchw.exe, which Explorer, the Start Menu shortcut and Task
/// Manager show. Without a resource compiler the build goes on without them.
fn windows_resource() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let icon = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../web/public/icons/workbench.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    let mut res = winresource::WindowsResource::new();
    res.set_icon(&icon.to_string_lossy()).set("ProductName", "Workbench").set("FileDescription", "Workbench");
    if let Err(e) = res.compile() {
        println!("cargo:warning=the Windows executables get no icon or version resource: {e}");
    }
}
