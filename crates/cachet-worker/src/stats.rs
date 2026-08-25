//! Writing what happened to the deployment's dataset.
//!
//! One function, because every counted thing should be counted the same
//! way: the schema is cachet-core's (`stats::StatPoint`), this is only
//! the binding and the write. Nothing here can fail a request. A
//! deployment with no dataset bound counts nothing and serves exactly as
//! well, which is what lets an older deployment upgrade its worker
//! without its bindings.

use cachet_core::constants::STATS_BINDING;
use cachet_core::stats::StatPoint;
use worker::{AnalyticsEngineDataPointBuilder, Env};

use crate::log;

/// Count one thing.
///
/// The event is the dataset's index, which is also its sampling key, so
/// it is deliberately coarse: reads, writes, probes, collections, and
/// credentials, rather than one key per store path.
pub fn emit(env: &Env, point: &StatPoint) {
    let Ok(dataset) = env.analytics_engine(STATS_BINDING) else {
        return;
    };
    let mut builder = AnalyticsEngineDataPointBuilder::new().indexes([point.event.name()]);
    for blob in point.blobs() {
        builder = builder.add_blob(blob);
    }
    for double in point.doubles() {
        builder = builder.add_double(double);
    }
    if let Err(failure) = dataset.write_data_point(&builder.build()) {
        // A statistic is never worth failing the thing it counts.
        log::event(
            "warn",
            "stats.write_failed",
            &[
                ("event", point.event.name().to_string()),
                ("error", failure.to_string()),
            ],
        );
    }
}
