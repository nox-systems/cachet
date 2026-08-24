//! The golden lane's locked wires (docs/testing/golden.md): the exact
//! bytes every consumer depends on. In-scope now: the cache handshake, the
//! bound constants, the error-code table, the problem document bodies,
//! the read path's headers, and the document shapes (lease, generation,
//! narinfo canonical form, fingerprint). Snapshots run with
//! INSTA_UPDATE=no in the gate, so an intentional change updates the snap
//! file in the same commit.

use cachet_core::constants::{
    MULTIPART_PARTS_MAX, NARINFO_BYTES_MAX, NIX_BASE32_ALPHABET, NIX_CACHE_INFO, PUSH_PATHS_MAX,
    UPLOAD_PART_BYTES, UPLOAD_SINGLE_MAX_BYTES,
};
use cachet_core::error::ClientError;
use cachet_core::generation::GenerationDocument;
use cachet_core::lease::LeaseDocument;
use cachet_core::narinfo::Narinfo;

#[test]
fn nix_cache_info_body_is_exact() {
    insta::assert_snapshot!(NIX_CACHE_INFO, @"StoreDir: /nix/store\nWantMassQuery: 1\nPriority: 30\n");
}

#[test]
fn bound_constants_are_the_contract() {
    let snap = format!(
        "{NARINFO_BYTES_MAX} {UPLOAD_SINGLE_MAX_BYTES} {UPLOAD_PART_BYTES} {MULTIPART_PARTS_MAX} {PUSH_PATHS_MAX}"
    );
    insta::assert_snapshot!(snap, @"65536 94371800 67108864 1000 16384");
}

#[test]
fn nix_base32_alphabet_matches_nix() {
    assert_eq!(NIX_BASE32_ALPHABET, "0123456789abcdfghijklmnpqrsvwxyz");
    assert_eq!(NIX_BASE32_ALPHABET.len(), 32);
    for dropped in [b'e', b'o', b'u', b't'] {
        assert!(
            !NIX_BASE32_ALPHABET.contains(char::from(dropped)),
            "the nix alphabet drops vowels e, o, u, and t"
        );
    }
}

