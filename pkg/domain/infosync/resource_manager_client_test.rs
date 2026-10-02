// Copyright 2026 AsterSQL.

use std::time::Duration;

use super::*;

fn resource_group(name: &str, fill_rate: i64, burst_limit: i64, priority: u32) -> ResourceGroup {
    ResourceGroup {
        Name: name.into(),
        RUSettings: TokenLimitSettings {
            FillRate: fill_rate,
            BurstLimit: burst_limit,
        },
        Priority: priority,
    }
}

#[test]
fn default_group_matches_go_mock() {
    let client = NewMockResourceManagerClient(42);

    let group = client
        .get_resource_group(DefaultResourceGroupName)
        .expect("the default resource group must exist");
    assert_eq!(
        group,
        resource_group(DefaultResourceGroupName, i32::MAX as i64, -1, 8)
    );
    assert_eq!(client.list_resource_groups(), vec![group]);
}

#[test]
fn crud_and_watch_events_match_go_mock() {
    let keyspace_id = 7;
    let client = NewMockResourceManagerClient(keyspace_id);
    assert!(client.watch(b"/resource_group/settings/8").is_none());
    let receiver = client
        .watch(&group_settings_path_prefix(keyspace_id))
        .expect("the matching keyspace prefix must be watchable");

    let original = resource_group("analytics", 100, 200, 3);
    assert_eq!(
        client.add_resource_group(original.clone()).unwrap(),
        "Success!"
    );
    assert_eq!(client.get_resource_group("analytics").unwrap(), original);
    assert_eq!(
        receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .Events,
        vec![ResourceGroupEvent {
            event_type: EventType::Put,
            group: original,
        }]
    );

    let modified = resource_group("analytics", 300, -1, 5);
    assert_eq!(
        client.modify_resource_group(modified.clone()).unwrap(),
        "Success!"
    );
    assert_eq!(client.get_resource_group("analytics").unwrap(), modified);
    assert_eq!(
        receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .Events,
        vec![ResourceGroupEvent {
            event_type: EventType::Put,
            group: modified.clone(),
        }]
    );

    assert_eq!(
        client.delete_resource_group("analytics").unwrap(),
        "Success!"
    );
    assert!(client.get_resource_group("analytics").is_err());
    assert_eq!(
        receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .Events,
        vec![ResourceGroupEvent {
            event_type: EventType::Delete,
            group: modified,
        }]
    );
}

#[test]
fn duplicate_add_and_missing_entries_preserve_state() {
    let client = NewMockResourceManagerClient(0);
    let group = resource_group("duplicate", 10, 20, 1);
    client.add_resource_group(group.clone()).unwrap();

    let duplicate_error = client.add_resource_group(group.clone()).unwrap_err();
    assert!(duplicate_error.to_string().contains("already exists"));
    assert_eq!(client.get_resource_group("duplicate").unwrap(), group);

    let get_error = client.get_resource_group("missing").unwrap_err();
    assert!(get_error.to_string().contains("does not exist"));
    assert!(client.delete_resource_group("missing").is_err());
    assert_eq!(client.list_resource_groups().len(), 2);
}

#[test]
fn watch_envelope_retains_pre_subscription_events() {
    let client = NewMockResourceManagerClient(7);
    let original = resource_group("queued", 10, -1, 8);
    client.add_resource_group(original.clone()).unwrap();
    let receiver = client.watch(&group_settings_path_prefix(7)).unwrap();
    let response = receiver
        .recv_timeout(Duration::from_millis(50))
        .expect("Go constructs a buffered watch channel before any subscription");
    assert_eq!(response.CompactRevision, 0);
    assert_eq!(
        response.Events,
        vec![ResourceGroupEvent {
            event_type: EventType::Put,
            group: original.clone()
        }]
    );
    let modified = resource_group("queued", 20, 30, 4);
    client.modify_resource_group(modified.clone()).unwrap();
    assert_eq!(
        receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .Events,
        vec![ResourceGroupEvent {
            event_type: EventType::Put,
            group: modified.clone()
        }]
    );
    client.delete_resource_group("queued").unwrap();
    assert_eq!(
        receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .Events,
        vec![ResourceGroupEvent {
            event_type: EventType::Delete,
            group: modified
        }]
    );
    assert!(client.watch(&group_settings_path_prefix(8)).is_none());
}

#[test]
fn watch_capacity_and_shared_consumer_semantics() {
    let client: std::sync::Arc<dyn ResourceManagerClient> = NewMockResourceManagerClient(7).into();
    for i in 0..100 {
        client
            .add_resource_group(resource_group(&format!("buffer-{i}"), i, -1, 8))
            .unwrap();
    }
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (finished_tx, finished_rx) = std::sync::mpsc::channel();
    let writer_client = client.clone();
    let writer = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        writer_client
            .add_resource_group(resource_group("buffer-100", 100, -1, 8))
            .unwrap();
        finished_tx.send(()).unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(finished_rx.recv_timeout(Duration::from_millis(30)).is_err());
    let first = client.watch(&group_settings_path_prefix(7)).unwrap();
    let second = client.watch(&group_settings_path_prefix(7)).unwrap();
    assert_eq!(
        first.recv_timeout(Duration::from_secs(1)).unwrap().Events[0]
            .group
            .Name,
        "buffer-0"
    );
    finished_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    writer.join().unwrap();
    // Watch returns the same Go channel, so the other consumer takes the next item.
    assert_eq!(
        second.recv_timeout(Duration::from_secs(1)).unwrap().Events[0]
            .group
            .Name,
        "buffer-1"
    );
    for i in 2..=100 {
        assert_eq!(
            first.recv_timeout(Duration::from_secs(1)).unwrap().Events[0]
                .group
                .Name,
            format!("buffer-{i}")
        );
    }
}
