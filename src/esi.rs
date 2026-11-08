//! A small client for EVE's ESI API.
//!
//! Only public endpoints for now (no login). It follows ESI's rules
//! (<https://developers.eveonline.com/docs/services/esi/best-practices/>):
//!
//! - sends `X-Compatibility-Date`, so a later ESI change can't silently
//!   change the answers we get,
//! - sends a `User-Agent` naming the app,
//! - sends `If-None-Match` with the last ETag, so unchanged data costs ESI
//!   (and us) almost nothing,
//! - backs off on both rate-limit schemes: the error limit
//!   (`X-ESI-Error-Limit-*`) and the newer token buckets (a 429 answer is
//!   waited out using `Retry-After`; `X-Ratelimit-Remaining` isn't read yet,
//!   since our ~42 requests an hour are far below any bucket).
//!
//! Requests block, so only use this from a background thread, never the UI
//! thread.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::StatusCode;
use reqwest::blocking::{Client, RequestBuilder, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;

const BASE_URL: &str = "https://esi.evetech.net";

/// The ESI version we were written against. See
/// <https://esi.evetech.net/meta/compatibility-dates> for the list.
const COMPATIBILITY_DATE: &str = "2026-08-18";

const USER_AGENT: &str = concat!(
    "Athena/",
    env!("CARGO_PKG_VERSION"),
    " (EVE desktop companion; +https://github.com/Razor170/Athena)"
);

/// How often one request is tried before giving up.
const MAX_ATTEMPTS: u32 = 3;

/// The result of a GET.
pub enum Fetched<T> {
    /// New data.
    Data {
        value: T,
        /// The ETag to send next time.
        etag: Option<String>,
        /// How many pages the whole result has (`X-Pages`, 1 if not paged).
        pages: u32,
    },
    /// The data is unchanged since the ETag we sent.
    NotModified,
}

pub struct EsiClient {
    http: Client,
    /// Errors we may still make before ESI blocks us, and seconds until that
    /// count resets. Updated from every response.
    error_limit_remain: Option<i64>,
    error_limit_reset_secs: Option<u64>,
    /// How long until ESI has new data for the last request: its `Expires`
    /// header minus its `Date` header. `None` if either was missing.
    pub last_expires_in: Option<Duration>,
}

impl EsiClient {
    pub fn new() -> Result<EsiClient> {
        let http = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(30))
            .build()
            .context("can't set up the HTTP client")?;
        Ok(EsiClient {
            http,
            error_limit_remain: None,
            error_limit_reset_secs: None,
            last_expires_in: None,
        })
    }

    /// GETs `path` (for example `/markets/10000002/orders?type_id=34`) and
    /// parses the JSON answer into `T`. Pass the ETag from last time, if any.
    pub fn get_json<T: DeserializeOwned>(
        &mut self,
        path: &str,
        etag: Option<&str>,
    ) -> Result<Fetched<T>> {
        let url = format!("{BASE_URL}{path}");
        let response = self.send(&url, |http| {
            let request = http.get(&url);
            match etag {
                Some(etag) => request.header("If-None-Match", etag),
                None => request,
            }
        })?;
        if response.status() == StatusCode::NOT_MODIFIED {
            return Ok(Fetched::NotModified);
        }
        let etag = header_text(&response, "etag");
        let pages = header_text(&response, "x-pages")
            .and_then(|text| text.parse().ok())
            .unwrap_or(1);
        let value = response
            .json::<T>()
            .with_context(|| format!("reading the answer from {url}"))?;
        Ok(Fetched::Data { value, etag, pages })
    }

    /// POSTs `body` as JSON to `path` and parses the JSON answer into `T`.
    /// Used for lookups such as `/universe/ids` (names to ids).
    pub fn post_json<B: Serialize, T: DeserializeOwned>(
        &mut self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let url = format!("{BASE_URL}{path}");
        let response = self.send(&url, |http| http.post(&url).json(body))?;
        response
            .json::<T>()
            .with_context(|| format!("reading the answer from {url}"))
    }

    /// Sends a request made by `build`, with the headers ESI wants, and
    /// retries network hiccups, rate limits and server errors. Returns a
    /// successful (or 304 "not modified") response.
    fn send(&mut self, url: &str, build: impl Fn(&Client) -> RequestBuilder) -> Result<Response> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            self.wait_for_error_limit();

            let request = build(&self.http)
                .header("X-Compatibility-Date", COMPATIBILITY_DATE)
                .header("Accept", "application/json");
            let response = match request.send() {
                Ok(response) => response,
                Err(_) if attempt < MAX_ATTEMPTS => {
                    // Network hiccup: wait a little and try again.
                    std::thread::sleep(Duration::from_secs(5));
                    continue;
                }
                Err(e) => return Err(e).with_context(|| format!("request to {url}")),
            };
            self.read_limits(&response);
            self.last_expires_in = expires_in(&response);

            let status = response.status();
            if status.is_success() || status == StatusCode::NOT_MODIFIED {
                return Ok(response);
            }

            // 420 = error limit hit, 429 = token bucket empty. Both say how
            // long to wait in Retry-After (or the error-limit reset header).
            let rate_limited = status.as_u16() == 420 || status == StatusCode::TOO_MANY_REQUESTS;
            let server_error = status.is_server_error();
            if (rate_limited || server_error) && attempt < MAX_ATTEMPTS {
                let wait = header_text(&response, "retry-after")
                    .and_then(|text| text.parse::<u64>().ok())
                    .or(self.error_limit_reset_secs)
                    .unwrap_or(10);
                std::thread::sleep(Duration::from_secs(wait.clamp(1, 900)));
                continue;
            }
            bail!("{url} failed: HTTP {status}");
        }
    }

    /// Remembers the error-limit headers of a response.
    fn read_limits(&mut self, response: &Response) {
        if let Some(remain) = header_text(response, "x-esi-error-limit-remain") {
            self.error_limit_remain = remain.parse().ok();
        }
        if let Some(reset) = header_text(response, "x-esi-error-limit-reset") {
            self.error_limit_reset_secs = reset.parse().ok();
        }
    }

    /// If we are close to ESI's error limit, waits until it resets.
    fn wait_for_error_limit(&mut self) {
        const KEEP_IN_RESERVE: i64 = 10;
        if self
            .error_limit_remain
            .is_some_and(|remain| remain < KEEP_IN_RESERVE)
        {
            let wait = self.error_limit_reset_secs.unwrap_or(60).clamp(1, 120);
            std::thread::sleep(Duration::from_secs(wait));
            self.error_limit_remain = None;
        }
    }
}

