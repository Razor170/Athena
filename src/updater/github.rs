//! Talking to GitHub: which releases exist, and downloading one.
//!
//! The repo is private, so every request carries the user's read-only token.
//! These functions block, so they only run on the updater's background thread.

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use reqwest::StatusCode;
use reqwest::blocking::{Client, RequestBuilder, Response};
use serde::Deserialize;

use super::semver::Version;

/// Releases are read from here. The repo name is public knowledge, not a secret.
const RELEASES_URL: &str = "https://api.github.com/repos/Razor170/Athena/releases?per_page=30";
/// The file CI attaches to every release.
const ASSET_NAME: &str = "Athena-windows.zip";

/// One release as GitHub's API sends it. Only the fields we use.
#[derive(Debug, Deserialize)]
pub struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Debug, Deserialize)]
struct ApiAsset {
    name: String,
    /// The API address of the file. Asking it for `application/octet-stream`
    /// sends us on to the file itself.
    url: String,
    size: u64,
}

/// A release Athena could update to.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub version: Version,
    /// The version as written in the tag, without the `v`, e.g. `0.1.1-alpha.7`.
    pub version_text: String,
    /// The release date, `YYYY-MM-DD`.
    pub date: String,
    /// One line per merged PR, taken from the release notes.
    pub notes: Vec<String>,
    asset_url: String,
    pub size: u64,
}

/// What a check found out.
pub struct CheckResult {
    /// The newest release Athena may update to, whether or not it is newer
    /// than this version (the caller compares).
    pub newest: Option<Release>,
    /// When the token stops working, `YYYY-MM-DD`, if GitHub said so.
    pub token_expires: Option<String>,
}

/// Builds the HTTP client. No overall time limit, because a download can
/// take a while on a slow line; the API call sets its own limit.
pub fn client() -> Result<Client> {
    Client::builder()
        .user_agent(concat!("Athena-updater/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(None)
        .build()
        .context("can't set up the HTTP client")
}

/// Headers every GitHub API call needs.
fn github_request(builder: RequestBuilder, token: &str) -> RequestBuilder {
    builder
        .bearer_auth(token)
        .header("X-GitHub-Api-Version", "2022-11-28")
}

/// Asks GitHub for the recent releases and picks the newest usable one.
pub fn check(client: &Client, token: &str, include_prereleases: bool) -> Result<CheckResult> {
    let response = github_request(client.get(RELEASES_URL), token)
        .header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(30))
        .send()
        .context("can't reach GitHub")?;
    let response = check_status(response)?;
    let token_expires = response
        .headers()
        .get("github-authentication-token-expiration")
        .and_then(|value| value.to_str().ok())
        // "2027-11-10 19:00:00 UTC": the date is enough.
        .and_then(|text| text.get(..10))
        .map(str::to_string);
    let releases: Vec<ApiRelease> = response
        .json()
        .context("GitHub sent a list of releases Athena can't read")?;
    Ok(CheckResult {
        newest: newest_release(releases, include_prereleases),
        token_expires,
    })
}

/// Turns GitHub's error codes into messages that say what to do.
fn check_status(response: Response) -> Result<Response> {
    match response.status() {
        status if status.is_success() => Ok(response),
        StatusCode::UNAUTHORIZED => Err(anyhow!(
            "GitHub refused the token. It may have expired; make a new one in Settings."
        )),
        StatusCode::NOT_FOUND => Err(anyhow!(
            "GitHub can't find Athena with this token. Check that the token has access to the Athena repository with Contents: Read-only."
        )),
        StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS => Err(anyhow!(
            "GitHub is limiting requests right now. Try again in a few minutes."
        )),
        status => Err(anyhow!("GitHub answered with an error ({status}).")),
    }
}

/// The newest release that has our zip, skipping drafts, tags that aren't
/// versions, and (unless wanted) pre-releases.
pub fn newest_release(releases: Vec<ApiRelease>, include_prereleases: bool) -> Option<Release> {
    releases
        .into_iter()
        .filter(|release| !release.draft)
        .filter(|release| include_prereleases || !release.prerelease)
        .filter_map(|release| {
            let version = Version::parse(&release.tag_name)?;
            // A release marked as normal but with an alpha version is still
            // an alpha, so the checkbox decides by the version too.
            if version.is_prerelease() && !include_prereleases {
                return None;
            }
            let asset = release.assets.iter().find(|a| a.name == ASSET_NAME)?;
            let tag = release.tag_name.trim();
            Some(Release {
                version,
                version_text: tag.strip_prefix('v').unwrap_or(tag).to_string(),
                date: release
                    .published_at
                    .as_deref()
                    .and_then(|date| date.get(..10))
                    .unwrap_or_default()
                    .to_string(),
                notes: note_lines(release.body.as_deref().unwrap_or_default()),
                asset_url: asset.url.clone(),
                size: asset.size,
            })
        })
        .max_by(|a, b| a.version.cmp(&b.version))
}

/// Shortens GitHub's generated notes to one line per PR:
/// `* Mining rates in m³/s by @someone in https://github.com/o/r/pull/35`
/// becomes `Mining rates in m³/s (#35)`.
fn note_lines(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|line| {
            let line = line.trim();
            let text = line
                .strip_prefix("* ")
                .or_else(|| line.strip_prefix("- "))?;
            let (title, pr) = match text.rsplit_once(" in https://") {
                Some((before, link)) => {
                    let number = link.rsplit('/').next().unwrap_or_default();
                    (before, Some(number))
                }
                None => (text, None),
            };
            let title = match title.rsplit_once(" by @") {
                Some((title, _author)) => title,
                None => title,
            };
            Some(match pr {
                Some(number) if number.chars().all(|c| c.is_ascii_digit()) => {
                    format!("{title} (#{number})")
                }
                _ => title.to_string(),
            })
        })
        .collect()
}

