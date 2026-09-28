// Copyright 2026 AsterSQL.
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

// dbreader 基于 fjall 后端的 MVCC 可见性与扫描行为单测。
//
// 用临时目录打开 fjall Database，按 TiKV 风格编码 versioned key，
// 验证 Get/BatchGet/Scan/ReverseScan、RcCheckTS 写冲突、ScanBreak
// 与 GetMvccInfo 等与 Go 侧语义对齐。

use std::collections::HashSet;

use super::fjall::{Database, Keyspace, KeyspaceCreateOptions, Readable, Snapshot};
use tempfile::TempDir;

use super::db_reader::{
    DBItem, DBIterator, DBReader, DbReaderError, IteratorOptions, NewDBReader, ReadTxn,
    ScanProcessor,
};
use super::{kvrpcpb, mvcc};

#[derive(Clone, Debug)]
/// fjall 解码后的一条 MVCC 记录项。
struct FjallItem {
    /// 用户键（已去除版本后缀）。
    key: Vec<u8>,
    /// 用户值载荷。
    value: Vec<u8>,
    /// 用户元数据（含 start_ts/commit_ts）。
    meta: mvcc::DBUserMeta,
    /// 版本号，通常等于 commit_ts。
    version: u64,
}

/// 将 FjallItem 适配为 DBReader 所需的 DBItem。
impl DBItem for FjallItem {
    fn Key(&self) -> &[u8] {
        &self.key
    }

    fn Value(&self) -> Result<Vec<u8>, DbReaderError> {
        Ok(self.value.clone())
    }

    fn UserMeta(&self) -> &[u8] {
        &self.meta.0
    }

    fn Version(&self) -> u64 {
        self.version
    }

    fn IsEmpty(&self) -> bool {
        self.value.is_empty()
    }
}

/// 编码 versioned key：转义 0x00，追加 0x00 0x00 与降序 commit_ts。
/// 降序编码使同一用户键下较新版本在字典序上更靠前。
fn encode_user_key(key: &[u8], commit_ts: u64) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(key.len() + 10);
    // 0x00 转义为 0x00 0xff，避免与结束标记冲突。
    for byte in key {
        if *byte == 0 {
            encoded.extend_from_slice(&[0, 0xff]);
        } else {
            encoded.push(*byte);
        }
    }
    // 键结束标记后接 (u64::MAX - commit_ts) 大端，实现版本降序。
    encoded.extend_from_slice(&[0, 0]);
    encoded.extend_from_slice(&(u64::MAX - commit_ts).to_be_bytes());
    encoded
}

/// 解码 versioned key，还原用户键与 commit_ts。
fn decode_user_key(encoded: &[u8]) -> Result<(Vec<u8>, u64), DbReaderError> {
    let mut key = Vec::new();
    let mut offset = 0;
    while offset + 1 < encoded.len() {
        if encoded[offset] != 0 {
            key.push(encoded[offset]);
            offset += 1;
        } else if encoded[offset + 1] == 0xff {
            key.push(0);
            offset += 2;
        } else if encoded[offset + 1] == 0 {
            offset += 2;
            break;
        } else {
            return Err(DbReaderError::backend("invalid escaped key"));
        }
    }
    if encoded.len() != offset + 8 {
        return Err(DbReaderError::backend("invalid versioned key"));
    }
    let descending = u64::from_be_bytes(encoded[offset..].try_into().unwrap());
    Ok((key, u64::MAX - descending))
}

/// 值布局：start_ts(8 LE) + commit_ts(8 LE) + 用户值。
fn encode_value(start_ts: u64, commit_ts: u64, value: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(16 + value.len());
    encoded.extend_from_slice(&start_ts.to_le_bytes());
    encoded.extend_from_slice(&commit_ts.to_le_bytes());
    encoded.extend_from_slice(value);
    encoded
}

/// 从存储原始 kv 解码为 FjallItem。
fn decode_item(key: &[u8], value: &[u8]) -> Result<FjallItem, DbReaderError> {
    if value.len() < 16 {
        return Err(DbReaderError::backend("invalid MVCC value"));
    }
    let (key, version) = decode_user_key(key)?;
    let start_ts = u64::from_le_bytes(value[..8].try_into().unwrap());
    let commit_ts = u64::from_le_bytes(value[8..16].try_into().unwrap());
    Ok(FjallItem {
        key,
        value: value[16..].to_vec(),
        meta: mvcc::NewDBUserMeta(start_ts, commit_ts),
        version,
    })
}

