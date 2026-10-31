//! Fetches a feed and shows how it is read, without storing anything:
//!
//!     cargo run -p calendar-ics --example probe -- 'webcal://example.org/feed.ics'

use calendar_caldav::ICalendarCodec;
use calendar_ics::{IcsProvider, feed_url};
use calendar_provider_api::ObjectCodec;

#[tokio::main]
async fn main() {
    let Some(address) = std::env::args().nth(1) else {
        eprintln!("usage: probe <feed address>");
        std::process::exit(2);
    };
    let url = feed_url(&address).unwrap_or_else(|e| panic!("{e}"));
    let feed = IcsProvider::new(url, "probe")
        .unwrap()
        .probe()
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    println!(
        "name: {:?}, colour: {:?}, objects: {}",
        feed.name,
        feed.color,
        feed.objects.len()
    );
    let (mut events, mut failed, mut approximate) = (0, 0, 0);
    for object in &feed.objects {
        match ICalendarCodec.read(&object.data) {
            Ok(read) => {
                events += usize::from(read.content.master.is_some()) + read.content.overrides.len();
                approximate += usize::from(!read.warnings.is_empty());
            }
            Err(e) => {
                failed += 1;
                eprintln!("unreadable {}: {e}", object.href);
            }
        }
    }
    println!("events read: {events}, unreadable objects: {failed}, approximate: {approximate}");
}
