pub static WELCOME: &str = r#"Hi, I'm srsrssrs, a serious RSS Rust bot. Sorry if it gave you a stroke to read that.

To get started, /add <FEED URL>. If you get tired of the feed, use /rm <FEED URL>. You can view what feeds you're subscribed to with /ls."#;

pub static NO_URL: &str = "You need to include a (valid) URL after the command.";

pub static NO_FEEDS: &str = "You're not subscribed to any feeds. Here's a good one you could try (wink, wink): https://lonami.dev/blog/atom.xml";

pub fn try_add(url: &str) -> String {
    format!("Trying to add {}...", url)
}

pub fn add_ok(url: &str) -> String {
    format!("Added {} to your list of feeds.", url)
}

pub fn add_err(url: &str, e: crate::feed::Error) -> String {
    format!("Failed to add {} to your list of feeds: {}.", url, e)
}

pub fn del_ok(url: &str) -> String {
    format!("You will no longer receive updates from {}.", url)
}

pub fn del_err(url: &str) -> String {
    format!("You were not subscribed to {}!", url)
}

pub fn feed_list(feeds: &[String]) -> String {
    if feeds.is_empty() {
        return NO_FEEDS.to_string();
    }

    let mut result = "These are your feeds:".to_string();
    feeds.iter().for_each(|feed| {
        result.push_str("\n• ");
        result.push_str(feed);
    });
    result
}

pub fn new_entry(feed: &feed_rs::model::Entry) -> String {
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
