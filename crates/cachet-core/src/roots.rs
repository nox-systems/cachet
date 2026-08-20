//! Lease renewal and lease reads (CLAUDE.md §1, §9). `POST /roots/{project}`
//! is the one route where the branch a token came from matters: every
//! branch may push store paths, because a teammate's checkout should be
//! fast, but only the default branch may keep a closure alive, or a
//! long-lived experiment branch would pin its dependencies forever. The
//! worker enforces the branch rule here, where the answer is decided,
//! because the action checks its own ref only as a convenience.
//!
//! Two more gates sit beside it (ADR 0017): the project a renewal names
//! must equal the token's repository claim, so one repo's CI can never
//! overwrite another repo's lease, and provenance comes only from the
//! verified token, never from the body.

use crate::auth::{OidcConfig, OidcIdentity, require_default_branch, require_project_binding};
use crate::constants::ROOTS_PROJECTS_MAX;
use crate::error::{ClientError, Result};
use crate::lease::LeaseDocument;
use crate::roots_payload::parse_roots_payload;
use crate::types::{ProjectName, UnixMillis};

/// Build the lease a renewal should write. The branch gate and the
/// project binding run before the body is even parsed: a request that may
/// not renew should not have its bytes examined.
///
/// # Errors
///
/// [`ClientError::ForbiddenRef`] off the default branch,
/// [`ClientError::ForbiddenProject`] when the project is not the token's
/// repository, [`ClientError::MalformedRoots`] on an unparsable payload,
/// [`ClientError::BodyTooLarge`] on an over-cap payload.
pub fn build_lease_renewal(
    project: &ProjectName,
    body_text: &str,
    identity: &OidcIdentity,
    config: &OidcConfig,
    now: UnixMillis,
) -> Result<LeaseDocument> {
    require_default_branch(identity, config)?;
    require_project_binding(identity, project)?;
    let payload = parse_roots_payload(body_text)?;
    Ok(LeaseDocument {
        project: project.as_str().to_string(),
        renewed_at_ms: now.as_u64(),
        repository: identity.repository.clone(),
        ref_: identity.ref_.clone(),
        run_id: identity.run_id.clone(),
        commit_sha: identity.sha.clone(),
        installables: payload.installables,
        store_paths: payload.store_paths,
    })
}

/// Bound a listing of project names. `GET /roots` is a read, so it carries
/// the whole answer or nothing: a bucket holding more leases than the cap
/// is a state to report, because a partial listing would make a reviewer
/// sample from a subset they could not know was incomplete.
///
/// # Errors
///
/// [`ClientError::BodyTooLarge`] when the listing exceeds
/// [`ROOTS_PROJECTS_MAX`].
pub fn bound_project_list(projects: &[ProjectName]) -> Result<()> {
    if projects.len() > ROOTS_PROJECTS_MAX {
        return Err(ClientError::BodyTooLarge);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> OidcConfig {
        OidcConfig {
            orgs: vec!["lane-org".to_string()],
            audience: "cachet-test".to_string(),
            default_branch_ref: "refs/heads/main".to_string(),
        }
    }

    fn identity() -> OidcIdentity {
        OidcIdentity {
            repository: "lane-org/lane-repo".to_string(),
            repository_owner: "lane-org".to_string(),
            ref_: "refs/heads/main".to_string(),
            run_id: "41".to_string(),
            sha: "abc".to_string(),
        }
    }

    fn payload() -> String {
        serde_json::json!({
            "installables": [".#devShells.aarch64-darwin.default"],
            "storePaths": ["/nix/store/0123456789abcdfghijklmnpqrsvwxyz-bash-5.2"],
        })
        .to_string()
    }

    #[test]
    fn the_renewal_carries_only_verified_provenance() {
        let lease = build_lease_renewal(
            &ProjectName::parse("lane-org-lane-repo").expect("valid"),
            &payload(),
            &identity(),
            &config(),
            UnixMillis::new(1_780_000_000_000),
        )
        .expect("the contract builds");
        assert_eq!(lease.repository, "lane-org/lane-repo");
        assert_eq!(lease.ref_, "refs/heads/main");
        assert_eq!(lease.renewed_at_ms, 1_780_000_000_000);
    }

    #[test]
    fn off_branch_renewals_are_forbidden_ref() {
        let mut stray = identity();
        stray.ref_ = "refs/heads/feature".to_string();
        assert_eq!(
            build_lease_renewal(
                &ProjectName::parse("lane-org-lane-repo").expect("valid"),
                &payload(),
                &stray,
                &config(),
                UnixMillis::new(1),
            ),
            Err(ClientError::ForbiddenRef)
        );
    }

    // why: ADR 0017. The previous worker had none of this check: any CI in
    // the org could overwrite any project's lease; the binding makes a
    // lease's project equal to the token's repository, verified.
    #[test]
    fn cross_repo_renewals_are_forbidden_project() {
        assert_eq!(
            build_lease_renewal(
                &ProjectName::parse("lane-org-their-repo").expect("valid"),
                &payload(),
                &identity(),
                &config(),
                UnixMillis::new(1),
            ),
            Err(ClientError::ForbiddenProject)
        );
    }

    #[test]
    fn the_branch_gate_runs_before_the_body() {
        let mut stray = identity();
        stray.ref_ = "refs/heads/feature".to_string();
        assert_eq!(
            build_lease_renewal(
                &ProjectName::parse("lane-org-lane-repo").expect("valid"),
                "not json at all",
                &stray,
                &config(),
                UnixMillis::new(1),
            ),
            Err(ClientError::ForbiddenRef),
            "a bad body behind a bad branch is reported as the branch"
        );
    }

    #[test]
    fn the_listing_bound_is_a_total_answer_or_nothing() {
        let within: Vec<ProjectName> = (0..4)
            .map(|i| ProjectName::parse(&format!("p{i}")).expect("valid"))
            .collect();
        assert!(bound_project_list(&within).is_ok());
        let too_many: Vec<ProjectName> = (0..=ROOTS_PROJECTS_MAX)
            .map(|i| ProjectName::parse(&format!("p{i}")).expect("valid"))
            .collect();
        assert_eq!(
            bound_project_list(&too_many),
            Err(ClientError::BodyTooLarge)
        );
    }
}
