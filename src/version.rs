#![allow(dead_code)]

/// Version constraint and resolution for relava.toml entries.
///
/// Two formats:
///   - `"X.Y.Z"` — exact version pin
///   - `"*"` — latest available version
///
/// A parsed version constraint from relava.toml.
#[derive(Debug, Clone, PartialEq)]
pub enum VersionConstraint {
    /// Exact version pin: "1.2.0"
    Exact(Version),
    /// Latest available: "*"
    Latest,
}

/// A semantic version (major.minor.patch).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    pub fn parse(s: &str) -> Result<Self, VersionError> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 {
            return Err(VersionError::InvalidFormat(s.to_string()));
        }
        let major = parts[0]
            .parse()
            .map_err(|_| VersionError::InvalidFormat(s.to_string()))?;
        let minor = parts[1]
            .parse()
            .map_err(|_| VersionError::InvalidFormat(s.to_string()))?;
        let patch = parts[2]
            .parse()
            .map_err(|_| VersionError::InvalidFormat(s.to_string()))?;
        Ok(Self {
            major,
            minor,
            patch,
        })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl VersionConstraint {
    pub fn parse(s: &str) -> Result<Self, VersionError> {
        if s == "*" {
            Ok(Self::Latest)
        } else {
            Ok(Self::Exact(Version::parse(s)?))
        }
    }

    /// Resolve this constraint against a list of available versions.
    /// Returns the matching version, or an error if none match.
    pub fn resolve(&self, available: &[Version]) -> Result<Version, VersionError> {
        if available.is_empty() {
            return Err(VersionError::NoVersionsAvailable);
        }
        match self {
            Self::Exact(v) => {
                if available.contains(v) {
                    Ok(v.clone())
                } else {
                    Err(VersionError::VersionNotFound(v.to_string()))
                }
            }
            Self::Latest => {
                let latest = available.iter().max().unwrap();
                Ok(latest.clone())
            }
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum VersionError {
    InvalidFormat(String),
    NoVersionsAvailable,
    VersionNotFound(String),
}

impl std::fmt::Display for VersionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VersionError::InvalidFormat(s) => write!(f, "invalid version format: '{s}'"),
            VersionError::NoVersionsAvailable => write!(f, "no versions available"),
            VersionError::VersionNotFound(v) => write!(f, "version {v} not found in registry"),
        }
    }
}

impl std::error::Error for VersionError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_version() {
        let v = Version::parse("1.2.3").unwrap();
        assert_eq!(
            v,
            Version {
                major: 1,
                minor: 2,
                patch: 3
            }
        );
    }

    #[test]
    fn parse_zero_version() {
        let v = Version::parse("0.0.0").unwrap();
        assert_eq!(
            v,
            Version {
                major: 0,
                minor: 0,
                patch: 0
            }
        );
    }

    #[test]
    fn parse_invalid_format() {
        assert!(Version::parse("1.2").is_err());
        assert!(Version::parse("1.2.3.4").is_err());
        assert!(Version::parse("abc").is_err());
        assert!(Version::parse("1.2.x").is_err());
        assert!(Version::parse("").is_err());
    }

    #[test]
    fn version_display() {
        let v = Version {
            major: 1,
            minor: 2,
            patch: 3,
        };
        assert_eq!(v.to_string(), "1.2.3");
    }

    #[test]
    fn version_ordering() {
        let v1 = Version::parse("1.0.0").unwrap();
        let v2 = Version::parse("1.0.1").unwrap();
        let v3 = Version::parse("1.1.0").unwrap();
        let v4 = Version::parse("2.0.0").unwrap();
        assert!(v1 < v2);
        assert!(v2 < v3);
        assert!(v3 < v4);
    }

    #[test]
    fn constraint_exact() {
        let c = VersionConstraint::parse("1.2.3").unwrap();
        assert_eq!(
            c,
            VersionConstraint::Exact(Version {
                major: 1,
                minor: 2,
                patch: 3
            })
        );
    }

    #[test]
    fn constraint_latest() {
        let c = VersionConstraint::parse("*").unwrap();
        assert_eq!(c, VersionConstraint::Latest);
    }

    #[test]
    fn constraint_invalid() {
        assert!(VersionConstraint::parse("1.2").is_err());
        assert!(VersionConstraint::parse("latest").is_err());
    }

    #[test]
    fn resolve_exact_found() {
        let available = vec![
            Version::parse("1.0.0").unwrap(),
            Version::parse("1.2.0").unwrap(),
            Version::parse("2.0.0").unwrap(),
        ];
        let c = VersionConstraint::parse("1.2.0").unwrap();
        let resolved = c.resolve(&available).unwrap();
        assert_eq!(resolved, Version::parse("1.2.0").unwrap());
    }

    #[test]
    fn resolve_exact_not_found() {
        let available = vec![
            Version::parse("1.0.0").unwrap(),
            Version::parse("2.0.0").unwrap(),
        ];
        let c = VersionConstraint::parse("1.2.0").unwrap();
        assert_eq!(
            c.resolve(&available),
            Err(VersionError::VersionNotFound("1.2.0".to_string()))
        );
    }

    #[test]
    fn resolve_latest() {
        let available = vec![
            Version::parse("1.0.0").unwrap(),
            Version::parse("0.5.0").unwrap(),
            Version::parse("2.1.0").unwrap(),
            Version::parse("1.9.0").unwrap(),
        ];
        let c = VersionConstraint::parse("*").unwrap();
        let resolved = c.resolve(&available).unwrap();
        assert_eq!(resolved, Version::parse("2.1.0").unwrap());
    }

    #[test]
    fn resolve_empty_available() {
        let c = VersionConstraint::parse("*").unwrap();
        assert_eq!(c.resolve(&[]), Err(VersionError::NoVersionsAvailable));

        let c = VersionConstraint::parse("1.0.0").unwrap();
        assert_eq!(c.resolve(&[]), Err(VersionError::NoVersionsAvailable));
    }
}