/// 基于快照物化可见项的测试用迭代器。
struct FjallIterator {
    /// fjall 快照。
    snapshot: Snapshot,
    /// 默认 keyspace。
    keyspace: Keyspace,
    /// 起止键与是否反向等选项。
    options: IteratorOptions,
    /// 读时间戳：只可见 version <= read_ts 的版本。
    read_ts: u64,
    /// 为 true 时保留同一键的所有可见版本。
    all_versions: bool,
    /// Seek 后物化的可见项列表。
    items: Vec<FjallItem>,
    /// 当前游标下标。
    cursor: usize,
}

/// 物化可见项的辅助实现。
impl FjallIterator {
    /// 扫描快照，按范围/read_ts/all_versions/Reverse 过滤。
    fn visible_items(&self) -> Result<Vec<FjallItem>, DbReaderError> {
        let mut decoded = Vec::new();
        for guard in self.snapshot.iter(&self.keyspace) {
            let (key, value) = guard.into_inner().map_err(DbReaderError::backend)?;
            let item = decode_item(&key, &value)?;
            if (!self.options.StartKey.is_empty() && item.key < self.options.StartKey)
                || (!self.options.EndKey.is_empty() && item.key >= self.options.EndKey)
            {
                continue;
            }
            decoded.push(item);
        }

        // 默认每键只保留最新可见版本（首次出现）；all_versions 则全留。
        let mut seen = HashSet::new();
        decoded.retain(|item| {
            if item.version > self.read_ts {
                return false;
            }
            self.all_versions || seen.insert(item.key.clone())
        });
        if self.options.Reverse {
            decoded.reverse();
        }
        Ok(decoded)
    }
}

/// DBIterator：Seek 物化列表后用游标遍历。
impl DBIterator for FjallIterator {
    fn SetAllVersions(&mut self, all_versions: bool) {
        self.all_versions = all_versions;
    }

    fn SetReadTS(&mut self, read_ts: u64) {
        self.read_ts = read_ts;
    }

    /// 物化可见项并将游标定位到 >=（或反向 <=）目标键。
    fn Seek(&mut self, key: &[u8]) -> Result<(), DbReaderError> {
        self.items = self.visible_items()?;
        self.cursor = self
            .items
            .iter()
            .position(|item| {
                if self.options.Reverse {
                    item.key.as_slice() <= key
                } else {
                    item.key.as_slice() >= key
                }
            })
            .unwrap_or(self.items.len());
        Ok(())
    }

    fn Valid(&self) -> bool {
        self.cursor < self.items.len()
    }

    fn Next(&mut self) {
        self.cursor += usize::from(self.Valid());
    }

    fn Item(&self) -> Option<Box<dyn DBItem>> {
        self.items
            .get(self.cursor)
            .cloned()
            .map(|item| Box::new(item) as Box<dyn DBItem>)
    }

    fn Close(&mut self) {}
}

/// 测试用只读事务：持有快照与 read_ts。
struct FjallTxn {
    snapshot: Snapshot,
    /// 默认命名空间。
    keyspace: Keyspace,
    read_ts: u64,
}

/// 枚举快照中全部解码项。
impl FjallTxn {
    /// 线性扫描 keyspace 并解码。
    fn all_items(&self) -> Result<Vec<FjallItem>, DbReaderError> {
        self.snapshot
            .iter(&self.keyspace)
            .map(|guard| {
                let (key, value) = guard.into_inner().map_err(DbReaderError::backend)?;
                decode_item(&key, &value)
            })
            .collect()
    }
}

/// ReadTxn：Get/MultiGet/NewIterator 基于快照。
impl ReadTxn for FjallTxn {
    fn SetReadTS(&mut self, read_ts: u64) {
        self.read_ts = read_ts;
    }

    /// 返回 version <= read_ts 的匹配项（扫描顺序下首个）。
    fn Get(&mut self, key: &[u8]) -> Result<Option<Box<dyn DBItem>>, DbReaderError> {
        Ok(self
            .all_items()?
            .into_iter()
            .find(|item| item.key == key && item.version <= self.read_ts)
            .map(|item| Box::new(item) as Box<dyn DBItem>))
    }

    fn MultiGet(
        &mut self,
        keys: &[Vec<u8>],
    ) -> Result<Vec<Option<Box<dyn DBItem>>>, DbReaderError> {
        keys.iter().map(|key| self.Get(key)).collect()
    }

    fn NewIterator(
        &mut self,
        options: IteratorOptions,
    ) -> Result<Box<dyn DBIterator>, DbReaderError> {
        Ok(Box::new(FjallIterator {
            snapshot: self.snapshot.clone(),
            keyspace: self.keyspace.clone(),
            options,
            read_ts: self.read_ts,
            all_versions: false,
            items: Vec::new(),
            cursor: 0,
        }))
    }

    fn Discard(&mut self) {}
}

/// Value 固定失败的条目，用于覆盖 Go BatchGet 的跨缺失项错误沿用语义。
struct ValueErrorItem;

