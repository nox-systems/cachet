//! The release-level facts every cachet deployment shares.
//!
//! These were the literals in `infra/alchemy.run.ts`. They are not policy
//! an operator tunes: two deployments on two compatibility dates are two
//! different runtimes, and the collector's schedule is the same everywhere
//! because its work is idempotent and bounded (ADR 0019).

/// The product this grammar deploys.
pub const PRODUCT: &str = "cachet";

/// The manifest schema version the deployer must agree with. The
/// placeholder set is closed, and this guards the two sides against
/// drifting apart.
pub const MANIFEST_VERSION: u32 = 1;

/// The platform semantics every deployment pins.
pub const COMPATIBILITY_DATE: &str = "2026-05-01";

/// The compatibility flags. alchemy added `nodejs_compat` on its own for a
/// worker with a compatibility date past 2024-09-23, so today's deployed
/// script carries it without the program naming it; a deploy that uploads
/// through the REST API has to state it, or the runtime changes.
pub const COMPATIBILITY_FLAGS: &[&str] = &["nodejs_compat"];

/// The collector's schedule (ADR 0019). Named once, so the trigger and the
/// `CACHET_GC_CRON` var the console counts down against cannot disagree.
pub const GC_CRON: &str = "0 * * * *";

/// The CPU ceiling in milliseconds. Five minutes covers the one request
/// whose cost scales with the object, a multipart completion measuring the
/// NAR its parts assembled (docs/DEPLOY.md).
pub const CPU_MS: u32 = 300_000;

/// Observability on: without it the worker's own read events go nowhere.
pub const OBSERVABILITY: bool = true;

/// The entry module worker-build emits.
pub const MAIN_MODULE: &str = "index.js";

/// What a module is, which decides the content type it uploads under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModuleKind {
    /// An ES module: `application/javascript+module`.
    EsModule,
    /// A compiled wasm module: `application/wasm`.
    CompiledWasm,
}

/// Every module the bundle uploads, named as a POSIX path relative to the
/// entry. `index.js` imports `./index_bg.wasm` by exactly that name, and
/// `worker/shim.mjs` is worker-build's back-compatibility re-export, which
/// the current deploy uploads and so this one does too.
pub const MODULES: &[(&str, ModuleKind)] = &[
    (MAIN_MODULE, ModuleKind::EsModule),
    ("index_bg.wasm", ModuleKind::CompiledWasm),
    ("worker/shim.mjs", ModuleKind::EsModule),
];

/// Where the console build lands, relative to the repository root.
pub const ASSETS_DIRECTORY: &str = "web/dist";

/// The path the console is served under, and the path its root redirects
/// to (ADR 0014).
pub const CONSOLE_PATH: &str = "/console";

/// The asset layer's handling modes, both `none`. Anything else would let
/// the layer answer a cache miss with the console's shell, and nix reads
/// "does this cache hold it" from the status (ADR 0014).
pub const ASSETS_HTML_HANDLING: &str = "none";

/// See [`ASSETS_HTML_HANDLING`].
pub const ASSETS_NOT_FOUND_HANDLING: &str = "none";

/// The callback path a deployment's GitHub OAuth App must name, fixed by
/// the host (`cachet-core/src/oauth.rs`).
pub const OAUTH_CALLBACK_PATH: &str = "/_auth/callback";
