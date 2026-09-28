// Copyright 2026 AsterSQL.
//
// Copyright 2019-present PingCAP, Inc.
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

// Copyright 2019-present PingCAP, Inc.
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

// DBReader：基于抽象 ReadTxn/DBIterator 的 MVCC 只读访问层。
//
// 对应 Go 侧同名组件。只读请求在创建本结构前应已完成锁检查；
// 支持正反向扫描、按 start_ts 查键、RC CheckTS 隔离冲突检测，
// 以及 IndexLookUp 用的 ExtraDbReaderProvider。

#![allow(non_snake_case, non_camel_case_types, dead_code)]

use std::any::Any;
use std::error::Error as StdError;
use std::fmt;
use std::sync::Arc;

use crate::{errorpb, kv, kverrors, kvrpcpb, metapb, mvcc};

/// Error returned by the storage adapter or by DBReader's MVCC checks.
///
/// 存储后端错误、写冲突（MVCC 冲突）或扫描主动中断。
#[derive(Clone, Debug)]
pub enum DbReaderError {
    Backend(Arc<anyhow::Error>),
    Conflict(kverrors::ErrConflict),
    ScanBreak,
}

impl DbReaderError {
    /// 将任意可 Display 的错误包装为 Backend 变体。
    pub fn backend(error: impl fmt::Display) -> Self {
        Self::Backend(Arc::new(anyhow::anyhow!(error.to_string())))
    }
}

impl fmt::Display for DbReaderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Backend(error) => fmt::Display::fmt(error, formatter),
            Self::Conflict(error) => fmt::Display::fmt(error, formatter),
            Self::ScanBreak => formatter.write_str("scan break error"),
        }
    }
}

impl StdError for DbReaderError {}

impl PartialEq for DbReaderError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Backend(left), Self::Backend(right)) => left.to_string() == right.to_string(),
            (Self::Conflict(left), Self::Conflict(right)) => left == right,
            (Self::ScanBreak, Self::ScanBreak) => true,
            _ => false,
        }
    }
}

/// Backend-independent equivalent of Badger iterator options.
///
/// 与具体引擎无关的迭代器选项：方向与起止键。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IteratorOptions {
    pub Reverse: bool,
    pub StartKey: Vec<u8>,
    pub EndKey: Vec<u8>,
}

/// A committed item returned by a read transaction.
///
/// 已提交版本条目：键、值、用户元数据、版本号。
pub trait DBItem: Send {
    fn Key(&self) -> &[u8];
    fn Value(&self) -> Result<Vec<u8>, DbReaderError>;
    fn UserMeta(&self) -> &[u8];
    fn Version(&self) -> u64;
    fn IsEmpty(&self) -> bool;

    fn KeyCopy(&self) -> Vec<u8> {
        self.Key().to_vec()
    }

    fn ValueCopy(&self) -> Result<Vec<u8>, DbReaderError> {
        self.Value()
    }
}

/// Iterator operations DBReader needs from the selected storage engine.
///
/// 引擎迭代器能力：设置全版本/读时间戳、Seek、前进与取条目。
pub trait DBIterator: Send {
    fn SetAllVersions(&mut self, all_versions: bool);
    fn SetReadTS(&mut self, read_ts: u64);
    fn Seek(&mut self, key: &[u8]) -> Result<(), DbReaderError>;
    fn Valid(&self) -> bool;
    fn Next(&mut self);
    fn Item(&self) -> Option<Box<dyn DBItem>>;
    fn Close(&mut self);
}

/// Read-only transaction boundary implemented by the package's storage adapter.
///
/// 只读事务边界：点查、批量查、创建迭代器与丢弃。
pub trait ReadTxn: Send {
    fn SetReadTS(&mut self, read_ts: u64);
    fn Get(&mut self, key: &[u8]) -> Result<Option<Box<dyn DBItem>>, DbReaderError>;
    fn MultiGet(&mut self, keys: &[Vec<u8>])
    -> Result<Vec<Option<Box<dyn DBItem>>>, DbReaderError>;
    fn NewIterator(
        &mut self,
        options: IteratorOptions,
    ) -> Result<Box<dyn DBIterator>, DbReaderError>;
    fn Discard(&mut self);
}

