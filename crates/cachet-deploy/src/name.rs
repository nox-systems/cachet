//! The name grammar: a deployment's name in, every derived name out.
//!
//! Pure by construction. No I/O, no clock, no ambient environment read,
//! which is CLAUDE.md §3's seam law applied to deployment configuration:
//! the same name produces the same resource names on any machine, so a
//! plan is reviewable before it runs.

use core::fmt;

use crate::error::DeployError;

/// The shortest name: a letter and one more character.
const NAME_MIN: usize = 2;

/// The longest name. The name becomes the R2 bucket, the KV title, and the
/// worker name, prefixed when absent, so it fits the strictest of those
/// grammars (ADR 0011).
const NAME_MAX: usize = 32;

/// The prefix every resource name carries.
pub const PREFIX: &str = "cachet-";

/// One deployment's name, and everything it derives.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Deployment {
    name: String,
}

impl Deployment {
    /// Validates a name against `^[a-z][a-z0-9-]{1,31}$`, the grammar
    /// `infra/src/config.ts` has enforced since ADR 0011.
    ///
    /// # Errors
    /// [`DeployError::BadName`] for anything outside it.
    ///
    /// # Examples
    /// ```
    /// use cachet_deploy::Deployment;
    /// let production = Deployment::new("production")?;
    /// assert_eq!(production.resource_name(), "cachet-production");
    /// // A name already carrying the prefix is used as it is, so no
    /// // deployment ever reads cachet-cachet-*.
    /// let staging = Deployment::new("cachet-staging")?;
    /// assert_eq!(staging.resource_name(), "cachet-staging");
    /// assert!(Deployment::new("Production").is_err());
    /// # Ok::<(), cachet_deploy::DeployError>(())
    /// ```
    pub fn new(name: &str) -> Result<Self, DeployError> {
        if !(NAME_MIN..=NAME_MAX).contains(&name.len()) {
            return Err(DeployError::BadName);
        }
        let mut bytes = name.bytes();
        let Some(first) = bytes.next() else {
            return Err(DeployError::BadName);
        };
        if !first.is_ascii_lowercase() {
            return Err(DeployError::BadName);
        }
        if !bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-') {
            return Err(DeployError::BadName);
        }
        Ok(Self {
            name: name.to_owned(),
        })
    }

    /// The name as the operator wrote it. This is what `CACHET_DEPLOY_NAME`
    /// carries and what the console prints.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The name every account resource is created under: `cachet-<name>`,
    /// or the name as it is when it already starts with the prefix.
    #[must_use]
    pub fn resource_name(&self) -> String {
        if self.name.starts_with(PREFIX) {
            self.name.clone()
        } else {
            format!("{PREFIX}{}", self.name)
        }
    }

    /// The worker's name, which is the resource name.
    #[must_use]
    pub fn worker_name(&self) -> String {
        self.resource_name()
    }

    /// The R2 bucket's name, which is the resource name.
    #[must_use]
    pub fn bucket_name(&self) -> String {
        self.resource_name()
    }

    /// The KV namespace's title, which is the resource name.
    #[must_use]
    pub fn kv_title(&self) -> String {
        self.resource_name()
    }

    /// The Analytics Engine dataset's name. A dataset name admits no dash,
    /// so the resource name's dashes become underscores.
    #[must_use]
    pub fn dataset(&self) -> String {
        self.resource_name().replace('-', "_")
    }

    /// The environment file this deployment's own configuration lives in,
    /// derived from the name so two deployments can never share one.
    #[must_use]
    pub fn env_file_path(&self) -> String {
        format!("infra/.env.{}", self.name)
    }
}

impl fmt::Display for Deployment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grammar_is_config_ts_grammar() {
        for good in [
            "production",
            "staging",
            "cachet-staging",
            "agx",
            "a1",
            "prod-acme",
            "a-",
            "a--b",
            "abcdefghijklmnopqrstuvwxyz012345",
        ] {
            assert!(Deployment::new(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            "a",
            "Production",
            "1prod",
            "-prod",
            "prod_acme",
            "prod.acme",
            "prod acme",
            "abcdefghijklmnopqrstuvwxyz0123456",
        ] {
            assert_eq!(Deployment::new(bad), Err(DeployError::BadName), "{bad}");
        }
    }

    #[test]
    fn the_prefix_is_a_guarantee_and_never_doubled() {
        let plain = Deployment::new("production").expect("valid");
        assert_eq!(plain.resource_name(), "cachet-production");
        assert_eq!(plain.name(), "production");
        let prefixed = Deployment::new("cachet-production").expect("valid");
        assert_eq!(prefixed.resource_name(), "cachet-production");
        assert_eq!(prefixed.name(), "cachet-production");
    }

    #[test]
    fn every_derived_name_hangs_off_the_name() {
        let acme = Deployment::new("prod-acme").expect("valid");
        assert_eq!(acme.worker_name(), "cachet-prod-acme");
        assert_eq!(acme.bucket_name(), "cachet-prod-acme");
        assert_eq!(acme.kv_title(), "cachet-prod-acme");
        assert_eq!(acme.dataset(), "cachet_prod_acme");
        assert_eq!(acme.env_file_path(), "infra/.env.prod-acme");
        assert_eq!(acme.to_string(), "prod-acme");
    }
}
