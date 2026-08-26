//! The worker's build script. It exists for one line: the build stamp is
//! compile-time input, so cargo has to be told to recompile when it
//! changes or a rebuild at a new commit would serve the previous one.

fn main() {
    println!("cargo:rerun-if-env-changed=CACHET_BUILD_SHA");
}