// LocateExtraRegionResult is the result of LocateExtraRegion.
/// LocateExtraRegion 的返回：是否找到、Region/Peer 及是否 leader。
pub struct LocateExtraRegionResult {
    pub Found: bool,
    pub Region: Option<metapb::Region>,
    pub Peer: Option<metapb::Peer>,
    pub IsLeader: bool,
}

// GetExtraDBReaderContext is the context for GetExtraDBReaderByRegion.
/// 按 Region 获取额外 DBReader 时的上下文（含 KeyRange 列表）。
pub struct GetExtraDBReaderContext {
    pub Region: Option<metapb::Region>,
    pub Peer: Option<metapb::Peer>,
    pub Ranges: Vec<kv::KeyRange>,
}

// ExtraDbReaderProvider is used to provide extra DBReader.
// It is used by the IndexLookUp.
/// 为 IndexLookUp 提供跨 Region 的额外 DBReader。
pub trait ExtraDbReaderProvider {
    // LocateExtraRegion locates the region for the key in local.
    /// 在本地定位 key 所属 Region。
    fn LocateExtraRegion(
        &mut self,
        ctx: &dyn Any,
        key: &[u8],
    ) -> Result<LocateExtraRegionResult, DbReaderError>;

    // GetExtraDBReaderByRegion returns a DBReader for the region.
    /// 按 Region 上下文返回额外 DBReader，或协议层 Error。
    fn GetExtraDBReaderByRegion(
        &mut self,
        ctx: GetExtraDBReaderContext,
    ) -> (Option<DBReader>, Option<errorpb::Error>);
}

// NewDBReader returns a new DBReader.
/// 构造绑定键范围与只读事务的 DBReader。
pub fn NewDBReader(startKey: Vec<u8>, endKey: Vec<u8>, txn: Box<dyn ReadTxn>) -> DBReader {
    DBReader {
        StartKey: startKey,
        EndKey: endKey,
        txn,
        iter: None,
        extraIter: None,
        revIter: None,
        RcCheckTS: false,
        ExtraDbReaderProvider: None,
    }
}

// NewIterator returns a new storage iterator.
/// 在给定事务上创建正/反向迭代器。
pub fn NewIterator(
    txn: &mut dyn ReadTxn,
    reverse: bool,
    startKey: &[u8],
    endKey: &[u8],
) -> Result<Box<dyn DBIterator>, DbReaderError> {
    txn.NewIterator(IteratorOptions {
        Reverse: reverse,
        StartKey: startKey.to_vec(),
        EndKey: endKey.to_vec(),
    })
}

// DBReader reads data from DB. For read-only requests, locks must already be
// checked before DBReader is created.
/// 从 DB 读取数据；只读请求创建前须已检查锁。
pub struct DBReader {
    /// Region/扫描下界键。
    pub StartKey: Vec<u8>,
    /// Region/扫描上界键。
    pub EndKey: Vec<u8>,
    txn: Box<dyn ReadTxn>,
    iter: Option<Box<dyn DBIterator>>,
    extraIter: Option<Box<dyn DBIterator>>,
    revIter: Option<Box<dyn DBIterator>>,
    /// 是否启用 RC CheckTS 隔离（读时用 MAX ts 再校验 commit_ts）。
    pub RcCheckTS: bool,
    /// IndexLookUp 额外 Region 读取提供者。
    pub ExtraDbReaderProvider: Option<Box<dyn ExtraDbReaderProvider>>,
}

impl DBReader {
    /// 同步事务与已创建迭代器的读时间戳。
    fn setReadTS(&mut self, readTS: u64) {
        self.txn.SetReadTS(readTS);
        for iter in [&mut self.iter, &mut self.extraIter, &mut self.revIter] {
            if let Some(iter) = iter.as_deref_mut() {
                iter.SetReadTS(readTS);
            }
        }
    }

