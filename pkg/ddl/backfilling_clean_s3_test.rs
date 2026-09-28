// Copyright 2026 AsterSQL.

// S3 回填清理模块的云存储 URI 脱敏回归测试。
//
// 重点验证仅替换受支持云协议的敏感查询参数，同时保留用户信息和普通参数，
// 并与 Go 实现对非云协议及非法 URI 的处理保持一致。

use std::cell::RefCell;

use crate::backfilling_clean_s3::{
    BackfillCleanUpS3, CleanupError, CleanupStorage, CleanupTask, MeteringData, TaskState,
    redact_cloud_storage_uri,
};
use crate::backfilling_dist_executor::{BACKFILL_TASK_META_VERSION_1, BackfillTaskMeta};
use crate::backfilling_read_index::SubtaskSummary;

#[derive(Default)]
struct RecordingStorage {
    prefixes: Vec<String>,
    fail_on: Option<String>,
}

impl CleanupStorage for RecordingStorage {
    fn cleanup_prefix(&mut self, prefix: &str) -> Result<(), String> {
        self.prefixes.push(prefix.to_owned());
        if self.fail_on.as_deref() == Some(prefix) {
            return Err(format!("failed to clean {prefix}"));
        }
        Ok(())
    }
}

fn task(version: u32, state: TaskState, merge_temporary_index: bool) -> CleanupTask {
    CleanupTask {
        id: 42,
        state,
        meta: BackfillTaskMeta {
            job_id: 7,
            cloud_storage_uri: "s3://bucket/path?secret-access-key=secret".to_owned(),
            merge_temporary_index,
            version,
            ..BackfillTaskMeta::default()
        },
    }
}

#[test]
/// 验证 S3 密钥别名会被脱敏，而不适用脱敏规则的输入保持原样。
fn redact_cloud_storage_uri_matches_go_url_redaction() {
    assert_eq!(
        redact_cloud_storage_uri(
            "s3://user:password@bucket/path?access-key=secret&other=value&secret_access_key=token"
        ),
        "s3://user:password@bucket/path?access-key=xxxxxx&other=value&secret_access_key=xxxxxx"
    );

    // HTTP URL 与非法 URI 不属于支持的云存储 URI，必须完整保留原输入。
    assert_eq!(
        redact_cloud_storage_uri("https://bucket/path?access-key=secret&other=value"),
        "https://bucket/path?access-key=secret&other=value"
    );
    assert_eq!(redact_cloud_storage_uri("not a URL"), "not a URL");

    // Go's url.Values groups duplicate keys and replaces all secret values with
    // one redaction marker before serializing the sorted query.
    assert_eq!(
        redact_cloud_storage_uri(
            "s3://bucket/path?secret-access-key=first&other=a%20b&secret-access-key=second"
        ),
        "s3://bucket/path?other=a+b&secret-access-key=xxxxxx"
    );
}

#[test]
fn cleanup_matches_go_prefix_metering_and_redaction_order() {
    let mut cleanup_task = task(0, TaskState::Succeed, false);
    let mut storage = RecordingStorage::default();
    let meter = RefCell::new(Vec::new());
    let summaries = [
        SubtaskSummary {
            row_count: 3,
            processed_bytes: 11,
            ..SubtaskSummary::default()
        },
        SubtaskSummary {
            row_count: 5,
            processed_bytes: 13,
            ..SubtaskSummary::default()
        },
    ];

    BackfillCleanUpS3
        .cleanup(&mut cleanup_task, &mut storage, true, &summaries, |data| {
            meter.borrow_mut().push(data);
            Ok(())
        })
        .unwrap();

    assert_eq!(storage.prefixes, ["42", "7"]);
    assert_eq!(
        meter.into_inner(),
        [MeteringData {
            row_count: 8,
            index_kv_size: 24,
        }]
    );
    assert_eq!(
        cleanup_task.meta.cloud_storage_uri,
        "s3://bucket/path?secret-access-key=xxxxxx"
    );
}

#[test]
fn cleanup_short_circuits_errors_and_preserves_unredacted_meta() {
    let mut cleanup_task = task(BACKFILL_TASK_META_VERSION_1, TaskState::Succeed, false);
    let mut storage = RecordingStorage {
        fail_on: Some("42".to_owned()),
        ..RecordingStorage::default()
    };
    let mut meter_called = false;

    let error = BackfillCleanUpS3
        .cleanup(&mut cleanup_task, &mut storage, true, &[], |_| {
            meter_called = true;
            Ok(())
        })
        .unwrap_err();

    assert_eq!(
        error,
        CleanupError::Storage("failed to clean 42".to_owned())
    );
    assert_eq!(storage.prefixes, ["42"]);
    assert!(!meter_called);
    assert_eq!(
        cleanup_task.meta.cloud_storage_uri,
        "s3://bucket/path?secret-access-key=secret"
    );
}

#[test]
fn cleanup_uses_all_go_metering_gates() {
    for (next_generation_kernel, state, merge_temporary_index) in [
        (false, TaskState::Succeed, false),
        (true, TaskState::Pending, false),
        (true, TaskState::Running, false),
        (true, TaskState::Failed, false),
        (true, TaskState::Succeed, true),
    ] {
        let mut cleanup_task = task(BACKFILL_TASK_META_VERSION_1, state, merge_temporary_index);
        let mut storage = RecordingStorage::default();
        let mut meter_called = false;

        BackfillCleanUpS3
            .cleanup(
                &mut cleanup_task,
                &mut storage,
                next_generation_kernel,
                &[],
                |_| {
                    meter_called = true;
                    Ok(())
                },
            )
            .unwrap();

        assert!(!meter_called);
        assert_eq!(storage.prefixes, ["42"]);
    }
}

#[test]
fn cleanup_skips_empty_uri_and_rejects_invalid_uri_before_storage() {
    let mut empty = task(BACKFILL_TASK_META_VERSION_1, TaskState::Succeed, false);
    empty.meta.cloud_storage_uri.clear();
    let mut storage = RecordingStorage::default();
    let mut meter_called = false;
    BackfillCleanUpS3
        .cleanup(&mut empty, &mut storage, true, &[], |_| {
            meter_called = true;
            Ok(())
        })
        .unwrap();
    assert!(storage.prefixes.is_empty());
    assert!(!meter_called);

    let mut invalid = task(BACKFILL_TASK_META_VERSION_1, TaskState::Succeed, false);
    invalid.meta.cloud_storage_uri = "missing-scheme".to_owned();
    assert_eq!(
        BackfillCleanUpS3.cleanup(&mut invalid, &mut storage, true, &[], |_| Ok(())),
        Err(CleanupError::InvalidCloudStorageUri)
    );
    assert!(storage.prefixes.is_empty());
}

#[test]
fn cleanup_propagates_meter_error_before_redacting_meta() {
    let mut cleanup_task = task(BACKFILL_TASK_META_VERSION_1, TaskState::Succeed, false);
    let mut storage = RecordingStorage::default();

    assert_eq!(
        BackfillCleanUpS3.cleanup(&mut cleanup_task, &mut storage, true, &[], |_| {
            Err("meter unavailable".to_owned())
        }),
        Err(CleanupError::Meter("meter unavailable".to_owned()))
    );
    assert_eq!(storage.prefixes, ["42"]);
    assert_eq!(
        cleanup_task.meta.cloud_storage_uri,
        "s3://bucket/path?secret-access-key=secret"
    );
}
