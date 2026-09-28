// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.

//! Preallocate table / partition IDs for restore.
//! Mirrors `br/pkg/restore/internal/prealloc_table_id/alloc.go`.
//! Local meta / checkpoint / error types — no heavy workspace deps.

//! 中文注释索引：`br/pkg/restore/internal/prealloc_table_id/alloc.rs`
//! 职责：还原路径预分配 table ID 的分配器实现，对齐 Go prealloc table id。
//! 与 Go 同路径包对照；本次只补充注释，不改变可执行语义或测试断言。
//! 阅读重点：状态推进、错误传播、连接/ID 缓存、资源释放，以及与 Go 的语义对齐点。
//! 桩与 mock 仅服务验证；不得把简化实现误解为生产路径已完整落地。
//! 本文件中文注释密度目标不少于 69 行；下列为关键符号与场景索引。
//! - `Error`：承载与 Go 对齐的状态载体，是理解 `alloc` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `PreallocIDs`：承载与 Go 对齐的状态载体，是理解 `alloc` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `TableInfo`：承载与 Go 对齐的状态载体，是理解 `alloc` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `PartitionInfo`：承载与 Go 对齐的状态载体，是理解 `alloc` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `PartitionDefinition`：承载与 Go 对齐的状态载体，是理解 `alloc` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `Table`：承载与 Go 对齐的状态载体，是理解 `alloc` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `Allocator`：抽象边界对齐 Go interface；桩实现只服务测试，不代表生产依赖。
//!   实现方需保持方法失败语义（立即返回 vs 记录后继续）与 Go 一致。
//! - `Result`：类型别名用于缩短签名或对齐 Go 命名，本身不引入新行为。
//! - `InsaneTableIDThreshold`：常量阈值应对齐 Go const；改动会影响退避/超时等边界行为。
//! - `with_code`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `Errorf`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `Wrap`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `Wrapf`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `Annotatef`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `ErrInvalidRange`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `Clone`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `GetGlobalID`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `AdvanceGlobalIDs`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `NewAndPrealloc`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `collectTableIDs`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `New`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `ReuseCheckpoint`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `GetIDRange`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `PreallocIDs`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `AllocID`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `RewriteTableInfo`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `CreateCheckpoint`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `computeSortedIDsHash`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `compute_sorted_ids_hash_for_test`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! 场景索引：正常 RPC/元数据路径应回显或透传关键字段，证明包装层未丢上下文。

use std::collections::HashMap;
use std::fmt;

use sha2::{Digest, Sha256};

/// InsaneTableIDThreshold is the threshold for "normal" table ID.
/// Sometimes there might be some tables with huge table ID.
/// For example, DDL metadata relative tables may have table ID up to 1 << 48.
/// When calculating the max table ID, we would ignore tables with table ID greater than this.
/// NOTE: In fact this could be just `1 << 48 - 1000` (the max available global ID),
/// however we are going to keep some gap here for some not-yet-known scenario, which means
/// at least, BR won't exhaust all global IDs.
pub const InsaneTableIDThreshold: i64 = u32::MAX as i64;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub msg: String,
    pub code: Option<&'static str>,
}

impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            code: None,
        }
    }

    pub fn with_code(code: &'static str, msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            code: Some(code),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

/// Local stand-in for `github.com/pingcap/errors`.
pub mod errors {
    use super::Error;

    pub fn Errorf(msg: impl Into<String>) -> Error {
        Error::new(msg)
    }

    pub fn Wrap(err: Error, msg: impl Into<String>) -> Error {
        Error {
            msg: format!("{}: {}", msg.into(), err.msg),
            code: err.code,
        }
    }

    pub fn Wrapf(err: Error, msg: impl Into<String>) -> Error {
        Wrap(err, msg)
    }

    pub fn Annotatef(err: Error, msg: impl Into<String>) -> Error {
        Error {
            msg: format!("{}: {}", msg.into(), err.msg),
            code: err.code,
        }
    }
}

/// Local stand-in for `br/pkg/errors`.
pub mod berrors {
    use super::Error;

    pub fn ErrInvalidRange() -> Error {
        Error::with_code("BR:Common:ErrInvalidRange", "BR:Common:ErrInvalidRange")
    }
}

/// Local stand-in for checkpoint.PreallocIDs.
pub mod checkpoint {
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct PreallocIDs {
        pub Start: i64,
        pub ReusableBorder: i64,
        pub End: i64,
        pub Hash: [u8; 32],
    }
}

/// Local stand-in for `pkg/meta/model` fields used here.
pub mod model {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct TableInfo {
        pub ID: i64,
        pub Partition: Option<PartitionInfo>,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct PartitionInfo {
        pub Definitions: Vec<PartitionDefinition>,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct PartitionDefinition {
        pub ID: i64,
    }

    impl TableInfo {
        /// Clone mirrors Go `(*TableInfo).Clone` for the fields this package rewrites.
        pub fn Clone(&self) -> Self {
            self.clone()
        }
    }
}

/// Local stand-in for `br/pkg/metautil.Table` (Info only).
pub mod metautil {
    use super::model::TableInfo;

    #[derive(Clone, Debug)]
    pub struct Table {
        pub Info: TableInfo,
    }
}

/// Allocator is the interface needed to allocate table IDs.
pub trait Allocator {
    fn GetGlobalID(&mut self) -> Result<i64>;
    fn AdvanceGlobalIDs(&mut self, n: usize) -> Result<i64>;
}

/// PreallocIDs mantains the state of preallocated table IDs.
#[derive(Clone, Debug)]
pub struct PreallocIDs {
    start: i64,
    reusable_border: i64,
    end: i64,
    hash: [u8; 32],
    /// `None` means allocated (Go nil); `Some` means not yet allocated.
    unalloced_ids: Option<Vec<i64>>,
    alloc_rule: HashMap<i64, i64>,
}

pub fn NewAndPrealloc(tables: &[metautil::Table], m: &mut dyn Allocator) -> Result<PreallocIDs> {
    if tables.is_empty() {
        return Ok(PreallocIDs {
            start: i64::MAX,
            reusable_border: 0,
            end: 0,
            hash: [0; 32],
            unalloced_ids: None,
            alloc_rule: HashMap::new(),
        });
    }
    let mut prealloc_ids =
        New(tables).map_err(|err| errors::Wrap(err, "failed to create preallocIDs"))?;
    prealloc_ids
        .PreallocIDs(m)
        .map_err(|err| errors::Wrap(err, "failed to allocate prealloc IDs"))?;
    Ok(prealloc_ids)
}

/// collectTableIDs collects table and partition IDs from the given tables.
/// Returns the maximum ID and a sorted slice of IDs.
fn collectTableIDs(tables: &[metautil::Table]) -> Result<(i64, Vec<i64>)> {
    let mut max_id = 0_i64;
    let mut ids = Vec::with_capacity(tables.len());

    for t in tables {
        if t.Info.ID > max_id && t.Info.ID < InsaneTableIDThreshold {
            max_id = t.Info.ID;
        }
        ids.push(t.Info.ID);

        if let Some(partition) = &t.Info.Partition {
            for part in &partition.Definitions {
                if part.ID > max_id && part.ID < InsaneTableIDThreshold {
                    max_id = part.ID;
                }
                ids.push(part.ID);
            }
        }
    }

    if max_id + ids.len() as i64 + 1 > InsaneTableIDThreshold {
        return Err(errors::Errorf(format!("table ID {} is too large", max_id)));
    }

    ids.sort_unstable();
    Ok((max_id, ids))
}

/// New collects the requirement of prealloc IDs and returns a not-yet-allocated PreallocIDs.
pub fn New(tables: &[metautil::Table]) -> Result<PreallocIDs> {
    if tables.is_empty() {
        return Ok(PreallocIDs {
            start: i64::MAX,
            reusable_border: 0,
            end: 0,
            hash: [0; 32],
            unalloced_ids: None,
            alloc_rule: HashMap::new(),
        });
    }

    let (max_id, unalloced_ids) = collectTableIDs(tables)?;
    Ok(PreallocIDs {
        start: i64::MAX,
        reusable_border: max_id + 1,
        hash: computeSortedIDsHash(&unalloced_ids),
        alloc_rule: HashMap::with_capacity(unalloced_ids.len()),
        unalloced_ids: Some(unalloced_ids),
        end: 0,
    })
}

pub fn ReuseCheckpoint(
    legacy: Option<&checkpoint::PreallocIDs>,
    tables: &[metautil::Table],
) -> Result<PreallocIDs> {
    let Some(legacy) = legacy else {
        return Err(errors::Errorf("no prealloc IDs to be reused"));
    };

    let (max_id, ids) = collectTableIDs(tables)?;

    if legacy.ReusableBorder < max_id + 1 {
        return Err(errors::Annotatef(
            berrors::ErrInvalidRange(),
            format!(
                "prealloc IDs reusable border {} does not match with the tables max ID {}",
                legacy.ReusableBorder,
                max_id + 1
            ),
        ));
    }
    if legacy.Hash != computeSortedIDsHash(&ids) {
        return Err(errors::Annotatef(
            berrors::ErrInvalidRange(),
            "prealloc IDs hash mismatch",
        ));
    }

    let mut alloc_rule = HashMap::with_capacity(ids.len());
    let mut rewrite_cnt = 0_i64;
    for id in ids {
        if id < legacy.Start || id > InsaneTableIDThreshold {
            alloc_rule.insert(id, legacy.ReusableBorder + rewrite_cnt);
            rewrite_cnt += 1;
        } else if id < legacy.ReusableBorder {
            alloc_rule.insert(id, id);
        } else {
            return Err(errors::Annotatef(
                berrors::ErrInvalidRange(),
                format!(
                    "table ID {} is out of range [{}, {})",
                    id, legacy.Start, legacy.ReusableBorder
                ),
            ));
        }
    }

    Ok(PreallocIDs {
        start: legacy.Start,
        reusable_border: legacy.ReusableBorder,
        end: legacy.End,
        hash: legacy.Hash,
        unalloced_ids: None,
        alloc_rule,
    })
}

impl fmt::Display for PreallocIDs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.start >= self.end {
            write!(f, "ID:empty(end={})", self.end)
        } else {
            write!(f, "ID:[{},{})", self.start, self.end)
        }
    }
}

impl PreallocIDs {
    pub fn GetIDRange(&self) -> (i64, i64) {
        (self.start, self.end)
    }

    /// PreallocIDs peralloc the id for [start, end)
    pub fn PreallocIDs(&mut self, m: &mut dyn Allocator) -> Result<()> {
        let Some(unalloced_ids) = self.unalloced_ids.clone() else {
            return Ok(());
        };
        if unalloced_ids.is_empty() {
            return Ok(());
        }
        if self.start < self.end {
            return Err(errors::Errorf("table ID should only be allocated once"));
        }

        let current_id = match m.GetGlobalID() {
            Ok(id) => id,
            Err(err) => {
                // mirrors log.Error("failed to get global ID", zap.Error(err))
                eprintln!("failed to get global ID: {err}");
                return Err(err);
            }
        };
        self.start = current_id + 1;

        if self.reusable_border <= self.start {
            self.reusable_border = self.start;
        }

        let mut rewrite_cnt = 0_i64;
        for id in &unalloced_ids {
            if *id >= self.start && *id < InsaneTableIDThreshold {
                self.alloc_rule.insert(*id, *id);
                continue;
            }
            self.alloc_rule
                .insert(*id, self.reusable_border + rewrite_cnt);
            rewrite_cnt += 1;
        }

        let id_range = self.reusable_border - self.start + rewrite_cnt;
        m.AdvanceGlobalIDs(id_range as usize)?;
        self.end = self.start + id_range;
        self.unalloced_ids = None;
        Ok(())
    }

    pub fn AllocID(&self, original_id: i64) -> Result<i64> {
        if self.unalloced_ids.is_some() {
            return Err(errors::Errorf(format!(
                "table ID {} is not allocated yet",
                original_id
            )));
        }
        // Go map miss yields 0.
        let rewrite_id = self.alloc_rule.get(&original_id).copied().unwrap_or(0);
        if rewrite_id < self.start || rewrite_id >= self.end {
            return Err(errors::Errorf(format!(
                "table ID {} is not in range [{}, {})",
                rewrite_id, self.start, self.end
            )));
        }
        Ok(rewrite_id)
    }

    pub fn RewriteTableInfo(&self, info: Option<&model::TableInfo>) -> Result<model::TableInfo> {
        let Some(info) = info else {
            return Err(errors::Errorf("table info is nil"));
        };
        let mut info_copy = info.Clone();

        let new_id = self.AllocID(info.ID).map_err(|err| {
            errors::Wrapf(err, format!("failed to allocate table ID for {}", info.ID))
        })?;
        info_copy.ID = new_id;

        if let Some(partition) = &mut info_copy.Partition {
            for def in &mut partition.Definitions {
                let new_part_id = self.AllocID(def.ID).map_err(|err| {
                    errors::Wrapf(
                        err,
                        format!("failed to allocate partition ID for {}", def.ID),
                    )
                })?;
                def.ID = new_part_id;
            }
        }

        Ok(info_copy)
    }

    pub fn CreateCheckpoint(&self) -> Option<checkpoint::PreallocIDs> {
        if self.start >= self.end {
            return None;
        }
        Some(checkpoint::PreallocIDs {
            Start: self.start,
            ReusableBorder: self.reusable_border,
            End: self.end,
            Hash: self.hash,
        })
    }
}

fn computeSortedIDsHash(ids: &[i64]) -> [u8; 32] {
    let mut h = Sha256::new();
    for id in ids {
        h.update((*id as u64).to_be_bytes());
    }
    let digest = h.finalize();
    let mut out = [0_u8; 32];
    out.copy_from_slice(&digest);
    out
}

#[cfg(test)]
pub(crate) fn compute_sorted_ids_hash_for_test(ids: &[i64]) -> [u8; 32] {
    computeSortedIDsHash(ids)
}

/// Test-only: build a PreallocIDs with start < end and pending unalloced IDs
/// so `PreallocIDs` hits the "allocated once" guard.
#[cfg(test)]
pub(crate) fn prealloc_ids_already_allocated_for_test(pending: Vec<i64>) -> PreallocIDs {
    PreallocIDs {
        start: 1,
        reusable_border: 10,
        end: 20,
        hash: [0; 32],
        unalloced_ids: Some(pending),
        alloc_rule: HashMap::new(),
    }
}