    // GetMvccInfoByKey fills MvccInfo reading committed keys from db.
    /// 枚举某键全部已提交版本，填充 MvccInfo.writes。
    pub fn GetMvccInfoByKey(
        &mut self,
        key: &[u8],
        _unused: bool,
        mvccInfo: &mut kvrpcpb::MvccInfo,
    ) -> Result<(), DbReaderError> {
        let iter = self.GetIter()?;
        // 打开全版本并以最大读时间戳扫描该键的全部写入。
        iter.SetAllVersions(true);
        iter.SetReadTS(u64::MAX);
        iter.Seek(key)?;
        while iter.Valid() {
            let item = iter
                .Item()
                .ok_or_else(|| DbReaderError::backend("valid iterator has no item"))?;
            if item.Key() != key {
                break;
            }
            let val = item.ValueCopy()?;
            let userMeta = mvcc::DBUserMeta(item.UserMeta().to_vec());
            let item_type = if val.is_empty() {
                kvrpcpb::Op::Del
            } else {
                kvrpcpb::Op::Put
            };
            mvccInfo.writes.push(kvrpcpb::MvccWrite {
                r_type: item_type,
                start_ts: userMeta.StartTS(),
                commit_ts: userMeta.CommitTS(),
                short_value: val,
                ..Default::default()
            });
            iter.Next();
        }
        Ok(())
    }

    // Get gets a value with the key and start ts.
    /// 按键与 start_ts（事务开始时间戳）读取可见值及用户元数据。
    pub fn Get(
        &mut self,
        key: &[u8],
        startTS: u64,
    ) -> Result<Option<(Vec<u8>, mvcc::DBUserMeta)>, DbReaderError> {
        self.setReadTS(if self.RcCheckTS { u64::MAX } else { startTS });
        let Some(item) = self.txn.Get(key)? else {
            return Ok(None);
        };
        self.CheckWriteItemForRcCheckTSRead(startTS, Some(item.as_ref()))?;
        let val = item.Value()?;
        Ok(Some((val, mvcc::DBUserMeta(item.UserMeta().to_vec()))))
    }

    // GetIter returns the forward iterator of a DBReader.
    /// 懒创建并返回正向迭代器。
    pub fn GetIter(&mut self) -> Result<&mut (dyn DBIterator + '_), DbReaderError> {
        if self.iter.is_none() {
            self.iter = Some(NewIterator(
                self.txn.as_mut(),
                false,
                &self.StartKey,
                &self.EndKey,
            )?);
        }
        match self.iter.as_deref_mut() {
            Some(iter) => Ok(iter),
            None => unreachable!(),
        }
    }

    // GetExtraIter returns the extra iterator of a DBReader.
    /// 额外迭代器：将 Start/End 首字节加一，用于临时键空间等。
    pub fn GetExtraIter(&mut self) -> Result<&mut (dyn DBIterator + '_), DbReaderError> {
        if self.extraIter.is_none() {
            let mut rbStartKey = self.StartKey.clone();
            if let Some(first) = rbStartKey.first_mut() {
                *first = first.wrapping_add(1);
            }
            let mut rbEndKey = self.EndKey.clone();
            if let Some(first) = rbEndKey.first_mut() {
                *first = first.wrapping_add(1);
            }
            self.extraIter = Some(NewIterator(
                self.txn.as_mut(),
                false,
                &rbStartKey,
                &rbEndKey,
            )?);
        }
        match self.extraIter.as_deref_mut() {
            Some(iter) => Ok(iter),
            None => unreachable!(),
        }
    }

    /// 懒创建并返回反向迭代器。
    fn getReverseIter(&mut self) -> Result<&mut (dyn DBIterator + '_), DbReaderError> {
        if self.revIter.is_none() {
            self.revIter = Some(NewIterator(
                self.txn.as_mut(),
                true,
                &self.StartKey,
                &self.EndKey,
            )?);
        }
        match self.revIter.as_deref_mut() {
            Some(iter) => Ok(iter),
            None => unreachable!(),
        }
    }

