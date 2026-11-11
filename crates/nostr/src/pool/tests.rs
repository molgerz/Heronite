use std::time::{Duration, Instant};

use nostr::prelude::{EventBuilder, FinalizeEvent, Keys, Kind};

use super::*;
use crate::relay::{fetch, publish_once};
use crate::testing::MockRelay;

fn note(keys: &Keys, text: &str) -> Event {
    EventBuilder::new(Kind::TextNote, text).finalize(keys).unwrap()
}

/// The next pool event that `pick` accepts, within a few seconds.
fn wait_for<T>(rx: &Receiver<PoolEvent>, mut pick: impl FnMut(PoolEvent) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(ev) = rx.recv_timeout(Duration::from_millis(100)) {
            if let Some(t) = pick(ev) {
                return t;
            }
        }
    }
    panic!("timed out");
}

#[test]
fn publishes_subscribes_and_passes_each_event_on_once() {
    let (a, b) = (MockRelay::start(), MockRelay::start());
    let (mut pool, rx) = Pool::new();
    pool.set_relays(&[(a.url.clone(), true, true), (b.url.clone(), true, true)]);
    let keys = Keys::generate();
    pool.subscribe("notes", vec![Filter::new().kind(Kind::TextNote).author(keys.public_key())]);
    for _ in 0..2 {
        wait_for(&rx, |e| matches!(e, PoolEvent::EndOfStored { .. }).then_some(()));
    }
    let event = note(&keys, "hello");
    assert_eq!(pool.publish(&event), 2);
    // Both relays answer OK; the event itself comes out only once.
    let (mut oks, mut seen) = (0, Vec::new());
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(PoolEvent::Published { accepted: true, .. }) => oks += 1,
            Ok(PoolEvent::Event { event, subscription, .. }) if subscription == "notes" => seen.push(event.id),
            _ => {}
        }
    }
    assert_eq!(seen, vec![event.id]);
    assert_eq!(oks, 2);
    assert_eq!(a.stored().len(), 1);
    assert_eq!(b.stored().len(), 1);
    pool.stop();
}

#[test]
fn read_only_relays_get_no_events_and_removed_relays_close() {
    let (a, b) = (MockRelay::start(), MockRelay::start());
    let (mut pool, rx) = Pool::new();
    pool.set_relays(&[(a.url.clone(), true, true), (b.url.clone(), true, false)]);
    for _ in 0..2 {
        wait_for(&rx, |e| matches!(e, PoolEvent::Status { status: RelayStatus::Connected, .. }).then_some(()));
    }
    let keys = Keys::generate();
    assert_eq!(pool.publish(&note(&keys, "x")), 1);
    wait_for(&rx, |e| matches!(e, PoolEvent::Published { .. }).then_some(()));
    assert!(b.stored().is_empty());
    pool.set_relays(&[(b.url.clone(), true, true)]);
    assert_eq!(pool.relays(), vec![b.url.clone()]);
    assert_eq!(pool.publish(&note(&keys, "y")), 1);
    wait_for(&rx, |e| matches!(e, PoolEvent::Published { .. }).then_some(()));
    assert_eq!(a.stored().len(), 1);
    assert_eq!(b.stored().len(), 1);
}

#[test]
fn an_unreachable_relay_reports_failed() {
    let (mut pool, rx) = Pool::new();
    // Nothing listens on port 1 of the loopback.
    pool.set_relays(&[("ws://127.0.0.1:1".into(), true, true)]);
    let text = wait_for(&rx, |e| match e {
        PoolEvent::Status { status: RelayStatus::Failed(t), .. } => Some(t),
        _ => None,
    });
    assert!(!text.is_empty());
    pool.stop();
}

#[test]
fn one_shot_fetch_and_publish() {
    let (a, b) = (MockRelay::start(), MockRelay::start());
    let keys = Keys::generate();
    let event = note(&keys, "once");
    let urls = vec![a.url.clone(), b.url.clone(), "ws://127.0.0.1:1".to_string()];
    let report = publish_once(&urls, &event, Duration::from_secs(5));
    assert_eq!(report.iter().filter(|p| p.result.is_ok()).count(), 2);
    let found = fetch(&urls, vec![Filter::new().author(keys.public_key())], Duration::from_secs(5));
    assert_eq!(found.len(), 1, "the same event from two relays is one");
    assert_eq!(found[0].id, event.id);
}
