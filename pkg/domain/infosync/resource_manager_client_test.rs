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
        receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
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
        receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
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
        receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
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