    // BatchGet batch gets keys.
    /// 批量点查：对每个键回调 f(key, value, userMeta, error)。
    pub fn BatchGet(&mut self, keys: &[Vec<u8>], startTS: u64, f: &mut BatchGetFunc<'_>) {
        self.setReadTS(if self.RcCheckTS { u64::MAX } else { startTS });
        let items = match self.txn.MultiGet(keys) {
            Ok(items) => items,
            Err(error) => {
                // MultiGet 整体失败时对每个键回传同一错误。
                let userMeta = mvcc::DBUserMeta::default();
                for key in keys {
                    f(key, None, &userMeta, Some(&error));
                }
                return;
            }
        };

        // Go declares `err` outside this loop. Therefore a missing item keeps
        // the previous item's Value/RC-check error until a later non-nil item
        // assigns `err` again.
        let mut error = None;
        for (index, item) in items.into_iter().enumerate() {
            let key = &keys[index];
            let mut val = None;
            let mut userMeta = mvcc::DBUserMeta::default();
            if let Some(item) = item {
                match item.Value() {
                    Ok(value) => {
                        val = Some(value);
                        error = None;
                    }
                    Err(err) => error = Some(err),
                }
                if error.is_none() {
                    error = self
                        .CheckWriteItemForRcCheckTSRead(startTS, Some(item.as_ref()))
                        .err();
                }
                if error.is_none() {
                    userMeta = mvcc::DBUserMeta(item.UserMeta().to_vec());
                }
            }
            f(key, val.as_deref(), &userMeta, error.as_ref());
        }
    }

    // Scan scans the key range with the given ScanProcessor.
    /// 正向扫描 `[startKey, endKey)`，经 ScanProcessor 处理，受 limit 限制。
    pub fn Scan(
        &mut self,
        startKey: &[u8],
        endKey: &[u8],
        limit: isize,
        startTS: u64,
        proc: &mut dyn ScanProcessor,
    ) -> Result<(), DbReaderError> {
        self.setReadTS(if self.RcCheckTS { u64::MAX } else { startTS });
        let skipValue = proc.SkipValue();
        self.GetIter()?.Seek(startKey)?;
        let mut count = 0;
        loop {
            let item = {
                let iter = self.GetIter()?;
                if !iter.Valid() {
                    break;
                }
                iter.Item()
                    .ok_or_else(|| DbReaderError::backend("valid iterator has no item"))?
            };
            if exceedEndKey(item.Key(), endKey) {
                break;
            }
            self.CheckWriteItemForRcCheckTSRead(startTS, Some(item.as_ref()))?;
            // 空条目（删除占位）跳过。
            if item.IsEmpty() {
                self.GetIter()?.Next();
                continue;
            }
            let value = if skipValue { Vec::new() } else { item.Value()? };
            match proc.Process(item.Key(), &value, item.Version()) {
                Ok(()) => {}
                Err(DbReaderError::ScanBreak) => break,
                Err(error) => return Err(error),
            }
            count += 1;
            if count >= limit {
                break;
            }
            self.GetIter()?.Next();
        }
        Ok(())
    }

    // GetKeyByStartTs gets a key with the start ts.
    /// 在范围内查找 userMeta.StartTS 等于 startTs 的第一个键。
    pub fn GetKeyByStartTs(
        &mut self,
        startKey: &[u8],
        endKey: &[u8],
        startTs: u64,
    ) -> Result<Option<Vec<u8>>, DbReaderError> {
        let iter = self.GetIter()?;
        iter.SetAllVersions(true);
        iter.SetReadTS(u64::MAX);
        iter.Seek(startKey)?;
        while iter.Valid() {
            let item = iter
                .Item()
                .ok_or_else(|| DbReaderError::backend("valid iterator has no item"))?;
            if !endKey.is_empty() && item.Key() >= endKey {
                break;
            }
            let meta = mvcc::DBUserMeta(item.UserMeta().to_vec());
            if meta.StartTS() == startTs {
                return Ok(Some(item.KeyCopy()));
            }
            iter.Next();
        }
        Ok(None)
    }