// The whole code → status table, locked: adding a case is a deliberate
// contract change, and previous statuses never move.
#[test]
fn the_error_code_table_is_locked() {
    let table = [
        ClientError::MalformedKey,
        ClientError::MalformedNarinfo,
        ClientError::MalformedRoots,
        ClientError::MalformedAuth,
        ClientError::PartNumberInvalid,
        ClientError::PartSizeMismatch,
        ClientError::CompletePartsMismatch,
        ClientError::UnsupportedCompression,
        ClientError::StorePathMismatch,
        ClientError::FileHashMismatch,
        ClientError::NarHashMismatch,
        ClientError::Unauthorized,
        ClientError::ForbiddenOrg,
        ClientError::ForbiddenRef,
        ClientError::ForbiddenProject,
        ClientError::NotFound,
        ClientError::UploadUnknown,
        ClientError::NarinfoNarMissing,
        ClientError::LengthRequired,
        ClientError::BodyTooLarge,
        ClientError::AuthUnavailable,
        ClientError::StorageUnavailable,
        ClientError::MalformedOauth,
        ClientError::OauthStateUnknown,
        ClientError::ForbiddenAdmin,
    ];
    let body = table
        .iter()
        .map(|code| format!("{} {}", code.code(), code.status()))
        .collect::<Vec<_>>()
        .join("\n");
    insta::assert_snapshot!(body, @"
    malformed_key 400
    malformed_narinfo 400
    malformed_roots 400
    malformed_auth 400
    part_number_invalid 400
    part_size_mismatch 400
    complete_parts_mismatch 400
    unsupported_compression 400
    store_path_mismatch 400
    file_hash_mismatch 400
    nar_hash_mismatch 400
    unauthorized 401
    forbidden_org 403
    forbidden_ref 403
    forbidden_project 403
    not_found 404
    upload_unknown 404
    narinfo_nar_missing 409
    length_required 411
    body_too_large 413
    auth_unavailable 503
    storage_unavailable 503
    malformed_oauth 400
    oauth_state_unknown 401
    forbidden_admin 403
    ");
}

#[test]
fn lease_document_writes_in_the_locked_shape() {
    let document = LeaseDocument::parse(
        &serde_json::json!({
            "project": "my-org-my-repo",
            "renewedAtMs": 1_780_000_000_000_u64,
            "repository": "my-org/my-repo",
            "ref": "refs/heads/main",
            "runId": "12345678901",
            "commitSha": "0123456789abcdef0123456789abcdef01234567",
            "installables": [".#devShells.aarch64-darwin.default"],
            "storePaths": ["/nix/store/0123456789abcdfghijklmnpqrsvwxyz-bash-5.2"],
        })
        .to_string(),
    )
    .expect("the fixture parses");
    insta::assert_snapshot!(document.serialize(), @r#"
    {
      "project": "my-org-my-repo",
      "renewedAtMs": 1780000000000,
      "repository": "my-org/my-repo",
      "ref": "refs/heads/main",
      "runId": "12345678901",
      "commitSha": "0123456789abcdef0123456789abcdef01234567",
      "installables": [
        ".#devShells.aarch64-darwin.default"
      ],
      "storePaths": [
        "/nix/store/0123456789abcdfghijklmnpqrsvwxyz-bash-5.2"
      ]
    }
    "#);
}

#[test]
fn the_generation_document_writes_compactly() {
    let document = GenerationDocument {
        generation: 7,
        bumped_at_ms: 1_780_000_000_000,
    };
    insta::assert_snapshot!(document.serialize(), @"{\"generation\":7,\"bumpedAtMs\":1780000000000}\n");
}

// The collector's documents: the report every run lands and the cursor a
// parked run leaves, both byte-locked because runs and ticks read each
// other's bytes.
#[test]
fn the_gc_documents_write_stably() {
    use cachet_core::gc::{GcCursor, GcReport, GcStage, SweepCursor};
    let report = GcReport {
        run_id: "1780000000000-00ff112233445566".to_string(),
        started_at_ms: 1_780_000_000_000,
        finished_at_ms: 1_780_000_000_500,
        inventory_paths: 12,
        active_leases: 2,
        marked_paths: 9,
        unreadable_deep: 1,
        narinfos_deleted: 3,
        nars_deleted: 2,
        bytes_freed: 486_400,
        uploads_aborted: 1,
        gate: None,
    };
    insta::assert_snapshot!(report.serialize(), @"
    {
      \"runId\": \"1780000000000-00ff112233445566\",
      \"startedAtMs\": 1780000000000,
      \"finishedAtMs\": 1780000000500,
      \"inventoryPaths\": 12,
      \"activeLeases\": 2,
      \"markedPaths\": 9,
      \"unreadableDeep\": 1,
      \"narinfosDeleted\": 3,
      \"narsDeleted\": 2,
      \"bytesFreed\": 486400,
      \"uploadsAborted\": 1,
      \"gate\": null
    }
    ");

    let cursor = GcCursor {
        run_id: report.run_id.clone(),
        started_at_ms: report.started_at_ms,
        stage: GcStage::Sweep,
        inventory_paths: 12,
        active_leases: 2,
        marked_paths: 9,
        unreadable_deep: 1,
        mark: None,
        collect: None,
        sweep: Some(SweepCursor {
            narinfo_deletes: vec!["aa".to_string()],
            nar_deletes: vec!["nar/bb".to_string()],
            narinfos_deleted: 1,
            nars_deleted: 0,
            bytes_freed: 1024,
            bytes_by_key: [("aa".to_string(), 1024_u64)].into_iter().collect(),
        }),
        uploads_aborted: 0,
    };
    insta::assert_snapshot!(cursor.serialize(), @"
    {
      \"runId\": \"1780000000000-00ff112233445566\",
      \"startedAtMs\": 1780000000000,
      \"stage\": \"sweep\",
      \"inventoryPaths\": 12,
      \"activeLeases\": 2,
      \"markedPaths\": 9,
      \"unreadableDeep\": 1,
      \"sweep\": {
        \"narinfoDeletes\": [
          \"aa\"
        ],
        \"narDeletes\": [
          \"nar/bb\"
        ],
        \"narinfosDeleted\": 1,
        \"narsDeleted\": 0,
        \"bytesFreed\": 1024,
        \"bytesByKey\": {
          \"aa\": 1024
        }
      },
      \"uploadsAborted\": 0
    }
    ");
}

