use grammers_client::{Client, Config, InitParams};
use grammers_session::FileSession;
use std::collections::HashSet;

// Fetch an old feed and then its updated variant to figure out how "new entries" works.
static OLD_FEED: &str = env!("OLD_FEED");
static NEW_FEED: &str = env!("NEW_FEED");

// Values required by Telegram.
static TG_API_ID: &str = env!("TG_API_ID");
static TG_API_HASH: &str = env!("TG_API_HASH");
static BOT_TOKEN: &str = env!("BOT_TOKEN");

static SESSION_NAME: &str = "srsrssrs.session";

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
        println!(
            "New entry {}:\n  {}",
            entry.id,
            entry.title.unwrap().content
        );
    });

    let api_id = TG_API_ID.parse().unwrap();
    let mut client = Client::connect(Config {
        session: FileSession::load_or_create(SESSION_NAME).unwrap(),
        api_id,
        api_hash: TG_API_HASH.to_string(),
        params: InitParams {
            // Fetch the updates we missed while we were offline
            catch_up: true,
            ..Default::default()
        },
    })
    .await
    .unwrap();

    if !client.is_authorized().await.unwrap() {
        client
            .bot_sign_in(BOT_TOKEN, api_id, TG_API_HASH)
            .await
            .unwrap();
        client.session().save().unwrap();
    }

    while let Some(_updates) = client.next_updates().await.unwrap() {
        println!("Got updates");
    }
}