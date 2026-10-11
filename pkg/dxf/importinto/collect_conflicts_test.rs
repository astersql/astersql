// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// collect-conflicts 步骤的单元测试。
// 覆盖冲突结果回写以及文件名前缀的路径布局断言。

use astersql_dxf_importinto_conflictedkv::NewCollectResult;
use astersql_lightning_verification::MakeKVChecksum;

use crate::collect_conflicts::applyCollectResult;
use crate::{CollectConflictsStepMeta, getConflictRowFilenamePrefix};

/// Go `onFinished` persists all five aggregate outputs; test the whole wire
/// state transition instead of leaving the corresponding Go assertions only
/// in executable Rust coverage.
#[test]
fn collect_result_is_fully_persisted_to_subtask_meta() {
    let mut result = NewCollectResult(b"keyspace");
    result.RowCount = 9;
    result.TotalFileSize = 123;
    result.RowRecordingCapped = true;
    result.Checksum = MakeKVChecksum(456, 9, 789);
    result.Filenames = vec!["conflicted-rows/1/a".into(), "conflicted-rows/1/b".into()];

    let mut meta = CollectConflictsStepMeta::default();
    applyCollectResult(&mut meta, &result, true);

    let checksum = meta.Checksum.expect("Go always writes a checksum");
    assert_eq!(checksum.Sum, 789);
    assert_eq!(checksum.KVs, 9);
    assert_eq!(checksum.Size, 456);
    assert_eq!(meta.ConflictedRowCount, 9);
    assert_eq!(meta.ConflictedRowFilenames, result.Filenames);
    assert!(meta.ConflictedRowRecordingCapped);
    assert!(meta.TooManyConflictsFromIndex);
}

/// 冲突行文件前缀必须落在 `conflicted-rows/` 下，避免随任务目录一并被清理。
#[test]
fn conflict_row_prefix_survives_task_directory_cleanup() {
    assert_eq!(
        getConflictRowFilenamePrefix(12, 34, "abcd"),
        "conflicted-rows/12/34-abcd"
    );
}

fn unique_pair(
    handle: Box<dyn astersql_tablecodec::kv::Handle>,
    value: i64,
    prefix: &[u8],
) -> astersql_dxf_importinto_conflictedkv::ConflictKVPair {
    let encoded = astersql_tablecodec::codec::EncodeInt(vec![3], value);
    let key = astersql_tablecodec::EncodeIndexSeekKey(10, 2, Some(encoded));
    let mut bytes = prefix.to_vec();
    bytes.extend_from_slice(&key.0);
    astersql_dxf_importinto_conflictedkv::ConflictKVPair {
        Key: astersql_kv::Key(bytes),
        Value: astersql_tablecodec::EncodeHandleInUniqueIndexValue(handle, false),
    }
}

#[test]
fn mv_index_routes_encoded_handles_and_closes_channels() {
    use crate::collect_conflicts::*;
    use astersql_meta_model::{IndexColumn, IndexInfo};
    let index = IndexInfo {
        ID: 2,
        MVIndex: true,
        Columns: vec![IndexColumn::default()],
        ..Default::default()
    };
    for prefix in [Vec::new(), vec![b'x', 0, 0, 1]] {
        let pair = unique_pair(Box::new(astersql_tablecodec::kv::IntHandle(1)), 10, &prefix);
        let expected = conflictWorkerForPair(&pair, Some(&index), 4, 0, &prefix).unwrap();
        // Python/zlib independently gives IEEE CRC32=0x411e6a25 for Go encoded IntHandle(1).
        assert_eq!(1, expected);
        let (tx, rx) = std::sync::mpsc::channel();
        for value in [10, 20] {
            tx.send(unique_pair(
                Box::new(astersql_tablecodec::kv::IntHandle(1)),
                value,
                &prefix,
            ))
            .unwrap();
        }
        drop(tx);
        let (outputs, receivers): (Vec<_>, Vec<_>) =
            (0..4).map(|_| std::sync::mpsc::sync_channel(256)).unzip();
        dispatchMVIndexKVPairs(&Default::default(), &rx, outputs, &index, &prefix).unwrap();
        for (worker, rx) in receivers.into_iter().enumerate() {
            assert_eq!(
                if worker == expected { 2 } else { 0 },
                rx.into_iter().count()
            );
        }
        assert_eq!(
            1,
            conflictWorkerForPair(&pair, None, 4, 5, &prefix).unwrap()
        );
        assert_eq!(
            0,
            conflictWorkerForPair(&pair, Some(&index), 1, 0, &prefix).unwrap()
        );
    }
}

#[test]
fn index_metadata_errors_and_dispatch_cancellation_close_outputs() {
    use crate::collect_conflicts::*;
    use astersql_meta_model::{IndexColumn, IndexInfo, TableInfo};
    let index = IndexInfo {
        ID: 2,
        MVIndex: true,
        Columns: vec![IndexColumn::default()],
        ..Default::default()
    };
    let table = TableInfo {
        Indices: vec![index.clone()],
        ..Default::default()
    };
    assert!(getKVGroupIndexInfo(&table, "data").unwrap().is_none());
    assert!(getKVGroupIndexInfo(&table, "bad").is_err());
    assert!(getKVGroupIndexInfo(&table, "3").is_err());
    assert!(getKVGroupIndexInfo(&table, "2").unwrap().unwrap().MVIndex);
    for canceled in [false, true] {
        let context = astersql_dxf_importinto_conflictedkv::ConflictContext::default();
        if canceled {
            context.Cancel();
        }
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Default::default()).unwrap();
        drop(tx);
        let (output, result) = std::sync::mpsc::sync_channel(1);
        assert!(
            dispatchMVIndexKVPairs(&context, &rx, vec![output], &index, &[b'x', 0, 0, 1]).is_err()
        );
        assert!(result.recv().is_err());
    }
    let context = astersql_dxf_importinto_conflictedkv::ConflictContext::default();
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(unique_pair(
        Box::new(astersql_tablecodec::kv::IntHandle(1)),
        10,
        &[],
    ))
    .unwrap();
    let (output, result) = std::sync::mpsc::sync_channel(0);
    std::thread::scope(|scope| {
        let worker = scope.spawn({
            let context = context.clone();
            move || dispatchMVIndexKVPairs(&context, &rx, vec![output], &index, &[])
        });
        std::thread::sleep(std::time::Duration::from_millis(20));
        context.Cancel();
        assert!(worker.join().unwrap().unwrap_err().contains("cancelled"));
    });
    assert!(result.recv().is_err());
}
