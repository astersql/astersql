// Copyright 2026 AsterSQL.

use super::resource_group::{
    GroupState, ResourceGroupCatalog, ResourceGroupError, ResourceGroupInfo, ResourceGroupManager,
    ResourceGroupOption, ResourceGroupSettings, build_resource_group, parse_background_job_types,
    set_direct_resource_group_settings,
};

#[derive(Default)]
struct RecordingManager {
    fail_modify: bool,
    fail_delete: bool,
    modified: Vec<ResourceGroupInfo>,
}

impl ResourceGroupManager for RecordingManager {
    fn add(&mut self, _: &ResourceGroupInfo) -> Result<(), String> {
        Ok(())
    }

    fn modify(&mut self, group: &ResourceGroupInfo) -> Result<(), String> {
        self.modified.push(group.clone());
        if self.fail_modify {
            Err("modify failed".into())
        } else {
            Ok(())
        }
    }

    fn delete(&mut self, _: &str) -> Result<(), String> {
        if self.fail_delete {
            Err("delete failed".into())
        } else {
            Ok(())
        }
    }
}

fn group(id: i64, priority: u64) -> ResourceGroupInfo {
    ResourceGroupInfo {
        id,
        name: format!("group-{id}"),
        state: GroupState::Public,
        settings: ResourceGroupSettings {
            ru_rate: 100,
            priority,
            ..Default::default()
        },
    }
}

#[test]
fn alter_persists_meta_before_manager_sync_like_go() {
    let mut catalog = ResourceGroupCatalog::default();
    let mut manager = RecordingManager::default();
    catalog.create(group(1, 1), &mut manager).unwrap();

    let mut changed = group(1, 9).settings;
    manager.fail_modify = true;
    assert!(matches!(
        catalog.alter(1, changed.clone(), &mut manager),
        Err(ResourceGroupError::Backend(_))
    ));
    assert_eq!(catalog.get(1).unwrap().settings.priority, 9);

    changed.priority = 10;
    manager.fail_modify = false;
    catalog.alter(1, changed, &mut manager).unwrap();
    assert_eq!(manager.modified.last().unwrap().settings.priority, 10);
    assert_eq!(manager.modified[1].settings.priority, 10);
}

#[test]
fn drop_removes_meta_before_manager_sync_like_go() {
    let mut catalog = ResourceGroupCatalog::default();
    let mut manager = RecordingManager::default();
    catalog.create(group(1, 1), &mut manager).unwrap();

    manager.fail_delete = true;
    assert!(matches!(
        catalog.drop_group(1, &mut manager),
        Err(ResourceGroupError::Backend(_))
    ));
    manager.fail_delete = false;
    assert_eq!(
        catalog.drop_group(1, &mut manager),
        Err(ResourceGroupError::NotFound)
    );
}

#[test]
fn build_resets_state_and_does_not_add_validation_absent_from_go() {
    let old = group(1, 1);
    let built = build_resource_group(
        &old,
        &[ResourceGroupOption::RuRate {
            rate: 0,
            burstable: super::resource_group::Burstable::Disabled,
        }],
    )
    .unwrap();

    assert_eq!(built.state, GroupState::None);
    assert_eq!(built.settings.ru_rate, 0);
    assert_eq!(built.settings.burst_limit, 0);
}

#[test]
fn background_empty_options_and_explicit_type_allowlist_match_go() {
    let mut default_group = group(1, 1);
    default_group.name = "default".into();
    set_direct_resource_group_settings(&mut default_group, &ResourceGroupOption::Background(None))
        .unwrap();
    assert_eq!(default_group.settings.background, Some(Default::default()));

    assert_eq!(
        parse_background_job_types(" Dumpling, BACKGROUND ").unwrap(),
        ["dumpling", "background"]
    );
    assert!(parse_background_job_types("backup").is_err());
    assert!(parse_background_job_types("restore").is_err());
}

#[test]
fn runaway_durations_accept_go_compound_and_fractional_syntax() {
    let mut default_group = group(1, 1);
    set_direct_resource_group_settings(
        &mut default_group,
        &ResourceGroupOption::Runaway(Some(vec![
            super::resource_group::RunawayOption::ExecElapsed("1h30m".into()),
            super::resource_group::RunawayOption::Watch {
                watch_type: "exact".into(),
                duration: Some("1.5s".into()),
            },
        ])),
    )
    .unwrap();
    let runaway = default_group.settings.runaway.unwrap();
    assert_eq!(runaway.exec_elapsed_ms, 5_400_000);
    assert_eq!(runaway.watch_duration_ms, 1_500);
}