/// Downloads the release's zip to `destination`. `progress` gets
/// (bytes so far, total) a few times a second. Stops early with an error
/// when `cancel` is set.
pub fn download(
    client: &Client,
    token: &str,
    release: &Release,
    destination: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<()> {
    // GitHub answers with a redirect to its file servers. reqwest follows it
    // and drops the token from the request when the host changes, so the
    // token only ever goes to api.github.com.
    let response = github_request(client.get(&release.asset_url), token)
        .header("Accept", "application/octet-stream")
        .send()
        .context("can't reach GitHub")?;
    let mut response = check_status(response)?;
    let total = release.size;
    let mut file = File::create(destination)
        .with_context(|| format!("can't write {}", destination.display()))?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut done: u64 = 0;
    let mut last_report = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            bail!("Download cancelled.");
        }
        let read = response
            .read(&mut buffer)
            .context("the download was interrupted")?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])
            .with_context(|| format!("can't write {}", destination.display()))?;
        done += read as u64;
        // A handful of updates a second is plenty for a progress bar.
        if last_report.elapsed() >= Duration::from_millis(250) {
            progress(done, total);
            last_report = Instant::now();
        }
    }
    file.flush()?;
    if done != total {
        bail!("The download is incomplete ({done} of {total} bytes). Try again.");
    }
    progress(done, total);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, prerelease: bool, with_zip: bool) -> ApiRelease {
        ApiRelease {
            tag_name: tag.to_string(),
            prerelease,
            draft: false,
            body: None,
            published_at: Some("2026-11-10T19:00:00Z".to_string()),
            assets: if with_zip {
                vec![ApiAsset {
                    name: ASSET_NAME.to_string(),
                    url: format!("https://api.github.com/assets/{tag}"),
                    size: 100,
                }]
            } else {
                Vec::new()
            },
        }
    }

    #[test]
    fn picks_the_newest_by_version_not_by_list_order() {
        let list = vec![
            release("v0.1.1-alpha.9", true, true),
            release("v0.1.1-alpha.10", true, true),
            release("v0.1.0", false, true),
        ];
        let newest = newest_release(list, true).unwrap();
        assert_eq!(newest.version_text, "0.1.1-alpha.10");
        assert_eq!(newest.date, "2026-11-10");
    }

    #[test]
    fn prereleases_only_when_wanted() {
        let list = vec![
            release("v0.1.1-alpha.10", true, true),
            release("v0.1.0", false, true),
        ];
        assert_eq!(newest_release(list, false).unwrap().version_text, "0.1.0");
    }

    #[test]
    fn skips_drafts_odd_tags_and_releases_without_the_zip() {
        let mut draft = release("v0.3.0", false, true);
        draft.draft = true;
        let list = vec![
            draft,
            release("nightly", true, true),
            release("v0.2.0", false, false),
            release("v0.1.0", false, true),
        ];
        assert_eq!(newest_release(list, true).unwrap().version_text, "0.1.0");
        assert_eq!(newest_release(Vec::new(), true), None);
    }

    #[test]
    fn shortens_generated_notes() {
        let body = "## What's Changed\n\
            * Mining rates in m³/s by @Razor170 in https://github.com/Razor170/Athena/pull/35\n\
            * Plain line without a link\n\
            \n\
            **Full Changelog**: https://github.com/Razor170/Athena/compare/v0.1.0...v0.1.1-alpha.6";
        assert_eq!(
            note_lines(body),
            vec!["Mining rates in m³/s (#35)", "Plain line without a link"]
        );
    }
}
