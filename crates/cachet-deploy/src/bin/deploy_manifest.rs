//! The manifest emitter: `just deploy-manifest` runs this binary to
//! rewrite deploy-manifest.json from the roster and the release constants,
//! and `just deploy-manifest-check` regenerates through it and diffs.

#![forbid(unsafe_code)]

fn main() {
    let manifest = cachet_deploy::Manifest::template(env!("CARGO_PKG_VERSION"));
    let json = serde_json::to_string_pretty(&manifest).expect("the template always serializes");
    println!("{json}");
}
