use std::collections::HashSet;

struct Feed {
    url: String,
    users: Vec<i32>,
    seen_entries: HashSet<String>,
}

impl Feed {
    pub async fn new(http: &reqwest::Client, url: &str, user_id: i32) -> Self {
        let xml = http.get(url).send().await.unwrap().bytes().await.unwrap();

        let feed = feed_rs::parser::parse(xml.as_ref()).unwrap();
        let seen_entries = feed
            .entries
            .into_iter()
            .map(|entry| entry.id)
            .collect::<HashSet<_>>();

        Self {
            url: url.to_string(),
            users: vec![user_id],
            seen_entries,
        }
    }

    pub async fn check(&mut self, http: &reqwest::Client, url: &str) -> Vec<feed_rs::model::Entry> {
        let xml = http.get(url).send().await.unwrap().bytes().await.unwrap();

        let mut feed = feed_rs::parser::parse(xml.as_ref()).unwrap();
        feed.entries
            .retain(|entry| !self.seen_entries.contains(&entry.id));
        feed.entries
    }
}
