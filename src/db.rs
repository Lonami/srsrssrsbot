use crate::feed::Feed;
use chrono::{TimeZone, Utc};
use sqlite::State;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;

const VERSION: i64 = 1;

#[derive(Clone)]
pub struct Database(Arc<Mutex<sqlite::Connection>>);

impl Database {
    pub fn new(name: &str) -> sqlite::Result<Self> {
        let conn = sqlite::open(name)?;
        conn.execute("PRAGMA foreign_keys = ON")?;

        let version = match conn.prepare("SELECT version FROM version") {
            Ok(mut stmt) => {
                assert_eq!(State::Row, stmt.next().unwrap());
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

        conn.execute("BEGIN")?;
        conn.execute(
            "CREATE TABLE version (
            version INTEGER NOT NULL)",
        )?;
        {
            let mut stmt = conn.prepare("INSERT INTO version (version) VALUES (?)")?;
            stmt.bind(1, VERSION).unwrap();
            while stmt.next()? != State::Done {}
        }
        conn.execute(
            "CREATE TABLE feed (
            id INTEGER PRIMARY KEY,
            url TEXT NOT NULL UNIQUE ON CONFLICT REPLACE,
            last_check INTEGER NOT NULL,
            next_check INTEGER NOT NULL,
            etag TEXT)",
        )?;
        conn.execute(
            "CREATE TABLE entry (
            feed_id INTEGER NOT NULL REFERENCES feed (id) ON DELETE CASCADE,
            entry_id TEXT NOT NULL,
            CONSTRAINT non_dup_entries_con UNIQUE (feed_id, entry_id) ON CONFLICT IGNORE)",
        )?;
        conn.execute(
            "CREATE TABLE subscriber (
            feed_id INTEGER NOT NULL REFERENCES feed (id),
            user NOT NULL,
            CONSTRAINT one_sub_per_feed_con UNIQUE (feed_id, user) ON CONFLICT IGNORE)",
        )?;
        conn.execute("COMMIT")?;
        Ok(Self(Arc::new(Mutex::new(conn))))
    }

    pub fn add_feed(&self, feed: &Feed) -> sqlite::Result<()> {
        let conn = self.0.lock().unwrap();
        conn.execute("BEGIN")?;
        {
            let mut stmt = conn.prepare(
                "INSERT INTO feed (url, last_check, next_check, etag) VALUES (?, ?, ?, ?)",
            )?;

            stmt.bind::<&str>(1, feed.url.as_ref())?;
            stmt.bind(2, feed.last_fetch.timestamp())?;
            stmt.bind(3, feed.next_fetch_timestamp())?;
            stmt.bind::<Option<&str>>(4, feed.etag.as_deref())?;
            while stmt.next()? != State::Done {}

            stmt = conn.prepare("SELECT last_insert_rowid()")?;
            assert_eq!(State::Row, stmt.next()?);
            let feed_id = stmt.read::<i64>(0)?;

            for entry_id in feed.seen_entries.iter() {
                stmt = conn.prepare("INSERT INTO entry (feed_id, entry_id) VALUES (?, ?)")?;
                stmt.bind(1, feed_id)?;
                stmt.bind::<&str>(2, entry_id)?;
                while stmt.next()? != State::Done {}
            }
        }
        conn.execute("COMMIT")?;
        Ok(())
    }

    pub fn cleanup_feeds(&self) -> sqlite::Result<()> {
        let conn = self.0.lock().unwrap();
        conn.execute(
            "DELETE FROM feed AS f WHERE NOT EXISTS (
                SELECT * FROM subscriber AS s WHERE s.feed_id = f.id
            )",
        )
    }

    pub fn load_feeds(&self) -> sqlite::Result<BinaryHeap<Feed>> {
        let conn = self.0.lock().unwrap();
        let mut feeds = HashMap::<i64, Feed>::new();

        let mut stmt = conn.prepare(
            "SELECT id, url, last_check, next_check, etag, entry_id
            FROM feed JOIN entry ON (id = feed_id)",
        )?;
        while stmt.next()? == State::Row {
            feeds.entry(stmt.read(0)?).or_insert_with(|| Feed {
                url: stmt.read(1).unwrap(),
                users: Vec::new(),
                seen_entries: HashSet::new(),
                last_fetch: Utc.timestamp(stmt.read(2).unwrap(), 0),
                next_fetch: {
                    let due = stmt.read::<i64>(3).unwrap();
                    let now = Utc::now().timestamp();
                    let delta = due - now;
                    if delta < 0 {
                        Instant::now() - Duration::from_secs(-delta as u64)
                    } else {
                        Instant::now() + Duration::from_secs(delta as u64)
                    }
                },
                etag: stmt.read(4).unwrap(),
            });
        }

        Ok(feeds.into_iter().map(|(_, v)| v).collect())
    }

    /*
    pub fn add_subscriber(&self, user: PackedChat) -> sqlite::Result<()> {
        Ok(())
    }

    pub fn del_subscriber(&self, user: PackedChat) -> sqlite::Result<()> {
        Ok(())
    }
    */
}