#[test]
fn a_scrambled_narinfo_emits_the_canonical_form() {
    let scrambled = "X-Custom-Facts: kept because a cache is not an authority\nNarSize: 42\nReferences: cccccccccccccccccccccccccccccccc-dep-3 aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bash-5.2\nURL: nar/xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx.nar.zst\nStorePath: /nix/store/0123456789abcdfghijklmnpqrsvwxyz-bash-5.2\nSig: example:mhQ=\nCompression: zstd\nNarHash: sha256:0iqi00iqi00iqi00iqi00iqi00iqi00iqi00iqi00iqi00iqi00j\n";
    let parsed = Narinfo::parse(scrambled).expect("the fixture parses");
    // References sort, the unknown line keeps its place at the end, and the
    // trailing line breaks are exact.
    insta::assert_snapshot!(parsed.fingerprint(), @"1;/nix/store/0123456789abcdfghijklmnpqrsvwxyz-bash-5.2;sha256:0iqi00iqi00iqi00iqi00iqi00iqi00iqi00iqi00iqi00iqi00j;42;/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bash-5.2,/nix/store/cccccccccccccccccccccccccccccccc-dep-3");
    insta::assert_snapshot!(parsed.serialize(), @"
    StorePath: /nix/store/0123456789abcdfghijklmnpqrsvwxyz-bash-5.2
    URL: nar/xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx.nar.zst
    Compression: zstd
    NarHash: sha256:0iqi00iqi00iqi00iqi00iqi00iqi00iqi00iqi00iqi00iqi00j
    NarSize: 42
    References: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bash-5.2 cccccccccccccccccccccccccccccccc-dep-3
    Sig: example:mhQ=
    X-Custom-Facts: kept because a cache is not an authority
    ");
}

// The problem document is what a client matches on; its field order,
// field set, and trailing newline are the contract.
#[test]
fn problem_bodies_are_byte_locked() {
    insta::assert_snapshot!(
        cachet_core::problem::problem_body(ClientError::MalformedKey),
        @"{\"type\":\"about:blank\",\"status\":400,\"title\":\"key grammar rejected\",\"code\":\"malformed_key\"}\n"
    );
    insta::assert_snapshot!(
        cachet_core::problem::problem_body(ClientError::AuthUnavailable),
        @"{\"type\":\"about:blank\",\"status\":503,\"title\":\"authentication backend unavailable\",\"code\":\"auth_unavailable\"}\n"
    );
}

// The read path's header outputs: the exact strings a cache and a nix
// client see for each response class.
#[test]
fn read_response_headers_are_byte_locked() {
    use std::fmt::Write as _;
    let mut dump = String::new();
    for (kind, size) in [
        (cachet_core::read::ObjectKind::Narinfo, 1234_u64),
        (cachet_core::read::ObjectKind::Nar, 9_876_543),
    ] {
        for (name, value) in cachet_core::read::object_response_headers(kind, size) {
            writeln!(dump, "{name}: {value}").expect("writing a string");
        }
    }
    for headers in [
        cachet_core::read::not_found_response_headers(),
        cachet_core::read::cache_info_response_headers(),
        cachet_core::read::generation_response_headers(),
    ] {
        for (name, value) in headers {
            writeln!(dump, "{name}: {value}").expect("writing a string");
        }
    }
    insta::assert_snapshot!(dump, @"
    content-type: text/x-nix-narinfo
    content-length: 1234
    cache-control: public, max-age=2592000, immutable
    content-type: application/x-nix-nar
    content-length: 9876543
    cache-control: public, max-age=2592000, immutable
    content-type: text/plain; charset=utf-8
    cache-control: public, max-age=30
    content-type: text/x-nix-cache-info
    cache-control: public, max-age=300
    content-type: application/json
    cache-control: public, max-age=60
    ");
}