/// `Expires` minus `Date`: how long ESI's answer stays current. Both come
/// from ESI's clock, so the PC's clock being off doesn't matter.
fn expires_in(response: &Response) -> Option<Duration> {
    let expires = http_date_seconds(&header_text(response, "expires")?)?;
    let date = http_date_seconds(&header_text(response, "date")?)?;
    u64::try_from(expires - date).ok().map(Duration::from_secs)
}

/// An HTTP date such as "Sun, 08 Nov 2026 16:49:14 GMT" as seconds since
/// 1970. Only this one format, which is the one HTTP requires.
fn http_date_seconds(text: &str) -> Option<i64> {
    let parts: Vec<&str> = text.split_whitespace().collect();
    let [_, day, month, year, time, "GMT"] = parts[..] else {
        return None;
    };
    let day: i64 = day.parse().ok()?;
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .iter()
    .position(|m| *m == month)? as i64
        + 1;
    let year: i64 = year.parse().ok()?;
    let clock: Vec<i64> = time
        .split(':')
        .map(|part| part.parse().ok())
        .collect::<Option<Vec<i64>>>()?;
    let [hours, minutes, seconds] = clock[..] else {
        return None;
    };
    // Days since 1970-01-01 for a date (Howard Hinnant's well-known
    // "days from civil" calculation; March is treated as the first month
    // so that leap days fall at the end of the year).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let year_of_era = y - era * 400;
    let shifted_month = (month + 9) % 12;
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some(days * 86_400 + hours * 3600 + minutes * 60 + seconds)
}

/// A response header as text, if present and readable.
fn header_text(response: &Response, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)?
        .to_str()
        .ok()
        .map(|text| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_dates() {
        assert_eq!(http_date_seconds("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        // `date -u -d "2026-11-08 16:49:14" +%s`
        assert_eq!(
            http_date_seconds("Sun, 08 Nov 2026 16:49:14 GMT"),
            Some(1_794_156_554)
        );
        // A leap day.
        assert_eq!(
            http_date_seconds("Thu, 29 Feb 2024 12:00:00 GMT"),
            Some(1_709_208_000)
        );
        assert_eq!(http_date_seconds("yesterday"), None);
    }
}