impl DBItem for ValueErrorItem {
    fn Key(&self) -> &[u8] {
        b"bad"
    }

    fn Value(&self) -> Result<Vec<u8>, DbReaderError> {
        Err(DbReaderError::backend("value failed"))
    }

    fn UserMeta(&self) -> &[u8] {
        &[]
    }

    fn Version(&self) -> u64 {
        0
    }

    fn IsEmpty(&self) -> bool {
        false
    }
}

/// MultiGet 依次返回“Value 失败的 item、缺失 item”。
struct BatchValueErrorTxn;

impl ReadTxn for BatchValueErrorTxn {
    fn SetReadTS(&mut self, _read_ts: u64) {}

    fn Get(&mut self, _key: &[u8]) -> Result<Option<Box<dyn DBItem>>, DbReaderError> {
        unreachable!()
    }

    fn MultiGet(
        &mut self,
        _keys: &[Vec<u8>],
    ) -> Result<Vec<Option<Box<dyn DBItem>>>, DbReaderError> {
        Ok(vec![Some(Box::new(ValueErrorItem)), None])
    }

    fn NewIterator(
        &mut self,
        _options: IteratorOptions,
    ) -> Result<Box<dyn DBIterator>, DbReaderError> {
        unreachable!()
    }

    fn Discard(&mut self) {}
}

/// 临时目录上的 fjall 测试库封装。
struct TestStore {
    /// 保持临时目录存活。
    _dir: TempDir,
    /// fjall 数据库句柄。
    db: Database,
    keyspace: Keyspace,
}

/// 创建库、写入 MVCC 记录、构造 DBReader。
impl TestStore {
    /// 打开临时 fjall 库与 default keyspace。
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::builder(dir.path()).open().unwrap();
        let keyspace = db
            .keyspace("default", KeyspaceCreateOptions::default)
            .unwrap();
        Self {
            _dir: dir,
            db,
            keyspace,
        }
    }

    /// 写入一条带 start/commit ts 的 MVCC 记录。
    fn put(&self, key: &[u8], value: &[u8], start_ts: u64, commit_ts: u64) {
        self.keyspace
            .insert(
                encode_user_key(key, commit_ts),
                encode_value(start_ts, commit_ts, value),
            )
            .unwrap();
    }

    /// 在 [start, end) Region 范围上构造 DBReader。
    fn reader(&self, start: &[u8], end: &[u8]) -> DBReader {
        NewDBReader(
            start.to_vec(),
            end.to_vec(),
            Box::new(FjallTxn {
                snapshot: self.db.snapshot(),
                keyspace: self.keyspace.clone(),
                read_ts: u64::MAX,
            }),
        )
    }
}

#[derive(Default)]
/// 收集 Scan 回调结果的处理器；可配置跳过值或提前 ScanBreak。
struct CollectProcessor {
    /// 为 true 时 Scan 可不取 value。
    skip_value: bool,
    /// 收集到的 (key, value, commit_ts)。
    rows: Vec<(Vec<u8>, Vec<u8>, u64)>,
    /// 收集到该行数后返回 ScanBreak。
    break_after: Option<usize>,
}

/// 将每行写入 rows，并在达到 break_after 时中断扫描。
impl ScanProcessor for CollectProcessor {
    fn Process(&mut self, key: &[u8], value: &[u8], commit_ts: u64) -> Result<(), DbReaderError> {
        self.rows.push((key.to_vec(), value.to_vec(), commit_ts));
        if self.break_after == Some(self.rows.len()) {
            return Err(DbReaderError::ScanBreak);
        }
        Ok(())
    }

    fn SkipValue(&self) -> bool {
        self.skip_value
    }
}

#[test]
/// Get 按 read_ts 可见旧版本；开启 RcCheckTS 时遇更新版本报写冲突。
fn get_and_rc_check_ts_match_go_mvcc_visibility() {
    let store = TestStore::new();
    store.put(b"key", b"old", 5, 10);
    store.put(b"key", b"new", 15, 20);
    let mut reader = store.reader(b"", b"");

    // read_ts=15 只能看到 commit_ts=10 的 old，看不到 20 的 new。
    let (value, meta) = reader.Get(b"key", 15).unwrap().unwrap();
    assert_eq!(value, b"old");
    assert_eq!((meta.StartTS(), meta.CommitTS()), (5, 10));

    // RcCheckTS：读时若存在更新的提交版本则冲突（Read Committed 检查）。
    reader.RcCheckTS = true;
    let error = reader.Get(b"key", 15).unwrap_err();
    let DbReaderError::Conflict(conflict) = error else {
        panic!("expected RcCheckTs conflict");
    };
    assert_eq!(conflict.StartTS, 15);
    assert_eq!(conflict.ConflictTS, 15);
    assert_eq!(conflict.ConflictCommitTS, 20);
    assert_eq!(conflict.Reason, kvrpcpb::WriteConflictReason::RcCheckTs);
}

