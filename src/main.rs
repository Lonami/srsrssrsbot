mod db;
mod feed;

use grammers_client::client::chats::InvocationError;
use grammers_client::types::{Chat, Message};
use grammers_client::{Client, Config, Update};
use grammers_session::Session;
use log;
use simple_logger::SimpleLogger;
use std::collections::BinaryHeap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::{sleep, Instant};

/// How long to sleep if no feeds have been added yet.
const NO_FEED_DELAY: Duration = Duration::from_secs(60);

static LOG_LEVEL: &str = env!("LOG_LEVEL");

// Fetch an old feed and then its updated variant to figure out how "new entries" works.
/*
static OLD_FEED: &str = env!("OLD_FEED");
static NEW_FEED: &str = env!("NEW_FEED");
*/

// Values required by Telegram.
static TG_API_ID: &str = env!("TG_API_ID");
static TG_API_HASH: &str = env!("TG_API_HASH");
static BOT_TOKEN: &str = env!("BOT_TOKEN");

static DB_NAME: &str = "srsrssrs.db";
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

fn str_new_entry(feed: &feed_rs::model::Entry) -> String {
    let title = feed
        .title
        .as_ref()
        .map(|t| t.content.clone())
        .unwrap_or_else(|| "(untitled)".to_string());

    let url = feed
        .links
        .iter()
        .next()
        .map(|link| link.href.clone())
        .unwrap_or_else(|| "(no online url)".to_string());

    format!("{}\n{}", title, url)
}

async fn handle_updates(
    mut tg: Client,
    db: db::Database,
    feeds: Arc<Mutex<BinaryHeap<feed::Feed>>>,
) -> Result<(), InvocationError> {
    let http = reqwest::Client::new();

    while let Some(updates) = tg.next_updates().await? {
        for update in updates {
            match update {
                Update::NewMessage(message)
                    if !message.outgoing() && matches!(message.chat(), Chat::User(_)) =>
                {
                    handle_message(&mut tg, &http, &db, message, &feeds).await?;
                }
                _ => {}
            };
        }
    }

    Ok(())
}

async fn handle_message(
    tg: &mut Client,
    http: &reqwest::Client,
    db: &db::Database,
    message: Message,
    feeds: &Arc<Mutex<BinaryHeap<feed::Feed>>>,
) -> Result<(), InvocationError> {
    let cmd = match message.text().split_whitespace().next() {
        Some(cmd) => cmd,
        None => return Ok(()),
    };

    if cmd == "/start" || cmd == "/help" {
        tg.send_message(&message.chat(), STR_WELCOME.into())
            .await
            .unwrap();
    } else if cmd == "/add" {
        if let Some(url) = message.text().split_whitespace().nth(1) {
            let mut sent = tg
                .send_message(&message.chat(), str_try_add(url).into())
                .await
                .unwrap();

            match feed::Feed::new(&http, url, message.sender().unwrap().pack()).await {
                Ok(feed) => {
                    sent.edit(str_add_ok(url).into()).await.unwrap();
                    db.add_feed(&feed).unwrap();
                    feeds.lock().unwrap().push(feed);
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
    } else if cmd == "/rm" {
        tg.send_message(&message.chat(), STR_NOT_IMPLEMENTED.into())
            .await
            .unwrap();
    } else if cmd == "/ls" {
        tg.send_message(&message.chat(), STR_NOT_IMPLEMENTED.into())
            .await
            .unwrap();
    }

    Ok(())
}

async fn handle_feed(mut tg: Client, feeds: Arc<Mutex<BinaryHeap<feed::Feed>>>) {
    let http = reqwest::Client::new();

    loop {
        let delay = {
            match feeds.lock().unwrap().peek() {
                Some(feed) => feed.next_fetch.checked_duration_since(Instant::now()),
                None => Some(NO_FEED_DELAY),
            }
        };

        if let Some(delay) = delay {
            sleep(delay).await;
        }

        let mut feed = {
            match feeds.lock().unwrap().pop() {
                Some(feed) => feed,
                None => continue,
            }
        };
        for entry in feed.check(&http).await.unwrap() {
            for user in feed.users.iter() {
                tg.send_message(&user.unpack(), str_new_entry(&entry).into())
                    .await
                    .unwrap();
            }
        }
        feeds.lock().unwrap().push(feed);
    }
}

#[tokio::main]
async fn main() {
    let db = db::Database::new(DB_NAME).unwrap();
    db.cleanup_feeds().unwrap();

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

    let feeds = Arc::new(Mutex::new(db.load_feeds().unwrap()));

    let api_id = TG_API_ID.parse().unwrap();
    let mut client = Client::connect(Config {
        session: Session::load_file_or_create(SESSION_NAME).unwrap(),
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
        client.session().save_to_file(SESSION_NAME).unwrap();
    }

    tokio::select!(
        _ = tokio::signal::ctrl_c() => {
            println!("Got SIGINT; quitting early gracefully");
        }
        r = handle_updates(client.clone(), db, Arc::clone(&feeds)) => {
            match r {
                Ok(_) => println!("Got disconnected from Telegram gracefully"),
                Err(e) => println!("Error during update handling: {}", e),
            }
        }
        _ = handle_feed(client.clone(), Arc::clone(&feeds)) => {
            println!("Failed to check feed");
        }
    );

    client.session().save_to_file(SESSION_NAME).unwrap();
}
