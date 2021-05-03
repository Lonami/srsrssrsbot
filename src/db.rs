use crate::feed::Feed;
use chrono::{TimeZone, Utc};
use grammers_client::types::chat::PackedChat;
use sqlite::State;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;

const VERSION: i64 = 1;

#[derive(Clone)]
pub struct Database(Arc<Mutex<sqlite::Connection>>);

/// Helper macro to avoid the annoying `prepare` statements and `bind`.
///
/// # Examples
///
/// Simplest case:
///
/// ```
/// query!(conn."BEGIN");
/// ```
///
/// With parameters:
///
/// ```
/// query!(conn."DELETE FROM table WHERE column = ?"(value));
/// ```
///
/// Selection:
///
/// ```
/// query!(for (field: i64) in conn."SELECT * FROM table WHERE column = ?"(value) {
///     dbg!(field);
/// });
/// ```
///
/// Fetch one:
///
/// ```
/// return query!(fetch (field: i64) in conn."SELECT * FROM table WHERE column = ?"(value)).unwrap();
/// ```
macro_rules! query {
    ($conn:ident . $query:literal) => {{
        $conn.execute($query)?;
    }};
    ($conn:ident . $query:literal ($($arg:expr),+)) => {{
        let mut stmt = $conn.prepare($query)?;
        let mut i = 1;
        $(
            stmt.bind(i, $arg)?;
            i += 1;
        )+
        let _ = i;
        while stmt.next()? != State::Done {}
    }};
    (for ($($bind:ident : $ty:ty),+) in $conn:ident . $query:literal ($($arg:expr),*) $body:block) => {{
        let mut stmt = $conn.prepare($query)?;
        let mut i = 1;
        $(
            stmt.bind(i, $arg)?;
            i += 1;
        )*
        let _ = i;
        while stmt.next()? == State::Row {
            i = 0;
            $(
                let $bind = stmt.read::<$ty>(i)?;
                i += 1;
            )+
            let _ = i;
            $body
        }
    }};
    (fetch ($($bind:ident : $ty:ty),+) in $conn:ident . $query:literal ($($arg:expr),*)) => {{
        let mut stmt = $conn.prepare($query)?;
        let mut i = 1;
        $(
            stmt.bind(i, $arg)?;
            i += 1;
        )*
        let _ = i;
        if stmt.next()? == State::Row {
            i = 0;
            $(
                let $bind = stmt.read::<$ty>(i)?;
                i += 1;
            )+
            let _ = i;
            Some(($($bind),+))
        } else {
            None
        }
    }};
}