    // ReverseScan implements the MVCCStore interface. The search range is
    // [startKey, endKey).
    /// 反向扫描，范围 `[startKey, endKey)`；首条若恰为 endKey 则跳过（排他上界）。
    pub fn ReverseScan(
        &mut self,
        startKey: &[u8],
        endKey: &[u8],
        limit: isize,
        startTS: u64,
        proc: &mut dyn ScanProcessor,
    ) -> Result<(), DbReaderError> {
        let skipValue = proc.SkipValue();
        self.setReadTS(if self.RcCheckTS { u64::MAX } else { startTS });
        self.getReverseIter()?.Seek(endKey)?;
        let mut count = 0;
        loop {
            let item = {
                let iter = self.getReverseIter()?;
                if !iter.Valid() {
                    break;
                }
                iter.Item()
                    .ok_or_else(|| DbReaderError::backend("valid iterator has no item"))?
            };
            if item.Key() < startKey {
                break;
            }
            // 反向从 endKey 起 Seek，第一条等于 endKey 时跳过以保持半开区间。
            if count == 0 && item.Key() == endKey {
                self.getReverseIter()?.Next();
                continue;
            }
            self.CheckWriteItemForRcCheckTSRead(startTS, Some(item.as_ref()))?;
            if item.IsEmpty() {
                self.getReverseIter()?.Next();
                continue;
            }
            let value = if skipValue { Vec::new() } else { item.Value()? };
            match proc.Process(item.Key(), &value, item.Version()) {
                Ok(()) => {}
                Err(DbReaderError::ScanBreak) => break,
                Err(error) => return Err(error),
            }
            count += 1;
            if count >= limit {
                break;
            }
            self.getReverseIter()?.Next();
        }
        Ok(())
    }

    // CheckWriteItemForRcCheckTSRead checks the data version if RcCheckTS
    // isolation is used.
    /// RC CheckTS：若条目 commit_ts > readTS 则报写冲突。
    pub fn CheckWriteItemForRcCheckTSRead(
        &self,
        readTS: u64,
        item: Option<&dyn DBItem>,
    ) -> Result<(), DbReaderError> {
        let Some(item) = item else {
            return Ok(());
        };
        if !self.RcCheckTS {
            return Ok(());
        }
        let userMeta = mvcc::DBUserMeta(item.UserMeta().to_vec());
        if userMeta.CommitTS() > readTS {
            return Err(DbReaderError::Conflict(kverrors::ErrConflict {
                StartTS: readTS,
                ConflictTS: userMeta.StartTS(),
                ConflictCommitTS: userMeta.CommitTS(),
                Key: Vec::new(),
                Reason: kvrpcpb::WriteConflictReason::RcCheckTs,
            }));
        }
        Ok(())
    }

    // GetTxn gets the read transaction of the DBReader.
    /// 返回内部只读事务可变引用。
    pub fn GetTxn(&mut self) -> &mut dyn ReadTxn {
        self.txn.as_mut()
    }

    // Close closes the DBReader.
    /// 关闭全部迭代器并 Discard 事务。
    pub fn Close(&mut self) {
        if let Some(iter) = self.iter.as_deref_mut() {
            iter.Close();
        }
        if let Some(iter) = self.revIter.as_deref_mut() {
            iter.Close();
        }
        if let Some(iter) = self.extraIter.as_deref_mut() {
            iter.Close();
        }
        self.txn.Discard();
    }
}

// BatchGetFunc defines a batch get function.
/// BatchGet 回调：键、可选值、用户元数据、可选错误。
pub type BatchGetFunc<'a> =
    dyn FnMut(&[u8], Option<&[u8]>, &mvcc::DBUserMeta, Option<&DbReaderError>) + 'a;

// ErrScanBreak is returned by ScanProcessor to break the scan loop.
/// 构造 ScanBreak，供处理器提前结束扫描。
pub fn ErrScanBreak() -> DbReaderError {
    DbReaderError::ScanBreak
}

// ScanFunc accepts key and value and should not keep references to them.
/// 扫描回调类型；不得持久持有 key/value 引用。
pub type ScanFunc<'a> = dyn FnMut(&[u8], &[u8]) -> Result<(), DbReaderError> + 'a;

// ScanProcessor processes a key/value pair.
/// 扫描处理器：处理键值并可声明是否跳过加载 value。
pub trait ScanProcessor {
    // Process accepts key and value and should not keep references to them.
    /// 处理一对键值；返回 ScanBreak 可中断扫描。
    fn Process(&mut self, key: &[u8], value: &[u8], commitTS: u64) -> Result<(), DbReaderError>;

    // SkipValue returns whether DBReader can avoid loading values.
    /// 为 true 时 DBReader 可不加载 value。
    fn SkipValue(&self) -> bool;
}

/// 判断当前键是否已到达或越过 endKey（空 endKey 表示无上界）。
fn exceedEndKey(current: &[u8], endKey: &[u8]) -> bool {
    !endKey.is_empty() && current >= endKey
}
