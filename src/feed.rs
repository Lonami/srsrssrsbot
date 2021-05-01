use chrono::{DateTime, Duration, Utc};
use grammers_client::types::chat::PackedChat;
use reqwest::{header, StatusCode};
use std::{collections::HashSet, fmt};
use tokio::time::Instant;

pub struct Feed {
    pub url: String,
    pub users: Vec<PackedChat>,
    pub seen_entries: HashSet<String>,
    pub last_fetch: DateTime<Utc>,
    pub next_fetch: Instant,
    pub etag: Option<String>,
}

#[derive(Debug)]
pub enum Error {
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
    // Can't use constants here, `Duration::seconds` is not a const-fn as of 0.4.19.
    //
    // Maximum cache delay we're willing to accept.
    //
    // A bad-behaved server might put an absurd amount for the `max-age`, and then we would never
    // check that feed again.
    let max_fetch_delay: Duration = Duration::seconds(24 * 60 * 60);

    // If the server returns a very small value (or even in the past), use this instead.
    let min_fetch_delay: Duration = Duration::seconds(60);

    // If the server does not have any max age or expiration for the feed, use a default delay.
    let default_fetch_delay: Duration = Duration::seconds(10 * 60);

    let now = Utc::now();
    let delay = if let Some(cache_control) = header(headers, header::CACHE_CONTROL)? {
        let seconds = cache_control.split(",").find_map(|directive| {
            let mut parts = directive.split("=");
            let key = parts.next()?;
            let value = parts.next()?;
            if key.to_lowercase() == "max-age" {
                Some(value)
            } else {
                None
            }
        });
        if let Some(seconds) = seconds {
            Duration::seconds(
                seconds
                    .parse::<i64>()
                    .map_err(|_| Error::MalformedHeader(header::CACHE_CONTROL))?,
            )
        } else {
            default_fetch_delay
        }
    } else if let Some(expiry) = header(headers, header::EXPIRES)? {
        let expires = DateTime::parse_from_rfc2822(expiry)
            .map(DateTime::<Utc>::from)
            .map_err(|_| Error::MalformedHeader(header::EXPIRES))?;

        expires - now
    } else {
        default_fetch_delay
    };

    // Can't panic, `max(MIN_FETCH_DELAY)` will make it positive, so `to_std()` succeeds.
    Ok(Instant::now()
        + delay
            .min(max_fetch_delay)
            .max(min_fetch_delay)
            .to_std()
            .unwrap())
}

impl Feed {
    pub async fn new(http: &reqwest::Client, url: &str, user: PackedChat) -> Result<Self, Error> {
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
            users: vec![user],
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
        self.last_fetch = Utc::now();
        self.next_fetch = match find_expiry(resp.headers()) {
            Ok(expiry) => expiry,
            Err(_) => Instant::now() + Duration::seconds(10 * 60).to_std().unwrap(),
        };
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

impl PartialEq for Feed {
    fn eq(&self, other: &Self) -> bool {
        self.next_fetch == other.next_fetch && self.url == other.url
    }
}

impl Eq for Feed {}

impl PartialOrd for Feed {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Feed {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.next_fetch
            .cmp(&other.next_fetch)
            .then_with(|| self.url.cmp(&other.url))
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
