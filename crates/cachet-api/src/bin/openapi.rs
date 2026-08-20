//! The OpenAPI document generator: `just openapi` runs this binary to
//! rewrite docs/openapi.yaml from the route descriptors, and
//! `just openapi-check` regenerates through it and diffs.

#![forbid(unsafe_code)]

use utoipa::OpenApi as _;

fn main() {
    let yaml = cachet_api::ApiDoc::openapi()
        .to_yaml()
        .expect("the derived document always serializes");
    print!("{yaml}");
}
