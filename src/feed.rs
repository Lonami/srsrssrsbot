use chrono::{DateTime, Utc};
use reqwest::{header, StatusCode};
use std::time::Duration;
use std::{collections::HashSet, fmt};
use tokio::time::Instant;

/// Maximum cache delay we're willing to accept.
///
/// A bad-behaved server might put an absurd amount for the `max-age`, and then we would never
/// check that feed again.
const MAX_FETCH_DELAY: Duration = Duration::from_secs(24 * 60 * 60);

/// If the server does not have any max age or expiration for the feed, use a default delay.
const DEFAULT_FETCH_DELAY: Duration = Duration::from_secs(10 * 60);

struct Feed {
    url: String,
    users: Vec<i32>,
    seen_entries: HashSet<String>,
    last_fetch: DateTime<Utc>,
    next_fetch: Instant,
    etag: Option<String>,
}

#[derive(Debug)]
enum Error {
    ReadError(reqwest::Error),
    ParseError(feed_rs::parser::ParseFeedError),
    MalformedHeader(header::HeaderName),
}

fn header(headers: &header::HeaderMap, key: header::HeaderName) -> Result<Option<&str>, Error> {
    Ok(match headers.get(&key) {
        Some(v) => Some(v.to_str().map_err(|_| Error::MalformedHeader(key))?),
        None => None,
    })
}

fn find_expiry(headers: &header::HeaderMap) -> Result<Instant, Error> {
    let now = Utc::now();
    let delay = if let Some(_cache_control) = header(headers, header::CACHE_CONTROL)? {
        todo!()
    } else if let Some(expiry) = header(headers, header::EXPIRES)? {
        let expires = DateTime::parse_from_rfc2822(expiry)
            .map(DateTime::<Utc>::from)
            .map_err(|_| Error::MalformedHeader(header::EXPIRES))?;

        // If expiry date is less than now, `std::time::Duration` cannot represent it.
        // This means that the server time is either wrong or we took too long to fetch.
        (expires - now).to_std().unwrap_or(DEFAULT_FETCH_DELAY)
    } else {
        DEFAULT_FETCH_DELAY
    };

    Ok(Instant::now() + delay.min(MAX_FETCH_DELAY))
}

impl Feed {
    pub async fn new(http: &reqwest::Client, url: &str, user_id: i32) -> Result<Self, Error> {
        let resp = http.get(url).send().await?.error_for_status()?;
        let last_fetch = Utc::now();
        let next_fetch = find_expiry(resp.headers())?;
        let etag = header(resp.headers(), header::ETAG)?.map(String::from);
        let xml = resp.bytes().await?;

        let feed = feed_rs::parser::parse(xml.as_ref())?;
        let seen_entries = feed
            .entries
            .into_iter()
            .map(|entry| entry.id)
            .collect::<HashSet<_>>();

        Ok(Self {
            url: url.to_string(),
            users: vec![user_id],
            seen_entries,
            last_fetch,
            next_fetch,
            etag,
        })
    }

    pub async fn check(
        &mut self,
        http: &reqwest::Client,
    ) -> Result<Vec<feed_rs::model::Entry>, Error> {
        let mut request = http
            .get(&self.url)
            .header(header::IF_MODIFIED_SINCE, self.last_fetch.to_rfc2822());

        if let Some(etag) = self.etag.as_ref() {
            request = request.header(header::IF_NONE_MATCH, etag);
        }

        let resp = request.send().await?.error_for_status()?;
        if resp.status().as_u16() == StatusCode::NOT_MODIFIED {
            return Ok(Vec::new());
        }

        let xml = resp.bytes().await?;
        let mut feed = feed_rs::parser::parse(xml.as_ref())?;
        feed.entries
            .retain(|entry| !self.seen_entries.contains(&entry.id));

        Ok(feed.entries)
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Self::ReadError(e)
    }
}

impl From<feed_rs::parser::ParseFeedError> for Error {
    fn from(e: feed_rs::parser::ParseFeedError) -> Self {
        Self::ParseError(e)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadError(e) => write!(f, "network error: {}", e),
            Self::ParseError(e) => write!(f, "error parsing feed: {}", e),
            Self::MalformedHeader(e) => write!(f, "error parsing header {}", e),
        }
    }
}

impl std::error::Error for Error {}
