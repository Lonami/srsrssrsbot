use std::collections::HashSet;
use std::fmt;

struct Feed {
    url: String,
    users: Vec<i32>,
    seen_entries: HashSet<String>,
}

#[derive(Debug)]
enum Error {
    ReadError(reqwest::Error),
    ParseError(feed_rs::parser::ParseFeedError),
}

impl Feed {
    pub async fn new(http: &reqwest::Client, url: &str, user_id: i32) -> Result<Self, Error> {
        let xml = http.get(url).send().await?.bytes().await?;

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
        })
    }

    pub async fn check(
        &mut self,
        http: &reqwest::Client,
        url: &str,
    ) -> Result<Vec<feed_rs::model::Entry>, Error> {
        let xml = http.get(url).send().await?.bytes().await?;
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
