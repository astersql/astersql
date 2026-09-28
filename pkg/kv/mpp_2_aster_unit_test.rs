// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// Aster 侧针对 MPP / 选项 / 作用域 / 内部事务等行为与 Go 对齐的单元测试。
//
// 覆盖：MPP 版本与缓存键、`MPPTask::ToPB`、TxnSource 位图边界、
// `@@txn_scope`、Version/Variables、`WalkMemBuffer`/`IncInt64`、
// `RunInNewTxn` 重试与 keyspace 判断、`BackOff` 上限。

use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use super::*;
use ::keyspace;

/// 测试用 `MPPTaskMeta`：固定返回给定地址字符串。
struct TaskAddress(&'static str);

impl MPPTaskMeta for TaskAddress {
    fn GetAddress(&self) -> String {
        self.0.to_owned()
    }

    fn CloneBox(&self) -> Box<dyn MPPTaskMeta> {
        Box::new(Self(self.0))
    }
}

/// 校验 MPP 版本解析、BuildTasks 缓存键、`ToPB` 与根任务地址行为。
#[test]
fn mpp_2_versions_and_cache_keys_match_go() {
    assert_eq!(GetNewestMppVersion().ToInt64(), 3);
    for (name, expected) in [
        ("unspecified", MppVersionUnspecified),
        ("-1", MppVersionUnspecified),
        ("0", MppVersionV0),
        ("1", MppVersionV1),
        ("2", MppVersionV2),
        ("3", MppVersionV3),
    ] {
        assert_eq!(ToMppVersion(name), (expected, true));
    }
    assert_eq!(ToMppVersion("4"), (MppVersionUnspecified, false));
    assert_eq!(ToMppVersion("garbage"), (MppVersionUnspecified, false));

    // 非分区表：按 range_id + StartKey/EndKey 拼接缓存键。
    let non_partitioned = MPPBuildTasksRequest {
        KeyRanges: Some(vec![
            KeyRange {
                StartKey: Key(b"a".to_vec()),
                EndKey: Key(b"b".to_vec()),
            },
            KeyRange {
                StartKey: Key(b"c".to_vec()),
                EndKey: Key(b"d".to_vec()),
            },
        ]),
        StartTS: 9,
        PartitionIDAndRanges: vec![],
    };
    assert_eq!(non_partitioned.ToString(), "range_id06162range_id16364");

    // 分区表：partition_id + 各分区 range。
    let partitioned = MPPBuildTasksRequest {
        KeyRanges: None,
        StartTS: 9,
        PartitionIDAndRanges: vec![PartitionIDAndRanges {
            ID: 42,
            KeyRanges: vec![KeyRange {
                StartKey: Key(b"x".to_vec()),
                EndKey: Key(b"z".to_vec()),
            }],
        }],
    };
    assert_eq!(partitioned.ToString(), "partition_id42range_id0787a");

    let task = MPPTask {
        Meta: Some(Box::new(TaskAddress("tiflash-1:3930"))),
        ID: 7,
        StartTs: 11,
        GatherID: 13,
        MppQueryID: MPPQueryID {
            QueryTs: 17,
            LocalQueryID: 19,
            ServerID: 23,
        },
        TableID: 29,
        MppVersion: MppVersionV3,
        SessionID: 31,
        SessionAlias: "session".to_owned(),
        PartitionTableIDs: vec![37],
        TiFlashStaticPrune: true,
    };
    let pb = task.ToPB();
    assert_eq!(pb.start_ts, 11);
    assert_eq!(pb.task_id, 7);
    assert_eq!(pb.address, "tiflash-1:3930");
    assert_eq!(pb.gather_id, 13);
    assert_eq!(pb.query_ts, 17);
    assert_eq!(pb.local_query_id, 19);
    assert_eq!(pb.server_id, 23);
    assert_eq!(pb.mpp_version, 3);
    assert_eq!(pb.connection_id, 31);
    assert_eq!(pb.connection_alias, "session");

    let cloned_task = task.clone();
    assert_eq!(
        cloned_task.Meta.as_ref().unwrap().GetAddress(),
        "tiflash-1:3930"
    );

    // 根任务 ID=-1：protobuf 地址应为空。
    let root_task = MPPTask {
        Meta: None,
        ID: -1,
        ..task
    };
    assert!(root_task.ToPB().address.is_empty());
}

/// 校验副本读判定与 CDC / lossy DDL TxnSource 位图边界。
#[test]
fn mpp_2_option_bitmaps_match_go_boundaries() {
    assert_eq!(PrewriteEncounterLockPolicy, 44);
    assert!(!ReplicaReadType::ReplicaReadLeader.IsFollowerRead());
    for replica_read in [
        ReplicaReadType::ReplicaReadFollower,
        ReplicaReadType::ReplicaReadMixed,
        ReplicaReadType::ReplicaReadClosest,
        ReplicaReadType::ReplicaReadClosestAdaptive,
        ReplicaReadType::ReplicaReadLearner,
        ReplicaReadType::ReplicaReadPreferLeader,
    ] {
        assert!(replica_read.IsFollowerRead());
    }
    assert!(ReplicaReadType::ReplicaReadClosest.IsClosestRead());
    assert!(!ReplicaReadType::ReplicaReadClosestAdaptive.IsClosestRead());

    let mut source = 0;
    SetCDCWriteSource(&mut source, 0).unwrap();
    assert_eq!(GetCDCWriteSource(source), 0);
    assert!(!IsCDCWriteSourceSet(source));
    SetCDCWriteSource(&mut source, 1).unwrap();
    assert_eq!(GetCDCWriteSource(source), 1);
    assert!(IsCDCWriteSourceSet(source));
    SetCDCWriteSource(&mut source, 8).unwrap();
    assert_eq!(GetCDCWriteSource(source), 9);
    let source_before_error = source;
    assert!(SetCDCWriteSource(&mut source, 9).is_err());
    assert!(SetCDCWriteSource(&mut source, 16).is_err());
    assert_eq!(source, source_before_error);

    SetLossyDDLReorgSource(&mut source, LossyDDLColumnReorgSource).unwrap();
    assert_eq!(GetLossyDDLReorgSource(source), 1);
    assert!(IsLossyDDLReorgSourceSet(source));
    let source_before_error = source;
    assert!(SetLossyDDLReorgSource(&mut source, 256).is_err());
    assert_eq!(source, source_before_error);
}

/// 校验默认/全局/本地 TxnScopeVar 与 StandAloneTiDB 标志读写。
#[test]
fn mpp_2_scope_variables_and_process_flags_match_go() {
    let default_scope = NewDefaultTxnScopeVar();
    assert_eq!(default_scope.GetVarValue(), GlobalTxnScope);
    assert_eq!(default_scope.GetTxnScope(), GlobalTxnScope);

    let global = NewGlobalTxnScopeVar();
    assert_eq!(global.GetVarValue(), GlobalTxnScope);
    assert_eq!(global.GetTxnScope(), GlobalTxnScope);

    let local = NewLocalTxnScopeVar("zone-a".to_owned());
    assert_eq!(local.GetVarValue(), LocalTxnScope);
    assert_eq!(local.GetTxnScope(), "zone-a");

    StandAloneTiDB.store(true, Ordering::Relaxed);
    assert!(StandAloneTiDB.load(Ordering::Relaxed));
    StandAloneTiDB.store(false, Ordering::Relaxed);
}

/// 校验 Version 比较与 Variables 的 killed 标志联动。
#[test]
fn mpp_2_versions_and_client_variables_match_go() {
    assert_eq!(NewVersion(42).Cmp(NewVersion(43)), -1);
    assert_eq!(NewVersion(42).Cmp(NewVersion(41)), 1);
    assert_eq!(NewVersion(42).Cmp(NewVersion(42)), 0);
    assert_eq!(MinVersion.Cmp(MaxVersion), -1);

    let killed = AtomicU32::new(0);
    let variables = NewVariables(&killed);
    assert_eq!(variables.BackoffLockFast, DefBackoffLockFast);
    assert_eq!(variables.BackOffWeight, DefBackOffWeight);
    assert!(!variables.IsKilled());
    killed.store(3, Ordering::Relaxed);
    assert!(variables.IsKilled());
}

/// 内存 Map：用 HashMap 模拟 KV，供整数辅助与 Walk 测试。
#[derive(Default)]
struct MockMap {
    entries: HashMap<Key, Vec<u8>>,
}

/// 基于条目向量的简单迭代器。
struct MockIter {
    entries: Vec<(Key, Vec<u8>)>,
    index: usize,
    closed: bool,
}

impl Iterator for MockIter {
    fn Valid(&self) -> bool {
        self.index < self.entries.len()
    }

    fn Key(&self) -> Key {
        self.entries[self.index].0.clone()
    }

    fn Value(&self) -> Vec<u8> {
        self.entries[self.index].1.clone()
    }

    fn Next(&mut self) -> Result<(), Error> {
        self.index += 1;
        Ok(())
    }

    fn Close(&mut self) {
        self.closed = true;
    }
}

impl Getter for MockMap {
    fn Get(&self, _ctx: &Context, key: Key, _options: &[GetOption]) -> Result<ValueEntry, Error> {
        self.entries
            .get(&key)
            .cloned()
            .map(|value| NewValueEntry(value, 0))
            .ok_or_else(|| ErrNotExist.FastGenByArgs(&[]))
    }
}

impl Retriever for MockMap {
    fn Iter(&self, _start: Key, _end: Option<Key>) -> Result<Box<dyn Iterator>, Error> {
        Ok(Box::new(MockIter {
            entries: self
                .entries
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            index: 0,
            closed: false,
        }))
    }

    fn IterReverse(
        &self,
        _start: Option<Key>,
        _end: Option<Key>,
    ) -> Result<Box<dyn Iterator>, Error> {
        let mut entries: Vec<_> = self
            .entries
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        entries.reverse();
        Ok(Box::new(MockIter {
            entries,
            index: 0,
            closed: false,
        }))
    }
}

impl Mutator for MockMap {
    fn Set(&mut self, key: Key, value: Vec<u8>) -> Result<(), Error> {
        self.entries.insert(key, value);
        Ok(())
    }

    fn Delete(&mut self, key: Key) -> Result<(), Error> {
        self.entries.remove(&key);
        Ok(())
    }
}
impl RetrieverMutator for MockMap {}

/// 校验 GetInt64/IncInt64 与 WalkMemBuffer 遍历。
#[test]
fn mpp_2_integer_helpers_and_walk_match_go() {
    let mut map = MockMap::default();
    let key = Key(b"key".to_vec());
    assert_eq!(GetInt64(&Context::todo(), &map, &key).unwrap(), 0);
    assert_eq!(IncInt64(&mut map, &key, 1).unwrap(), 1);
    assert_eq!(IncInt64(&mut map, &key, 10).unwrap(), 11);
    assert_eq!(GetInt64(&Context::todo(), &map, &key).unwrap(), 11);

    // Go 的回归用例明确要求跨过 uint32::MAX 后仍按 int64 递增。
    let max_u32 = i64::from(u32::MAX);
    map.Set(key.clone(), max_u32.to_string().into_bytes())
        .unwrap();
    assert_eq!(IncInt64(&mut map, &key, 1).unwrap(), max_u32 + 1);

    map.Set(Key(b"bad".to_vec()), b"not int".to_vec()).unwrap();
    assert!(IncInt64(&mut map, &Key(b"bad".to_vec()), 1).is_err());

    let mut visited = Vec::new();
    WalkMemBuffer(&map, |key, value| {
        visited.push((key.clone(), value.to_vec()));
        Ok(())
    })
    .unwrap();
    assert_eq!(visited.len(), 2);
}

/// 最小 Transaction mock：成功 Commit，并用 start_ts 区分 Begin 次数。
struct MockTxn {
    start_ts: u64,
}

impl Getter for MockTxn {
    fn Get(&self, _ctx: &Context, _key: Key, _options: &[GetOption]) -> Result<ValueEntry, Error> {
        Err(ErrNotExist.FastGenByArgs(&[]))
    }
}

impl Retriever for MockTxn {
    fn Iter(&self, _start: Key, _end: Option<Key>) -> Result<Box<dyn Iterator>, Error> {
        Ok(Box::new(EmptyIterator))
    }

    fn IterReverse(
        &self,
        _start: Option<Key>,
        _end: Option<Key>,
    ) -> Result<Box<dyn Iterator>, Error> {
        Ok(Box::new(EmptyIterator))
    }
}

impl Mutator for MockTxn {
    fn Set(&mut self, _key: Key, _value: Vec<u8>) -> Result<(), Error> {
        Ok(())
    }
    fn Delete(&mut self, _key: Key) -> Result<(), Error> {
        Ok(())
    }
}
impl RetrieverMutator for MockTxn {}

impl FairLockingController for MockTxn {
    fn StartFairLocking(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn RetryFairLocking(&mut self, _ctx: &Context) -> Result<(), Error> {
        Ok(())
    }
    fn CancelFairLocking(&mut self, _ctx: &Context) -> Result<(), Error> {
        Ok(())
    }
    fn DoneFairLocking(&mut self, _ctx: &Context) -> Result<(), Error> {
        Ok(())
    }
    fn IsInFairLockingMode(&self) -> bool {
        false
    }
}

impl Transaction for MockTxn {
    fn Size(&self) -> usize {
        0
    }
    fn Mem(&self) -> u64 {
        0
    }
    fn SetMemoryFootprintChangeHook(&mut self, _hook: Box<dyn Fn(u64)>) {}
    fn MemHookSet(&self) -> bool {
        false
    }
    fn Len(&self) -> usize {
        0
    }
    fn StartTS(&self) -> u64 {
        self.start_ts
    }

    fn Commit(&mut self, _ctx: &Context) -> Result<(), Error> {
        Ok(())
    }

    fn Rollback(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn String(&self) -> String {
        format!("mock-txn-{}", self.start_ts)
    }
    fn LockKeys(
        &mut self,
        _ctx: &Context,
        _lock_ctx: &mut LockCtx,
        _keys: &[Key],
    ) -> Result<(), Error> {
        Ok(())
    }
    fn LockKeysFunc(
        &mut self,
        _ctx: &Context,
        _lock_ctx: &mut LockCtx,
        callback: &mut dyn FnMut(),
        _keys: &[Key],
    ) -> Result<(), Error> {
        callback();
        Ok(())
    }
    fn SetOption(&mut self, _option: i32, _value: Option<Box<dyn Any>>) {}
    fn GetOption(&self, _option: i32) -> Option<&dyn Any> {
        None
    }
    fn IsReadOnly(&self) -> bool {
        false
    }
    fn CommitTS(&self) -> u64 {
        0
    }
    fn Valid(&self) -> bool {
        true
    }
    fn GetMemBuffer(&self) -> &dyn MemBuffer {
        panic!("unused in focused test")
    }
    fn GetSnapshot(&self) -> &dyn Snapshot {
        panic!("unused in focused test")
    }
    fn SetVars(&mut self, _vars: Box<dyn Any>) {}
    fn GetVars(&self) -> &dyn Any {
        &()
    }
    fn BatchGet(
        &self,
        _ctx: &Context,
        _keys: &[Key],
        _options: &[BatchGetOption],
    ) -> Result<HashMap<String, ValueEntry>, Error> {
        Ok(HashMap::new())
    }
    fn IsPessimistic(&self) -> bool {
        false
    }
    fn CacheTableInfo(&mut self, _id: i64, _info: model::TableInfo) {}
    fn GetTableInfo(&self, _id: i64) -> Option<&model::TableInfo> {
        None
    }
    fn SetDiskFullOpt(&mut self, _level: kvrpcpb::DiskFullOpt) {}
    fn ClearDiskFullOpt(&mut self) {}
    fn GetMemDBCheckpoint(&self) -> &tikv::MemDBCheckpoint {
        &tikv::MemDBCheckpoint
    }
    fn RollbackMemDBToCheckpoint(&mut self, _checkpoint: &tikv::MemDBCheckpoint) {}
    fn IsPipelined(&self) -> bool {
        false
    }
    fn MayFlush(&mut self) -> Result<(), Error> {
        Ok(())
    }
}

/// Storage mock：每次 Begin 递增 start_ts，并暴露 keyspace 供用户/系统 KS 判断。
struct MockStore {
    begins: AtomicUsize,
    keyspace: String,
}

impl Storage for MockStore {
    fn Begin(&self, _options: &[tikv::TxnOption]) -> Result<Box<dyn Transaction>, Error> {
        let attempt = self.begins.fetch_add(1, Ordering::Relaxed) + 1;
        Ok(Box::new(MockTxn {
            start_ts: attempt as u64,
        }))
    }

    fn GetSnapshot(&self, _version: Version) -> Box<dyn Snapshot> {
        panic!("unused in focused test")
    }
    fn GetClient(&self) -> &dyn Client {
        panic!("unused in focused test")
    }
    fn GetMPPClient(&self) -> &dyn MPPClient {
        panic!("unused in focused test")
    }
    fn Close(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn UUID(&self) -> String {
        "mock-store".to_owned()
    }
    fn CurrentVersion(&self, _txn_scope: &str) -> Result<Version, Error> {
        Ok(NewVersion(0))
    }
    fn GetOracle(&self) -> &dyn oracle::Oracle {
        panic!("unused in focused test")
    }
    fn SupportDeleteRange(&self) -> bool {
        false
    }
    fn Name(&self) -> String {
        "mock".to_owned()
    }
    fn Describe(&self) -> String {
        "mock store".to_owned()
    }
    fn ShowStatus(&self, _ctx: &Context, _key: &str) -> Result<Box<dyn Any>, Error> {
        Ok(Box::new(()))
    }
    fn GetMemCache(&self) -> &dyn MemManager {
        panic!("unused in focused test")
    }
    fn GetMinSafeTS(&self, _txn_scope: &str) -> u64 {
        0
    }
    fn GetLockWaits(&self) -> Result<Vec<deadlockpb::WaitForEntry>, Error> {
        Ok(Vec::new())
    }
    fn GetCodec(&self) -> tikv::Codec {
        tikv::Codec
    }
    fn SetOption(&self, _key: Box<dyn Any>, _value: Box<dyn Any>) {}
    fn GetOption(&self, _key: &dyn Any) -> Option<&dyn Any> {
        None
    }
    fn GetClusterID(&self) -> u64 {
        0
    }
    fn GetKeyspace(&self) -> String {
        self.keyspace.clone()
    }
}

/// 校验 RunInNewTxn 可重试错误会多次 Begin，以及用户/系统 keyspace 判定。
#[test]
fn mpp_2_new_transaction_retries_and_keyspace_checks_match_go() {
    let store = MockStore {
        begins: AtomicUsize::new(0),
        keyspace: "user".to_owned(),
    };
    let context = WithInternalSourceType(Context::todo(), InternalTxnOthers);
    let mut calls = 0;
    // 前两次返回可重试错误，第三次成功，期望 Begin 三次。
    RunInNewTxn(&context, &store, true, |_ctx, _txn| {
        calls += 1;
        if calls < 3 {
            Err(ErrTxnRetryable.FastGenByArgs(&[]))
        } else {
            Ok(())
        }
    })
    .unwrap();
    assert_eq!(store.begins.load(Ordering::Relaxed), 3);
    assert_eq!(GetInternalSourceType(&context), InternalTxnOthers);
    assert_eq!(IsUserKS(&store), kerneltype::IsNextGen());
    assert!(!IsSystemKS(&store));

    let system_store = MockStore {
        begins: AtomicUsize::new(0),
        keyspace: keyspace::System.to_owned(),
    };
    assert!(!IsUserKS(&system_store));
    assert_eq!(IsSystemKS(&system_store), kerneltype::IsNextGen());

    // 不可重试错误应立即失败且只 Begin 一次。
    let non_retry_store = MockStore {
        begins: AtomicUsize::new(0),
        keyspace: "user".to_owned(),
    };
    let error = RunInNewTxn(&context, &non_retry_store, true, |_ctx, _txn| {
        Err(errors::New("do not retry"))
    })
    .unwrap_err();
    assert_eq!(error.to_string(), "do not retry");
    assert_eq!(non_retry_store.begins.load(Ordering::Relaxed), 1);
}

/// 校验 BackOff 在极大 attempts 时仍受 100ms 上限约束。
#[test]
fn mpp_2_backoff_is_capped_like_go() {
    for (attempts, upper_millis) in [(1, 2), (2, 4), (3, 8), (100_000, 100)] {
        assert!(BackOff(attempts) <= std::time::Duration::from_millis(upper_millis));
    }
}