impl Database {
    pub fn new(name: &str) -> sqlite::Result<Self> {
        let conn = sqlite::open(name)?;
        query!(conn."PRAGMA foreign_keys = ON");

        let version = match conn.prepare("SELECT version FROM version") {
            Ok(mut stmt) => {
                assert_eq!(State::Row, stmt.next()?);
                stmt.read(0)?
            }
            Err(err) => {
                if err
                    .message
                    .as_ref()
                    .filter(|m| m.starts_with("no such table"))
                    .is_some()
                {
                    0
                } else {
                    return Err(err);
                }
            }
        };

        assert!(
            version <= VERSION,
            "tried to load a database which is too new"
        );

        if version == VERSION {
            return Ok(Self(Arc::new(Mutex::new(conn))));
        }

        query!(conn."BEGIN");
        query!(conn.
            "CREATE TABLE version (
            version INTEGER NOT NULL)"
        );
        query!(conn."INSERT INTO version (version) VALUES (?)"(VERSION));
        query!(conn.
            "CREATE TABLE feed (
            id INTEGER PRIMARY KEY,
            url TEXT NOT NULL UNIQUE ON CONFLICT REPLACE,
            last_check INTEGER NOT NULL,
            next_check INTEGER NOT NULL,
            etag TEXT)"
        );
        query!(conn.
            "CREATE TABLE entry (
            feed_id INTEGER NOT NULL REFERENCES feed (id) ON DELETE CASCADE,
            entry_id TEXT NOT NULL,
            CONSTRAINT non_dup_entries_con UNIQUE (feed_id, entry_id) ON CONFLICT IGNORE)"
        );
        query!(conn.
            "CREATE TABLE subscriber (
            feed_id INTEGER NOT NULL REFERENCES feed (id) ON DELETE CASCADE,
            user NOT NULL,
            CONSTRAINT one_sub_per_feed_con UNIQUE (feed_id, user) ON CONFLICT IGNORE)"
        );
        query!(conn."COMMIT");
        Ok(Self(Arc::new(Mutex::new(conn))))
    }

    pub fn add_feed(&self, feed: &Feed) -> sqlite::Result<()> {
        let conn = self.0.lock().unwrap();
        query!(conn."BEGIN");
        {
            query!(conn."INSERT INTO feed (url, last_check, next_check, etag) VALUES (?, ?, ?, ?)"(
                feed.url.as_str(), feed.last_fetch.timestamp(), feed.next_fetch_timestamp(), feed.etag.as_deref()
            ));
            let feed_id = query!(fetch (id: i64) in conn."SELECT last_insert_rowid()"()).unwrap();

            for entry_id in feed.seen_entries.iter() {
                query!(conn."INSERT INTO entry (feed_id, entry_id) VALUES (?, ?)"(feed_id, entry_id.as_str()));
            }
            for sub in feed.users.iter() {
                query!(conn."INSERT INTO subscriber (feed_id, user) VALUES (?, ?)"(
                    feed_id, sub.to_bytes().as_slice()
                ));
            }
        }
        query!(conn."COMMIT");
        Ok(())
    }

    pub fn update_feeds_and_entries(&self, feeds: &[Feed]) -> sqlite::Result<()> {
        let conn = self.0.lock().unwrap();
        query!(conn."BEGIN");
        for feed in feeds {
            let feed_id = match query!(fetch (id: i64) in conn."SELECT id FROM feed WHERE url = ?"(feed.url.as_str()))
            {
                Some(id) => id,
                None => continue,
            };
            query!(conn."UPDATE feed SET last_check = ?, next_check = ?, etag = ? WHERE id = ?"(
                feed.last_fetch.timestamp(), feed.next_fetch_timestamp(), feed.etag.as_deref(), feed_id
            ));
            for entry_id in feed.seen_entries.iter() {
                query!(conn."INSERT INTO entry (feed_id, entry_id) VALUES (?, ?)"(feed_id, entry_id.as_str()));
            }
        }
        query!(conn."COMMIT");
        Ok(())
    }

    pub fn cleanup_feeds(&self) -> sqlite::Result<()> {
        let conn = self.0.lock().unwrap();
        query!(conn."DELETE FROM feed AS f WHERE NOT EXISTS (
            SELECT * FROM subscriber AS s WHERE s.feed_id = f.id)");
        Ok(())
    }

    pub fn load_pending_feeds(&self) -> sqlite::Result<BinaryHeap<Feed>> {
        let conn = self.0.lock().unwrap();
        let mut feeds = HashMap::<i64, Feed>::new();
        let now = Utc::now().timestamp();

        query!(for (id: i64, url: String, last_check: i64, next_fetch: i64, etag: Option<String>)
                in conn."SELECT id, url, last_check, next_check, etag FROM feed WHERE next_check < ?"(now) {
            feeds.entry(id).or_insert_with(|| Feed {
                url,
                users: Vec::new(),
                seen_entries: HashSet::new(),
                last_fetch: Utc.timestamp(last_check, 0),
                next_fetch: {
                    let now = Utc::now().timestamp();
                    let delta = next_fetch - now;
                    if delta < 0 {
                        Instant::now() - Duration::from_secs(-delta as u64)
                    } else {
                        Instant::now() + Duration::from_secs(delta as u64)
                    }
                },
                etag,
            });
        });

        query!(for (id: i64, entry: String)
                in conn."SELECT id, entry_id FROM feed JOIN entry ON (id = feed_id) WHERE next_check < ?"(now) {
            if let Some(feed) = feeds.get_mut(&id) {
                feed.seen_entries.insert(entry);
            }
        });

        query!(for (id: i64, user: Vec<u8>)
                in conn."SELECT id, user FROM feed JOIN subscriber ON (id = feed_id) WHERE next_check < ?"(now) {
            if let Some(feed) = feeds.get_mut(&id) {
                feed.users
                    .push(PackedChat::from_bytes(&user).unwrap());
            }
        });

        Ok(feeds.into_iter().map(|(_, v)| v).collect())
    }

    pub fn try_add_subscriber(&self, url: &str, user: &PackedChat) -> sqlite::Result<bool> {
        let conn = self.0.lock().unwrap();
        if let Some(feed_id) =
            query!(fetch (id: i64) in conn."SELECT id FROM feed WHERE url = ?"(url))
        {
            query!(conn."INSERT INTO subscriber (feed_id, user) VALUES (?, ?)"(feed_id, user.to_bytes().as_slice()));
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn try_del_subscriber(&self, url: &str, user: &PackedChat) -> sqlite::Result<bool> {
        let conn = self.0.lock().unwrap();
        query!(conn."DELETE FROM subscriber WHERE user = ? AND feed_id = (
            SELECT id FROM feed WHERE url = ?
        )"(user.to_bytes().as_slice(), url));
        if let Some(count) = query!(fetch (count: i64) in conn."SELECT changes()"()) {
            Ok(count == 1)
        } else {
            Ok(false)
        }
    }

    pub fn get_user_feeds(&self, user: &PackedChat) -> sqlite::Result<Vec<String>> {
        let conn = self.0.lock().unwrap();
        let mut result = Vec::new();
        query!(for (url: String)
                in conn."SELECT url FROM feed AS f
                    JOIN subscriber AS s ON (f.id = s.feed_id)
                    WHERE s.user = ?"(user.to_bytes().as_slice()) {
            result.push(url);
        });
        Ok(result)
    }
}
