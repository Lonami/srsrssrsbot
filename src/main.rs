mod feed;

use grammers_client::{Client, ClientHandle, Config, Update};
use grammers_session::FileSession;
use log;
use mpsc::{Sender, UnboundedSender};
use simple_logger::SimpleLogger;
use std::collections::{BinaryHeap, HashSet};
use tokio::sync::mpsc;

static LOG_LEVEL: &str = env!("LOG_LEVEL");

// Fetch an old feed and then its updated variant to figure out how "new entries" works.
static OLD_FEED: &str = env!("OLD_FEED");
static NEW_FEED: &str = env!("NEW_FEED");

// Values required by Telegram.
static TG_API_ID: &str = env!("TG_API_ID");
static TG_API_HASH: &str = env!("TG_API_HASH");
static BOT_TOKEN: &str = env!("BOT_TOKEN");

static SESSION_NAME: &str = "srsrssrs.session";

// Strings.
static STR_WELCOME: &str = r#"Hi, I'm srsrssrs, a serious RSS Rust bot. Sorry if it gave you a stroke to read that.

To get started, /add <FEED URL>. If you get tired of the feed, use /rm <FEED URL>. You can view what feeds you're subscribed to with /ls."#;

static STR_NOT_IMPLEMENTED: &str = "Not yet implemented.";

static STR_NO_URL: &str = "You need to include the URL after the command.";

fn str_try_add(url: &str) -> String {
    format!("Trying to add {}...", url)
}

fn str_add_ok(url: &str) -> String {
    format!("Added {} to your list of feeds.", url)
}

fn str_add_err(url: &str, e: feed::Error) -> String {
    format!("Failed to add {} to your list of feeds: {}.", url, e)
}

async fn step_network(
    mut tg: Client<FileSession>,
    tx: mpsc::UnboundedSender<Update>,
) -> Client<FileSession> {
    loop {
        match tg.next_updates().await {
            Ok(Some(updates)) => {
                updates.for_each(|update| tx.send(update).map_err(drop).unwrap());
            }
            Ok(None) => {
                break;
            }
            Err(e) => {
                eprintln!("Error reading updates: {}", e);
            }
        }
    }
    tg
}

async fn step_updates(mut tg: ClientHandle, mut rx: mpsc::UnboundedReceiver<Update>) {
    let http = reqwest::Client::new();
    let mut feeds = BinaryHeap::new();

    while let Some(update) = rx.recv().await {
        match update {
            Update::NewMessage(message) if !message.outgoing() => {
                if message.text().starts_with("/start") || message.text().starts_with("/help") {
                    tg.send_message(&message.chat(), STR_WELCOME.into())
                        .await
                        .unwrap();
                } else if message.text().starts_with("/add") {
                    if let Some(url) = message.text().split_whitespace().nth(1) {
                        let mut sent = tg
                            .send_message(&message.chat(), str_try_add(url).into())
                            .await
                            .unwrap();

                        match feed::Feed::new(&http, url, message.sender().unwrap().id()).await {
                            Ok(feed) => {
                                sent.edit(str_add_ok(url).into()).await.unwrap();
                                feeds.push(feed);
                            }
                            Err(e) => {
                                sent.edit(str_add_err(url, e).into()).await.unwrap();
                            }
                        }
                    } else {
                        tg.send_message(&message.chat(), STR_NO_URL.into())
                            .await
                            .unwrap();
                    }
                } else if message.text().starts_with("/rm") {
                    tg.send_message(&message.chat(), STR_NOT_IMPLEMENTED.into())
                        .await
                        .unwrap();
                } else if message.text().starts_with("/ls") {
                    tg.send_message(&message.chat(), STR_NOT_IMPLEMENTED.into())
                        .await
                        .unwrap();
                }
            }
            _ => {}
        };
    }
}

#[tokio::main]
async fn main() {
    SimpleLogger::new()
        .with_level(match LOG_LEVEL {
            "ERROR" => log::LevelFilter::Error,
            "WARN" => log::LevelFilter::Warn,
            "INFO" => log::LevelFilter::Info,
            "DEBUG" => log::LevelFilter::Debug,
            "TRACE" => log::LevelFilter::Trace,
            _ => log::LevelFilter::Off,
        })
        .init()
        .unwrap();

    let api_id = TG_API_ID.parse().unwrap();
    let mut client = Client::connect(Config {
        session: FileSession::load_or_create(SESSION_NAME).unwrap(),
        api_id,
        api_hash: TG_API_HASH.to_string(),
        params: Default::default(),
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

    let mut tg = client.handle();

    // Need the `client` to be stepping the network, or the handle methods will never complete.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::join!(step_updates(client.handle(), rx), step_network(client, tx));
}
