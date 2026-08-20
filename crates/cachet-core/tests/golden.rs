//! The golden lane's locked wires (docs/testing/golden.md): the exact
//! bytes every consumer depends on. In-scope now: the cache handshake, the
//! bound constants, the error-code table, the document shapes (lease,
//! generation, narinfo canonical form, fingerprint). Snapshots run with
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
    insta::assert_snapshot!(snap, @"65536 94371800 67108864 1000 4096");
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

#[test]
fn a_scrambled_narinfo_emits_the_canonical_form() {
    let scrambled = "X-Custom-Facts: kept because a cache is not an authority\nNarSize: 42\nReferences: cccccccccccccccccccccccccccccccc-dep-3 aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bash-5.2\nURL: nar/xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx.nar.zst\nStorePath: /nix/store/0123456789abcdfghijklmnpqrsvwxyz-bash-5.2\nSig: example:mhQ=\nCompression: zstd\nNarHash: sha256:0iqi00iqi00iqi00iqi00iqi00iqi00iqi00iqi00iqi00iqi00j\n";
    let parsed = Narinfo::parse(scrambled).expect("the fixture parses");
    // References sort, the unknown line keeps its place at the end, and the
    // trailing line breaks are exact.
    insta::assert_snapshot!(parsed.fingerprint(), @"1;/nix/store/0123456789abcdfghijklmnpqrsvwxyz-bash-5.2;sha256:0iqi00iqi00iqi00iqi00iqi00iqi00iqi00iqi00iqi00iqi00j;42;aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bash-5.2,cccccccccccccccccccccccccccccccc-dep-3");
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
