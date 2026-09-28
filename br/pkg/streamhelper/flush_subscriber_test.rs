// Copyright 2026 AsterSQL.

use crate::basic_lib_for_test::{
    create_fake_cluster, install_subscribe_support, one_store_failure,
};
use crate::flush_subscriber::NewSubscriber;
use crate::flush_subscriber::WithSubscriptionIdleTimeout;
use std::time::Duration;

#[test]
fn connection_errors_are_pending_and_retryable() {
    let c = create_fake_cluster(1, true);
    install_subscribe_support(&c);
    c.cluster.set_on_get_client(Some(one_store_failure()));

    let mut subscriber = NewSubscriber(c.clone(), Vec::new());
    subscriber.UpdateStoreTopology().unwrap();
    assert_eq!(subscriber.SubscriptionCount(), 1);
    assert!(subscriber.PendingErrors().is_err());

    c.cluster.set_on_get_client(None);
    subscriber.HandleErrors();
    assert!(subscriber.PendingErrors().is_ok());
}

#[test]
fn drop_closes_the_event_tunnel() {
    let c = create_fake_cluster(1, true);
    let mut subscriber = NewSubscriber(c, Vec::new());
    let rx = subscriber.TakeEventsRx().expect("event receiver");

    subscriber.Drop();
    assert!(rx.recv().is_err());
}

#[test]
fn live_flush_stream_forwards_events() {
    let c = create_fake_cluster(1, true);
    install_subscribe_support(&c);
    let mut subscriber = NewSubscriber(c.clone(), Vec::new());
    let rx = subscriber.TakeEventsRx().expect("event receiver");
    subscriber.UpdateStoreTopology().unwrap();

    let checkpoint = c.cluster.advance_checkpoints();
    c.cluster.flush_all();
    let event = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("flush event");
    assert_eq!(event.Value, checkpoint);
}

#[test]
fn idle_stream_records_retryable_error() {
    let c = create_fake_cluster(1, true);
    install_subscribe_support(&c);
    let mut subscriber = NewSubscriber(
        c,
        vec![WithSubscriptionIdleTimeout(Duration::from_millis(20))],
    );
    subscriber.UpdateStoreTopology().unwrap();
    std::thread::sleep(Duration::from_millis(80));
    assert!(
        subscriber
            .PendingErrors()
            .unwrap_err()
            .contains("no activity")
    );
}