#[test]
/// BatchGet 保持请求键顺序，缺失键返回 None。
fn batch_get_preserves_input_order_and_missing_values() {
    let store = TestStore::new();
    store.put(b"a", b"A", 1, 2);
    store.put(b"c", b"C", 3, 4);
    let mut reader = store.reader(b"", b"");
    let mut got = Vec::new();

    reader.BatchGet(
        &[b"c".to_vec(), b"b".to_vec(), b"a".to_vec()],
        10,
        &mut |key, value, meta, error| {
            assert!(error.is_none());
            got.push((
                key.to_vec(),
                value.map(<[u8]>::to_vec),
                if value.is_some() { meta.CommitTS() } else { 0 },
            ));
        },
    );

    assert_eq!(
        got,
        vec![
            (b"c".to_vec(), Some(b"C".to_vec()), 4),
            (b"b".to_vec(), None, 0),
            (b"a".to_vec(), Some(b"A".to_vec()), 2),
        ]
    );
}

#[test]
/// Go 循环外的 err 会让 Value 失败后的缺失 item 沿用同一错误。
fn batch_get_carries_value_error_to_following_missing_item() {
    let mut reader = NewDBReader(Vec::new(), Vec::new(), Box::new(BatchValueErrorTxn));
    let mut errors = Vec::new();

    reader.BatchGet(
        &[b"bad".to_vec(), b"missing".to_vec()],
        10,
        &mut |_key, _value, _meta, error| {
            errors.push(error.map(ToString::to_string));
        },
    );

    assert_eq!(
        errors,
        vec![
            Some("value failed".to_owned()),
            Some("value failed".to_owned())
        ]
    );
}

#[test]
/// 正向/反向 Scan 使用半开区间，且 ReverseScan 可 SkipValue。
fn forward_and_reverse_scan_match_half_open_ranges() {
    let store = TestStore::new();
    for (index, key) in [b"a", b"b", b"c", b"d"].into_iter().enumerate() {
        store.put(key, key, index as u64 + 1, index as u64 + 10);
    }
    store.put(b"bb", b"", 8, 18);
    let mut reader = store.reader(b"a", b"e");
    let mut forward = CollectProcessor::default();
    reader.Scan(b"b", b"d", 10, 30, &mut forward).unwrap();
    assert_eq!(
        forward
            .rows
            .iter()
            .map(|row| row.0.clone())
            .collect::<Vec<_>>(),
        vec![b"b".to_vec(), b"c".to_vec()]
    );

    let mut reverse = CollectProcessor {
        skip_value: true,
        ..Default::default()
    };
    reader
        .ReverseScan(b"b", b"d", 10, 30, &mut reverse)
        .unwrap();
    assert_eq!(
        reverse.rows,
        vec![
            (b"c".to_vec(), Vec::new(), 12),
            (b"b".to_vec(), Vec::new(), 11),
        ]
    );
}

#[test]
/// ScanBreak 提前结束；limit 截断返回行数。
fn scan_break_and_limit_follow_go_control_flow() {
    let store = TestStore::new();
    for key in [b"a", b"b", b"c"] {
        store.put(key, key, 1, 2);
    }
    let mut reader = store.reader(b"", b"");
    let mut breaking = CollectProcessor {
        break_after: Some(1),
        ..Default::default()
    };
    reader.Scan(b"", b"", 100, 10, &mut breaking).unwrap();
    assert_eq!(breaking.rows.len(), 1);

    let mut limited = CollectProcessor::default();
    reader.Scan(b"", b"", 2, 10, &mut limited).unwrap();
    assert_eq!(limited.rows.len(), 2);

    let mut negative = CollectProcessor::default();
    reader.Scan(b"", b"", -1, 10, &mut negative).unwrap();
    assert_eq!(negative.rows.len(), 1);
}

#[test]
/// GetMvccInfo 列出全部写版本；GetKeyByStartTs 按 start_ts 反查键。
fn all_versions_feed_mvcc_info_and_start_ts_lookup() {
    let store = TestStore::new();
    store.put(b"key", b"old", 5, 10);
    store.put(b"key", b"", 15, 20);
    let mut reader = store.reader(b"", b"");

    let mut info = kvrpcpb::MvccInfo::default();
    reader.GetMvccInfoByKey(b"key", false, &mut info).unwrap();
    assert_eq!(info.writes.len(), 2);
    assert_eq!(info.writes[0].r_type, kvrpcpb::Op::Del);
    assert_eq!(info.writes[1].short_value, b"old");
    assert_eq!(
        reader.GetKeyByStartTs(b"a", b"z", 5).unwrap(),
        Some(b"key".to_vec())
    );
}
