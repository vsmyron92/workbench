// rust-embed resolves `../web/dist` when the crate compiles. If the folder does not
// exist yet (fresh checkout, UI not built), debug builds bake in a path that never
// matches and every page answers 503 even after `npm run build`. Create it first.
fn main() {
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../web/dist");
    let _ = std::fs::create_dir_all(&dist);
    println!("cargo:rerun-if-changed=build.rs");
}
