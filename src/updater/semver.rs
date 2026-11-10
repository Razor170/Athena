//! Comparing versions the SemVer way (https://semver.org), so that
//! `0.1.1-alpha.10` is newer than `0.1.1-alpha.9`, and `0.1.1` is newer than
//! any `0.1.1-alpha.X`.

use std::cmp::Ordering;

/// A parsed version such as `0.1.1-alpha.6`. Build metadata after a `+`
/// (`+pr.34`) is dropped, because SemVer says it never makes a version newer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    /// The parts after the `-`, split at dots: `alpha.6` is `[Text("alpha"), Number(6)]`.
    /// Empty for a normal release.
    pre: Vec<PreId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PreId {
    Number(u64),
    Text(String),
}

impl Version {
    /// Reads `0.1.1-alpha.6`, `v0.1.1-alpha.6` or `0.1.1-alpha.6+pr.34`.
    /// Returns `None` for anything else.
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        // Drop the build metadata.
        let text = text.split('+').next()?;
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let numbers: Vec<u64> = core
            .split('.')
            .map(|part| part.parse().ok())
            .collect::<Option<Vec<u64>>>()?;
        let [major, minor, patch] = numbers[..] else {
            return None;
        };
        let pre = match pre {
            None => Vec::new(),
            Some(pre) => pre
                .split('.')
                .map(|part| {
                    if part.is_empty() {
                        None
                    } else if let Ok(number) = part.parse() {
                        Some(PreId::Number(number))
                    } else {
                        Some(PreId::Text(part.to_string()))
                    }
                })
                .collect::<Option<Vec<PreId>>>()?,
        };
        Some(Version {
            major,
            minor,
            patch,
            pre,
        })
    }

    /// True for versions such as `0.1.1-alpha.6`.
    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }
}

// `Ord` lets us use `<`, `>` and `max()` on versions. The rules are SemVer's.
impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| compare_pre(&self.pre, &other.pre))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A release (no pre-release part) is newer than any pre-release of the
/// same number. Otherwise the parts are compared one by one: numbers by
/// value, text alphabetically, numbers before text, and fewer parts first
/// when all the shared parts are equal.
fn compare_pre(a: &[PreId], b: &[PreId]) -> Ordering {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => return Ordering::Equal,
        (true, false) => return Ordering::Greater,
        (false, true) => return Ordering::Less,
        (false, false) => {}
    }
    for (x, y) in a.iter().zip(b) {
        let order = match (x, y) {
            (PreId::Number(x), PreId::Number(y)) => x.cmp(y),
            (PreId::Text(x), PreId::Text(y)) => x.cmp(y),
            (PreId::Number(_), PreId::Text(_)) => Ordering::Less,
            (PreId::Text(_), PreId::Number(_)) => Ordering::Greater,
        };
        if order != Ordering::Equal {
            return order;
        }
    }
    a.len().cmp(&b.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).expect(text)
    }

    #[test]
    fn reads_tags_and_versions() {
        assert_eq!(v("v0.1.1-alpha.6"), v("0.1.1-alpha.6"));
        assert_eq!(v("0.1.1-alpha.6+pr.34"), v("0.1.1-alpha.6"));
        assert!(v("0.1.1-alpha.6").is_prerelease());
        assert!(!v("v0.2.0").is_prerelease());
        assert_eq!(Version::parse("0.1"), None);
        assert_eq!(Version::parse("0.1.x"), None);
        assert_eq!(Version::parse("0.1.1-"), None);
        assert_eq!(Version::parse("build-142"), None);
    }

    #[test]
    fn alpha_numbers_compare_as_numbers() {
        assert!(v("0.1.1-alpha.10") > v("0.1.1-alpha.9"));
        assert!(v("0.1.1-alpha.6") < v("0.1.1-alpha.7"));
    }

    #[test]
    fn a_release_beats_its_alphas() {
        assert!(v("0.1.1") > v("0.1.1-alpha.99"));
        assert!(v("0.1.2-alpha.1") > v("0.1.1"));
        assert!(v("0.1.0") < v("0.1.1-alpha.1"));
    }

    #[test]
    fn semver_examples() {
        // The order from semver.org, item 11.
        let ordered = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
        ];
        for pair in ordered.windows(2) {
            assert!(v(pair[0]) < v(pair[1]), "{} < {}", pair[0], pair[1]);
        }
    }

    #[test]
    fn build_metadata_doesnt_count() {
        assert_eq!(
            v("0.1.1-alpha.6+pr.34").cmp(&v("0.1.1-alpha.6")),
            Ordering::Equal
        );
    }
}
