//! cachet-api is the HTTP surface, expressed with `http`-crate typed
//! handlers and utoipa derive macros (CLAUDE.md §1, docs/openapi.yaml). The
//! code is the source of truth for the OpenAPI document: `just openapi`
//! regenerates docs/openapi.yaml from these handlers and CI fails on drift.
//! Handlers are platform-free; the worker crate adapts them to workers-rs
//! request and response types.

#![forbid(unsafe_code)]
