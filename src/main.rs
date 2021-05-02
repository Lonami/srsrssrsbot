mod db;
mod feed;

use grammers_client::client::chats::InvocationError;
use grammers_client::types::{Chat, Message};
use grammers_client::{Client, Config, Update};
use grammers_session::Session;
use log;
use simple_logger::SimpleLogger;
use std::time::Duration;
use tokio::time::sleep;

/// How long to sleep before attempting to check which feeds we need to refetch.
const FETCH_FEEDS_DELAY: Duration = Duration::from_secs(60);

static LOG_LEVEL: &str = env!("LOG_LEVEL");

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

async fn handle_updates(mut tg: Client, db: &db::Database) -> Result<(), InvocationError> {
    let http = reqwest::Client::new();

    while let Some(updates) = tg.next_updates().await? {
        for update in updates {
            match update {
                Update::NewMessage(message)
                    if !message.outgoing() && matches!(message.chat(), Chat::User(_)) =>
                {
                    handle_message(&mut tg, &http, &db, message).await?;
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

            let user = message.sender().unwrap().pack();
            let err = if db.try_add_subscriber(url, &user).unwrap() {
                None
            } else {
                match feed::Feed::new(&http, url, user).await {
                    Ok(feed) => {
                        db.add_feed(&feed).unwrap();
                        None
                    }
                    Err(e) => Some(e),
                }
            };

            if let Some(err) = err {
                sent.edit(str_add_err(url, err).into()).await.unwrap();
            } else {
                sent.edit(str_add_ok(url).into()).await.unwrap();
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

async fn handle_feed(mut tg: Client, db: &db::Database) {
    let http = reqwest::Client::new();

    loop {
        let feeds = db.load_pending_feeds().unwrap();
        for mut feed in feeds {
            for entry in feed.check(&http).await.unwrap() {
                for user in feed.users.iter() {
                    tg.send_message(&user.unpack(), str_new_entry(&entry).into())
                        .await
                        .unwrap();
                }
            }
        }

        sleep(FETCH_FEEDS_DELAY).await;
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
        r = handle_updates(client.clone(), &db) => {
            match r {
                Ok(_) => println!("Got disconnected from Telegram gracefully"),
                Err(e) => println!("Error during update handling: {}", e),
            }
        }
        _ = handle_feed(client.clone(), &db) => {
            println!("Failed to check feed");
        }
    );

    client.session().save_to_file(SESSION_NAME).unwrap();
}
