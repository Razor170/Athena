//! Which version of Athena this is.
//!
//! CI works the version out from the git tags (see "Work out the version" in
//! `.github/workflows/ci.yml`) and hands it to the compiler in the
//! `ATHENA_VERSION` environment variable, for example `0.1.1-alpha.6`.
//! A build made on your own PC has no such variable, so it is a "local build".

/// The version CI built, or `None` for a local build.
///
/// `option_env!` reads the variable while compiling, not while running, so
/// the text is baked into the exe. Cargo rebuilds this file when it changes.
pub const BUILT_VERSION: Option<&str> = option_env!("ATHENA_VERSION");

/// The version as shown in Settings, such as `0.1.1-alpha.6` or
/// `0.1.0 (local build)`.
pub fn display_text() -> String {
    describe(BUILT_VERSION)
}

/// Split out from `display_text` so tests can try both kinds of build.
fn describe(built: Option<&str>) -> String {
    match built {
        Some(version) if !version.trim().is_empty() => version.trim().to_string(),
        // Falls back to the version in Cargo.toml.
        _ => format!("{} (local build)", env!("CARGO_PKG_VERSION")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ci_build_shows_its_version() {
        assert_eq!(describe(Some("0.1.1-alpha.6")), "0.1.1-alpha.6");
    }

    #[test]
    fn local_build_says_so() {
        let text = describe(None);
        assert!(text.ends_with("(local build)"), "{text}");
        assert_eq!(describe(Some("  ")), text);
    }
}
