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
/// Manager show, and their application manifest (`packaging/windows/workbench.manifest`:
/// Windows 10 and 11, Common Controls 6 for message boxes, long paths, never elevated).
/// They go into every executable of the package, the tests' too. Without a resource
/// compiler the build goes on without them.
fn windows_resource() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../packaging/windows");
    let (icon, manifest) = (dir.join("workbench.ico"), dir.join("workbench.manifest"));
    for f in [&icon, &manifest] {
        println!("cargo:rerun-if-changed={}", f.display());
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon(&icon.to_string_lossy())
        .set_manifest_file(&manifest.to_string_lossy())
        .set("ProductName", "Workbench")
        .set("FileDescription", "Workbench");
    if let Err(e) = res.compile() {
        println!("cargo:warning=the Windows executables get no icon, version resource or manifest: {e}");
    }
}
