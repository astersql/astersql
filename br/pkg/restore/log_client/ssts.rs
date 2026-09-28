// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc. Licensed under Apache-2.0.

//! SST collection types for log restore, matching `ssts.go`.
//!
//! 本模块对齐 Go `br/pkg/restore/log_client/ssts.go`，抽象日志恢复中两类 SST 集合：
//! - `CompactedSSTs`：compact-log-backup 产物（`LogFileSubcompaction`）；
//! - `CopiedSST`：直接拷贝/ingest 的单文件 SST，可能带表 ID 重写信息。
//!
//! 统一通过 `SSTs` trait 暴露 Type / TableID / Get/SetSSTs；`CopiedSST` 额外实现
//! `RewrittenSSTs`，供过滤阶段按上游表 ID 做 rewrite。`TableID` 从起止键解码并缓存，
//! 跨表 SST 会 panic（与 Go 一致，当前不支持）。

use std::fmt;
use std::sync::atomic::{AtomicI64, Ordering};

use crate::stubs::backuppb::{File, LogFileSubcompaction, RewrittenTableID};
use crate::stubs::tablecodec;

/// compact 产物类型标记，对齐 Go `CompactedSSTsType`。
pub const CompactedSSTsType: i32 = 1;
/// 拷贝/ingest SST 类型标记，对齐 Go `CopiedSSTsType`。
pub const CopiedSSTsType: i32 = 2;

/// Extension for SSTs that need extra key rewriting when filtering.
///
/// 过滤时若需按「重写目标表」判定归属，通过本扩展读取 Upstream 表 ID。
pub trait RewrittenSSTs {
    /// 返回 rewrite 目标表 ID；无 Upstream 时回落自身 TableID。
    fn RewrittenTo(&self) -> i64;
}

/// Collection of SST files to restore.
///
/// 日志恢复管线对 SST 批次的统一视图；`as_rewritten` 默认 None。
pub trait SSTs: fmt::Display {
    /// 区分 Compacted / Copied，供调度与指标分组。
    fn Type(&self) -> i32;
    /// 该批 SST 所属表；Copied 路径从键解码并缓存。
    fn TableID(&self) -> i64;
    fn GetSSTs(&self) -> Vec<File>;
    fn SetSSTs(&mut self, files: Vec<File>);
    /// CopiedSST 覆写为 Some(self)；Compacted 保持默认 None。
    fn as_rewritten(&self) -> Option<&dyn RewrittenSSTs> {
        None
    }
}

/// compact-log-backup 子压缩产物包装；TableID 取自 Meta.TableId。
pub struct CompactedSSTs {
    /// 原始 Subcompaction；SstOutputs 为可替换的文件列表。
    pub inner: LogFileSubcompaction,
}

impl CompactedSSTs {
    pub fn new(inner: LogFileSubcompaction) -> Self {
        Self { inner }
    }
}

impl fmt::Display for CompactedSSTs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CompactedSSTs: {}", self.inner.Meta)
    }
}

impl SSTs for CompactedSSTs {
    fn Type(&self) -> i32 {
        CompactedSSTsType
    }
    fn TableID(&self) -> i64 {
        self.inner.Meta.TableId
    }
    fn GetSSTs(&self) -> Vec<File> {
        self.inner.SstOutputs.clone()
    }
    fn SetSSTs(&mut self, files: Vec<File>) {
        self.inner.SstOutputs = files;
    }
}

/// 单文件拷贝 SST：可选 File + 表 ID 重写元数据 + TableID 原子缓存。
pub struct CopiedSST {
    /// 至多一个 File；空表示已被过滤清空。
    pub File: Option<File>,
    /// 上游/下游表 ID 映射；Upstream>0 时 RewrittenTo 优先用之。
    pub Rewritten: RewrittenTableID,
    /// 0 表示未缓存；解码成功后写入，避免重复 DecodeTableID。
    cachedTableID: AtomicI64,
}

impl CopiedSST {
    /// `cachedTableID` 初始为 0，首次 TableID() 时填充。
    pub fn new(file: Option<File>, rewritten: RewrittenTableID) -> Self {
        Self {
            File: file,
            Rewritten: rewritten,
            cachedTableID: AtomicI64::new(0),
        }
    }
}

impl fmt::Display for CopiedSST {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.File {
            Some(file) => write!(f, "CopiedSSTs: {file:?}"),
            None => write!(f, "CopiedSSTs: <nil>"),
        }
    }
}

impl SSTs for CopiedSST {
    fn Type(&self) -> i32 {
        CopiedSSTsType
    }

    fn TableID(&self) -> i64 {
        let cached = self.cachedTableID.load(Ordering::SeqCst);
        if cached != 0 {
            return cached;
        }
        // Go dereferences s.File here, so an empty CopiedSST is a caller error.
        let file = self
            .File
            .as_ref()
            .expect("CopiedSST.TableID called without a file");
        let id = tablecodec::DecodeTableID(&file.StartKey);
        let id2 = tablecodec::DecodeTableID(&file.EndKey);
        // 起止键跨表：与 Go 相同，直接 panic，不静默截断。
        if id != id2 {
            panic!(
                "yet restoring a SST with two adjacent tables not supported, they are {} and {} (start key = {}; end key = {})",
                id,
                id2,
                hex::encode(&file.StartKey),
                hex::encode(&file.EndKey),
            );
        }
        self.cachedTableID.store(id, Ordering::SeqCst);
        id
    }

    fn GetSSTs(&self) -> Vec<File> {
        self.File.clone().into_iter().collect()
    }

    fn SetSSTs(&mut self, fs: Vec<File>) {
        // 契约：CopiedSST 至多持有一个文件；>1 视为编程错误。
        match fs.len() {
            0 => self.File = None,
            1 => self.File = fs.into_iter().next(),
            _ => panic!("Too many files passed to AddedSSTs.SetSSTs."),
        }
    }

    fn as_rewritten(&self) -> Option<&dyn RewrittenSSTs> {
        Some(self)
    }
}

impl RewrittenSSTs for CopiedSST {
    fn RewrittenTo(&self) -> i64 {
        // Upstream 有效时表示已 rewrite 到该上游表视角。
        if self.Rewritten.Upstream > 0 {
            return self.Rewritten.Upstream;
        }
        self.TableID()
    }
}
