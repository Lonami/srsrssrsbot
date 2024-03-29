mod db;
mod feed;
mod string;

use grammers_client::client::chats::InvocationError;
use grammers_client::types::{Chat, Message};
use grammers_client::{Client, Config, Update};
use grammers_session::Session;
use log::{self, info, warn};
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

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn parse_url(url: Option<&str>) -> Option<&str> {
    let url = match url {
        Some(url) => url,
        None => return None,
    };

    let lower = url.to_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return None;
    }

    // Quick sanity check. Yes there are ways around this. But this should prevent dumb attempts.
    // More funny numbers https://daniel.haxx.se/blog/2021/04/19/curl-those-funny-ipv4-addresses/.
    if lower.starts_with("http://localhost") || lower.starts_with("http://127.0.0.1") {
        return None;
    }

    let mut end = url.len();
    end = end.min(url.find('#').unwrap_or(end));
    end = end.min(url.find('?').unwrap_or(end));
    Some(&url[..end])
}

async fn handle_updates(mut tg: Client, db: &db::Database) -> Result<()> {
    let http = reqwest::Client::new();

    while let Some(update) = tg.next_update().await? {
        match update {
            Update::NewMessage(message)
                if !message.outgoing() && matches!(message.chat(), Chat::User(_)) =>
            {
                match handle_message(&mut tg, &http, &db, &message).await {
                    Ok(_) => {}
                    Err(err) => match err.downcast::<InvocationError>() {
                        Ok(err) => match *err {
                            InvocationError::Rpc(rpc) if rpc.name == "USER_IS_BLOCKED" => {}
                            InvocationError::Rpc(rpc) => {
                                info!("failed to react in {}: {}", message.chat().pack(), rpc)
                            }
                            _ => warn!("failed to react in {}: {}", message.chat().pack(), err),
                        },
                        Err(err) => return Err(err),
                    },
                };
            }
            _ => {}
        };
    }

    Ok(())
}

async fn handle_message(
    tg: &mut Client,
    http: &reqwest::Client,
    db: &db::Database,
    message: &Message,
) -> Result<()> {
    let cmd = match message.text().split_whitespace().next() {
        Some(cmd) => cmd,
        None => return Ok(()),
    };

    if cmd == "/start" || cmd == "/help" {
        tg.send_message(&message.chat(), string::WELCOME)
            .await?;
    } else if cmd == "/add" {
        if let Some(url) = parse_url(message.text().split_whitespace().nth(1)) {
            let sent = tg
                .send_message(&message.chat(), string::try_add(url))
                .await?;

            let user = message.sender().unwrap().pack();
            let err = if db.try_add_subscriber(url, &user)? {
                None
            } else {
                match feed::Feed::new(&http, url, user).await {
                    Ok(feed) => {
                        db.add_feed(&feed)?;
                        None
                    }
                    Err(e) => Some(e),
                }
            };

            if let Some(err) = err {
                sent.edit(string::add_err(url, err)).await?;
            } else {
                sent.edit(string::add_ok(url)).await?;
            }
        } else {
            tg.send_message(&message.chat(), string::NO_URL)
                .await?;
        }
    } else if cmd == "/rm" || cmd == "/del" {
        let msg = if let Some(url) = parse_url(message.text().split_whitespace().nth(1)) {
            let user = message.sender().unwrap().pack();
            if db.try_del_subscriber(url, &user)? {
                string::del_ok(url)
            } else {
                string::del_err(url)
            }
        } else {
            string::NO_URL.to_string()
        };

        tg.send_message(&message.chat(), msg).await?;
    } else if cmd == "/ls" || cmd == "/list" {
        let feeds = db.get_user_feeds(&message.sender().unwrap().pack())?;

        tg.send_message(&message.chat(), string::feed_list(&feeds))
            .await?;
    }

    Ok(())
}

async fn handle_feed(tg: Client, db: &db::Database) -> Result<()> {
    let http = reqwest::Client::new();
    let mut last_save_failed = false;

    loop {
        let feeds = db.load_pending_feeds()?;
        let mut updated_feeds = Vec::with_capacity(feeds.len());

        for mut feed in feeds {
            let entries = match feed.check(&http).await {
                Ok(entries) => entries,
                Err(err) => {
                    warn!("failed to fetch {}: {}", feed.url, err);
                    feed.reset_expiry();
                    updated_feeds.push(feed);
                    continue;
                }
            };

            for entry in entries.iter() {
                let mut fail_count = 0;
                for user in feed.users.iter() {
                    match tg
                        .send_message(*user, string::new_entry(entry))
                        .await
                    {
                        Ok(_) => {}
                        Err(InvocationError::Rpc(rpc)) if rpc.name == "USER_IS_BLOCKED" => {}
                        Err(InvocationError::Rpc(rpc)) => {
                            fail_count += 1;
                            info!(
                                "failed to notify {} about {}/{}: {}",
                                user, feed.url, entry.id, rpc
                            );
                        }
                        Err(err) => {
                            fail_count += 1;
                            warn!(
                                "failed to notify {} about {}/{}: {}",
                                user, feed.url, entry.id, err
                            );
                        }
                    };
                }

                if fail_count == feed.users.len() {
                    warn!(
                        "failed to notify all {} users about {}/{}",
                        feed.users.len(),
                        feed.url,
                        entry.id
                    );
                    feed.reset_entries(&entries);
                    break;
                }
            }

            updated_feeds.push(feed);
        }

        match db.update_feeds_and_entries(&updated_feeds) {
            Ok(_) => last_save_failed = false,
            Err(e) => {
                warn!("failed to store updated feeds: {}", e);
                if last_save_failed {
                    panic!("failed to store updated feeds twice in a row");
                } else {
                    last_save_failed = true;
                }
            }
        }
        sleep(FETCH_FEEDS_DELAY).await;
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let db = db::Database::new(DB_NAME)?;
    db.cleanup_feeds()?;

    SimpleLogger::new()
        .with_level(match LOG_LEVEL {
            "ERROR" => log::LevelFilter::Error,
            "WARN" => log::LevelFilter::Warn,
            "INFO" => log::LevelFilter::Info,
            "DEBUG" => log::LevelFilter::Debug,
            "TRACE" => log::LevelFilter::Trace,
            _ => log::LevelFilter::Off,
        })
        .init()?;

    let api_id = TG_API_ID.parse()?;
    let client = Client::connect(Config {
        session: Session::load_file_or_create(SESSION_NAME)?,
        api_id,
        api_hash: TG_API_HASH.to_string(),
        params: Default::default(),
    })
    .await?;

    if !client.is_authorized().await? {
        client.bot_sign_in(BOT_TOKEN, api_id, TG_API_HASH).await?;
        client.session().save_to_file(SESSION_NAME)?;
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

    client.session().save_to_file(SESSION_NAME)?;
    Ok(())
}
