// Ensure web/dist exists so the crate compiles before the frontend is built.
// Real releases run `npm run build` in web/ first; see .github/workflows/release.yml.
fn main() {
    let dist = std::path::Path::new("web/dist");
    let index = dist.join("index.html");
    if !index.exists() {
        std::fs::create_dir_all(dist).expect("create web/dist");
        std::fs::write(
            &index,
            "<!doctype html><title>Receipts</title><p>Frontend not built. Run <code>npm run build</code> in <code>web/</code>, then rebuild.</p>",
        )
        .expect("write placeholder index.html");
    }
    println!("cargo:rerun-if-changed=web/dist");
    // The self-updater downloads the release archive that matches this target triple.
    println!("cargo:rustc-env=RECEIPTS_TARGET={}", std::env::var("TARGET").unwrap());
}
