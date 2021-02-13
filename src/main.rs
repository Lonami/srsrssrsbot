use std::collections::HashSet;

// Fetch an old feed and then its updated variant to figure out how "new entries" works.
static OLD_FEED: &str = env!("OLD_FEED");
static NEW_FEED: &str = env!("NEW_FEED");

#[tokio::main]
async fn main() {
    let http = reqwest::Client::new();

    let xml = http
        .get(OLD_FEED)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();

    let feed = feed_rs::parser::parse(xml.as_ref()).unwrap();
    let entries = feed
        .entries
        .into_iter()
        .map(|entry| entry.id)
        .collect::<HashSet<_>>();

    let xml = http
        .get(NEW_FEED)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();

    let mut feed = feed_rs::parser::parse(xml.as_ref()).unwrap();
    feed.entries.retain(|entry| !entries.contains(&entry.id));

    feed.entries.into_iter().for_each(|entry| {
        println!("New entry {}:\n  {}", entry.id, entry.title.unwrap().content);
    });
}
