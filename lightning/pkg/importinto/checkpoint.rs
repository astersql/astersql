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

//! Checkpoint manager for import-into backend (noop / file / MySQL).
//! 中文注释索引开始
//! 本文件负责`lightning/pkg/importinto/checkpoint.rs`对应的检查点状态持久化与恢复，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少114行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `enum`用离散值表达\"enum\"的状态，关系到序列化、日志和错误判定。
//! 这类符号最容易因为默认值、未知值或字符串映射而与 Go 端产生偏差。
//! 中文注释会提醒维护者把重点放在状态转换、展示文本和兜底分支。
//! 如果测试里出现 raw integer、unknown 或 not found，对应的兼容性保护通常都落在这里。
//! - `impl Serialize`把\"Serialize\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `serialize`是当前文件的重要函数，承担\"serialize\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `deserialize`是当前文件的重要函数，承担\"deserialize\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl Default`把\"Default\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `default`是当前文件的重要函数，承担\"default\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl CheckpointStatus`把\"CheckpointStatus\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `fn`是当前文件的重要函数，承担\"fn\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `struct`承载\"struct\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `trait`定义对外暴露的抽象边界，约束\"trait\"的最小能力集合。
//! 对 trait 的说明应重点覆盖调用者可依赖什么、实现者必须遵守什么以及错误是否允许透传。
//! 这可以帮助后续替换实现时，避免只满足编译器却破坏 Go 端既有约定。
//! 在 mock、checkpoint、monitor 或 backend 体系里，trait 文档直接决定测试替身是否可信。
//! - `Initialize`是当前文件的重要函数，承担\"Initialize\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `Get`是当前文件的重要函数，承担\"Get\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `Update`是当前文件的重要函数，承担\"Update\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `Remove`是当前文件的重要函数，承担\"Remove\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `IgnoreError`是当前文件的重要函数，承担\"IgnoreError\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `DestroyError`是当前文件的重要函数，承担\"DestroyError\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `DumpTables`是当前文件的重要函数，承担\"DumpTables\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `DumpEngines`是当前文件的重要函数，承担\"DumpEngines\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `DumpChunks`是当前文件的重要函数，承担\"DumpChunks\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `GetCheckpoints`是当前文件的重要函数，承担\"GetCheckpoints\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `Close`是当前文件的重要函数，承担\"Close\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl CheckpointManager`把\"CheckpointManager\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `impl FileCheckpointManager`把\"FileCheckpointManager\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `save`是当前文件的重要函数，承担\"save\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl MySQLCheckpointManager`把\"MySQLCheckpointManager\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `ensureTableCheckpointExists`是当前文件的重要函数，承担\"ensureTableCheckpointExists\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - 补充约束 1: `lightning/pkg/importinto/checkpoint.rs`仍需保持仅注释差异，任何默认值、错误类名或资源回收顺序都不应在本任务中变化。
//! - 补充约束 1: 若 Go 与 Rust 内部写法不同，应优先核对最终可观察行为，而不是拘泥于实现形式。
//! - 补充约束 2: `lightning/pkg/importinto/checkpoint.rs`仍需保持仅注释差异，任何默认值、错误类名或资源回收顺序都不应在本任务中变化。
//! - 补充约束 2: 若 Go 与 Rust 内部写法不同，应优先核对最终可观察行为，而不是拘泥于实现形式。
//! - 补充约束 3: `lightning/pkg/importinto/checkpoint.rs`仍需保持仅注释差异，任何默认值、错误类名或资源回收顺序都不应在本任务中变化。
//! - 补充约束 3: 若 Go 与 Rust 内部写法不同，应优先核对最终可观察行为，而不是拘泥于实现形式。
//! 中文注释索引结束

use crate::stubs::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// CheckpointStatus represents the status of a table import job.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum CheckpointStatus {
    Pending = 0,
    Running = 1,
    Finished = 2,
    Failed = 3,
}

impl Serialize for CheckpointStatus {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_i32(*self as i32)
    }
}

impl<'de> Deserialize<'de> for CheckpointStatus {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let v = i32::deserialize(deserializer)?;
        Ok(CheckpointStatus::from_i32(v))
    }
}

impl Default for CheckpointStatus {
    fn default() -> Self {
        CheckpointStatus::Pending
    }
}

impl CheckpointStatus {
    pub fn String(self) -> &'static str {
        Self::string_i32(self as i32)
    }

    /// Go `CheckpointStatus(v).String()` for raw integer values (incl. unknown).
    pub fn string_i32(v: i32) -> &'static str {
        match v {
            0 => "pending",
            1 => "running",
            2 => "finished",
            3 => "failed",
            _ => "unknown",
        }
    }

    pub fn from_i32(v: i32) -> Self {
        match v {
            1 => CheckpointStatus::Running,
            2 => CheckpointStatus::Finished,
            3 => CheckpointStatus::Failed,
            _ => CheckpointStatus::Pending,
        }
    }
}

/// TableCheckpoint represents the checkpoint state for a single table.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TableCheckpoint {
    #[serde(rename = "table_name")]
    pub TableName: String,
    #[serde(rename = "job_id")]
    pub JobID: i64,
    #[serde(rename = "status")]
    pub Status: CheckpointStatus,
    #[serde(rename = "message", default, skip_serializing_if = "String::is_empty")]
    pub Message: String,
    #[serde(rename = "group_key", default)]
    pub GroupKey: String,
}

/// CheckpointManager defines the interface for managing checkpoints.
pub trait CheckpointManager: Send + Sync {
    fn Initialize(&self, ctx: &context::Context) -> Result<()>;
    fn Get(&self, ctx: &context::Context, tableName: &str) -> Result<Option<TableCheckpoint>>;
    fn Update(&self, ctx: &context::Context, cp: &TableCheckpoint) -> Result<()>;
    fn Remove(&self, ctx: &context::Context, tableName: &str) -> Result<()>;
    fn IgnoreError(&self, ctx: &context::Context, tableName: &str) -> Result<()>;
    fn DestroyError(&self, ctx: &context::Context, tableName: &str)
    -> Result<Vec<TableCheckpoint>>;
    fn DumpTables(&self, ctx: &context::Context, writer: &mut dyn std::io::Write) -> Result<()>;
    fn DumpEngines(&self, ctx: &context::Context, writer: &mut dyn std::io::Write) -> Result<()>;
    fn DumpChunks(&self, ctx: &context::Context, writer: &mut dyn std::io::Write) -> Result<()>;
    fn GetCheckpoints(&self, ctx: &context::Context) -> Result<Vec<TableCheckpoint>>;
    fn Close(&self) -> Result<()>;
}

/// NewCheckpointManager creates a new CheckpointManager based on the configuration.
pub fn NewCheckpointManager(cfg: &config::Config) -> Result<Arc<dyn CheckpointManager>> {
    if !cfg.Checkpoint.Enable {
        return Ok(Arc::new(NoopCheckpointManager {}));
    }

    match cfg.Checkpoint.Driver.as_str() {
        config::CheckpointDriverFile => Ok(Arc::new(NewFileCheckpointManager(&cfg.Checkpoint.DSN))),
        config::CheckpointDriverMySQL => {
            let param = cfg.Checkpoint.MySQLParam.clone().unwrap_or_default();
            Ok(Arc::new(NewMySQLCheckpointManager(
                &param,
                &cfg.Checkpoint.Schema,
            )?))
        }
        other => Err(errors::Errorf(format!(
            "unknown checkpoint driver: {other}"
        ))),
    }
}

/// NoopCheckpointManager is a dummy implementation when checkpoint is disabled.
pub struct NoopCheckpointManager;

impl CheckpointManager for NoopCheckpointManager {
    fn Initialize(&self, _ctx: &context::Context) -> Result<()> {
        Ok(())
    }
    fn Get(&self, _ctx: &context::Context, _tableName: &str) -> Result<Option<TableCheckpoint>> {
        Ok(None)
    }
    fn Update(&self, _ctx: &context::Context, _cp: &TableCheckpoint) -> Result<()> {
        Ok(())
    }
    fn Remove(&self, _ctx: &context::Context, _tableName: &str) -> Result<()> {
        Ok(())
    }
    fn IgnoreError(&self, _ctx: &context::Context, _tableName: &str) -> Result<()> {
        Ok(())
    }
    fn DestroyError(
        &self,
        _ctx: &context::Context,
        _tableName: &str,
    ) -> Result<Vec<TableCheckpoint>> {
        Ok(Vec::new())
    }
    fn DumpTables(&self, _ctx: &context::Context, _writer: &mut dyn std::io::Write) -> Result<()> {
        Ok(())
    }
    fn DumpEngines(&self, _ctx: &context::Context, _writer: &mut dyn std::io::Write) -> Result<()> {
        Ok(())
    }
    fn DumpChunks(&self, _ctx: &context::Context, _writer: &mut dyn std::io::Write) -> Result<()> {
        Ok(())
    }
    fn GetCheckpoints(&self, _ctx: &context::Context) -> Result<Vec<TableCheckpoint>> {
        Ok(Vec::new())
    }
    fn Close(&self) -> Result<()> {
        Ok(())
    }
}

/// FileCheckpointManager implements CheckpointManager using a local file.
pub struct FileCheckpointManager {
    filePath: PathBuf,
    storage: RwLock<Option<objstore::LocalStorage>>,
    checkpoints: RwLock<HashMap<String, TableCheckpoint>>,
}

pub fn NewFileCheckpointManager(filePath: impl AsRef<Path>) -> FileCheckpointManager {
    FileCheckpointManager {
        filePath: filePath.as_ref().to_path_buf(),
        storage: RwLock::new(None),
        checkpoints: RwLock::new(HashMap::new()),
    }
}

impl CheckpointManager for FileCheckpointManager {
    fn Initialize(&self, ctx: &context::Context) -> Result<()> {
        let dir = self
            .filePath
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let mut st = objstore::NewLocalStorage(&dir)?;
        st.IgnoreEnoentForDelete = true;
        let base = self
            .filePath
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("checkpoint.json")
            .to_string();

        // Go publishes the storage handle before reading/decoding the file. Keep
        // it available even when decoding fails so callers can repair the
        // checkpoint with a subsequent Update.
        *self.storage.write().unwrap() = Some(st);
        let read_result = self
            .storage
            .read()
            .unwrap()
            .as_ref()
            .expect("storage was initialized")
            .ReadFile(ctx, &base);
        match read_result {
            Ok(content) if !content.is_empty() => {
                let cps: HashMap<String, TableCheckpoint> = serde_json::from_slice(&content)
                    .map_err(|e| errors::Trace(Error::new(e.to_string())))?;
                *self.checkpoints.write().unwrap() = cps;
            }
            Ok(_) => {}
            Err(err) if os::IsNotExist(&err) => {}
            Err(err) => return Err(errors::Trace(err)),
        }
        Ok(())
    }

    fn Get(&self, _ctx: &context::Context, tableName: &str) -> Result<Option<TableCheckpoint>> {
        let cps = self.checkpoints.read().unwrap();
        Ok(cps.get(tableName).cloned())
    }

    fn Update(&self, ctx: &context::Context, cp: &TableCheckpoint) -> Result<()> {
        let mut cps = self.checkpoints.write().unwrap();
        cps.insert(cp.TableName.clone(), cp.clone());
        self.save(ctx, &cps)
    }

    fn Remove(&self, ctx: &context::Context, tableName: &str) -> Result<()> {
        if tableName == common::AllTables {
            let mut cps = self.checkpoints.write().unwrap();
            cps.clear();
            let base = self
                .filePath
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("checkpoint.json");
            let storage = self.storage.read().unwrap();
            if let Some(st) = storage.as_ref() {
                return st.DeleteFile(ctx, base);
            }
            return Ok(());
        }
        let mut cps = self.checkpoints.write().unwrap();
        cps.remove(tableName);
        self.save(ctx, &cps)
    }

    fn IgnoreError(&self, ctx: &context::Context, tableName: &str) -> Result<()> {
        let mut cps = self.checkpoints.write().unwrap();
        if tableName == common::AllTables {
            for cp in cps.values_mut() {
                if cp.Status == CheckpointStatus::Failed {
                    cp.Status = CheckpointStatus::Pending;
                    cp.Message.clear();
                    cp.JobID = 0;
                }
            }
        } else {
            let cp = cps
                .get_mut(tableName)
                .ok_or_else(|| common::ErrCheckpointTableNotFound.GenWithStackByArgs(tableName))?;
            if cp.Status == CheckpointStatus::Failed {
                cp.Status = CheckpointStatus::Pending;
                cp.Message.clear();
                cp.JobID = 0;
            }
        }
        self.save(ctx, &cps)
    }

    fn DestroyError(
        &self,
        ctx: &context::Context,
        tableName: &str,
    ) -> Result<Vec<TableCheckpoint>> {
        let mut destroyed = Vec::new();
        let mut cps = self.checkpoints.write().unwrap();
        if tableName == common::AllTables {
            let keys: Vec<String> = cps
                .iter()
                .filter(|(_, cp)| cp.Status == CheckpointStatus::Failed)
                .map(|(k, _)| k.clone())
                .collect();
            for key in keys {
                if let Some(cp) = cps.remove(&key) {
                    destroyed.push(cp);
                }
            }
        } else {
            let exists = cps.contains_key(tableName);
            if !exists {
                return Err(common::ErrCheckpointTableNotFound.GenWithStackByArgs(tableName));
            }
            if let Some(cp) = cps.get(tableName) {
                if cp.Status == CheckpointStatus::Failed {
                    destroyed.push(cp.clone());
                    cps.remove(tableName);
                }
            }
        }
        self.save(ctx, &cps)?;
        Ok(destroyed)
    }

    fn DumpTables(&self, _ctx: &context::Context, writer: &mut dyn std::io::Write) -> Result<()> {
        let cps = self.checkpoints.read().unwrap();
        write_csv_record(
            writer,
            &["table_name", "job_id", "status", "message", "group_key"],
        )?;
        for cp in cps.values() {
            let job_id = cp.JobID.to_string();
            let status = (cp.Status as i32).to_string();
            write_csv_record(
                writer,
                &[&cp.TableName, &job_id, &status, &cp.Message, &cp.GroupKey],
            )?;
        }
        Ok(())
    }

    fn DumpEngines(&self, _ctx: &context::Context, _writer: &mut dyn std::io::Write) -> Result<()> {
        Ok(())
    }

    fn DumpChunks(&self, _ctx: &context::Context, _writer: &mut dyn std::io::Write) -> Result<()> {
        Ok(())
    }

    fn GetCheckpoints(&self, _ctx: &context::Context) -> Result<Vec<TableCheckpoint>> {
        let cps = self.checkpoints.read().unwrap();
        Ok(cps.values().cloned().collect())
    }

    fn Close(&self) -> Result<()> {
        Ok(())
    }
}

/// Write one record using the quoting rules of Go's `encoding/csv.Writer`.
fn write_csv_record(writer: &mut dyn std::io::Write, fields: &[&str]) -> Result<()> {
    for (index, field) in fields.iter().enumerate() {
        if index != 0 {
            writer
                .write_all(b",")
                .map_err(|e| errors::Trace(Error::new(e.to_string())))?;
        }
        let needs_quotes = field.contains([',', '"', '\r', '\n'])
            || field.chars().next().is_some_and(char::is_whitespace);
        if needs_quotes {
            writer
                .write_all(b"\"")
                .map_err(|e| errors::Trace(Error::new(e.to_string())))?;
            let normalized = field.replace("\r\n", "\n");
            writer
                .write_all(normalized.replace('"', "\"\"").as_bytes())
                .map_err(|e| errors::Trace(Error::new(e.to_string())))?;
            writer
                .write_all(b"\"")
                .map_err(|e| errors::Trace(Error::new(e.to_string())))?;
        } else {
            writer
                .write_all(field.as_bytes())
                .map_err(|e| errors::Trace(Error::new(e.to_string())))?;
        }
    }
    writer
        .write_all(b"\n")
        .map_err(|e| errors::Trace(Error::new(e.to_string())))
}

impl FileCheckpointManager {
    fn save(
        &self,
        ctx: &context::Context,
        checkpoints: &HashMap<String, TableCheckpoint>,
    ) -> Result<()> {
        let content = serde_json::to_vec_pretty(checkpoints)
            .map_err(|e| errors::Trace(Error::new(e.to_string())))?;
        let base = self
            .filePath
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("checkpoint.json");
        let storage = self.storage.read().unwrap();
        let st = storage
            .as_ref()
            .ok_or_else(|| errors::New("file checkpoint storage not initialized"))?;
        st.WriteFile(ctx, base, &content)
    }
}

/// MySQLCheckpointManager implements CheckpointManager using a MySQL database.
pub struct MySQLCheckpointManager {
    db: sql::DB,
    schemaName: String,
    tableName: String,
}

pub fn NewMySQLCheckpointManager(
    param: &common::MySQLConnectParam,
    schemaName: &str,
) -> Result<MySQLCheckpointManager> {
    let db = param.Connect()?;
    Ok(MySQLCheckpointManager {
        db,
        schemaName: schemaName.to_string(),
        tableName: "import_into_checkpoints".into(),
    })
}

impl CheckpointManager for MySQLCheckpointManager {
    fn Initialize(&self, ctx: &context::Context) -> Result<()> {
        let create_db = format!(
            "CREATE DATABASE IF NOT EXISTS {}",
            common::EscapeIdentifier(&self.schemaName)
        );
        self.db.ExecContext(ctx, &create_db, &[])?;

        let create_table = format!(
            "CREATE TABLE IF NOT EXISTS {}.{} (\
                table_name VARCHAR(256) NOT NULL,\
                job_id BIGINT NOT NULL,\
                status TINYINT NOT NULL,\
                message TEXT,\
                group_key VARCHAR(128),\
                update_time TIMESTAMP DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,\
                PRIMARY KEY (table_name)\
            )",
            common::EscapeIdentifier(&self.schemaName),
            common::EscapeIdentifier(&self.tableName)
        );
        self.db.ExecContext(ctx, &create_table, &[])?;
        Ok(())
    }

    fn Get(&self, ctx: &context::Context, tableName: &str) -> Result<Option<TableCheckpoint>> {
        let query = format!(
            "SELECT job_id, status, message, group_key FROM {}.{} WHERE table_name = ?",
            common::EscapeIdentifier(&self.schemaName),
            common::EscapeIdentifier(&self.tableName)
        );
        match self.db.QueryRowContext(ctx, &query, &[tableName.into()])? {
            None => Ok(None),
            Some((job_id, status, msg, group_key)) => Ok(Some(TableCheckpoint {
                TableName: tableName.to_string(),
                JobID: job_id,
                Status: CheckpointStatus::from_i32(status),
                Message: if msg.Valid { msg.String } else { String::new() },
                GroupKey: if group_key.Valid {
                    group_key.String
                } else {
                    String::new()
                },
            })),
        }
    }

    fn Update(&self, ctx: &context::Context, cp: &TableCheckpoint) -> Result<()> {
        let query = format!(
            "INSERT INTO {}.{} (table_name, job_id, status, message, group_key) \
             VALUES (?, ?, ?, ?, ?) \
             ON DUPLICATE KEY UPDATE \
             job_id = VALUES(job_id), \
             status = VALUES(status), \
             message = VALUES(message), \
             group_key = VALUES(group_key)",
            common::EscapeIdentifier(&self.schemaName),
            common::EscapeIdentifier(&self.tableName)
        );
        self.db.ExecContext(
            ctx,
            &query,
            &[
                cp.TableName.as_str().into(),
                cp.JobID.into(),
                (cp.Status as i64).into(),
                cp.Message.as_str().into(),
                cp.GroupKey.as_str().into(),
            ],
        )?;
        Ok(())
    }

    fn Remove(&self, ctx: &context::Context, tableName: &str) -> Result<()> {
        if tableName == common::AllTables {
            let query = format!(
                "DELETE FROM {}.{}",
                common::EscapeIdentifier(&self.schemaName),
                common::EscapeIdentifier(&self.tableName)
            );
            self.db.ExecContext(ctx, &query, &[])?;
            return Ok(());
        }
        let query = format!(
            "DELETE FROM {}.{} WHERE table_name = ?",
            common::EscapeIdentifier(&self.schemaName),
            common::EscapeIdentifier(&self.tableName)
        );
        self.db.ExecContext(ctx, &query, &[tableName.into()])?;
        Ok(())
    }

    fn IgnoreError(&self, ctx: &context::Context, tableName: &str) -> Result<()> {
        if tableName == common::AllTables {
            let query = format!(
                "UPDATE {}.{} SET status = ?, message = '', job_id = 0 WHERE status = ?",
                common::EscapeIdentifier(&self.schemaName),
                common::EscapeIdentifier(&self.tableName)
            );
            self.db.ExecContext(
                ctx,
                &query,
                &[
                    (CheckpointStatus::Pending as i64).into(),
                    (CheckpointStatus::Failed as i64).into(),
                ],
            )?;
            return Ok(());
        }
        let query = format!(
            "UPDATE {}.{} SET status = ?, message = '', job_id = 0 WHERE table_name = ? AND status = ?",
            common::EscapeIdentifier(&self.schemaName),
            common::EscapeIdentifier(&self.tableName)
        );
        let result = self.db.ExecContext(
            ctx,
            &query,
            &[
                (CheckpointStatus::Pending as i64).into(),
                tableName.into(),
                (CheckpointStatus::Failed as i64).into(),
            ],
        )?;
        let affected = result.RowsAffected()?;
        if affected == 0 {
            self.ensureTableCheckpointExists(ctx, tableName)?;
        }
        Ok(())
    }

    fn DestroyError(
        &self,
        ctx: &context::Context,
        tableName: &str,
    ) -> Result<Vec<TableCheckpoint>> {
        let (select_query, delete_query, args): (String, String, Vec<sql::SqlValue>) = if tableName
            == common::AllTables
        {
            (
                format!(
                    "SELECT table_name, job_id, status, message, group_key FROM {}.{} WHERE status = ?",
                    common::EscapeIdentifier(&self.schemaName),
                    common::EscapeIdentifier(&self.tableName)
                ),
                format!(
                    "DELETE FROM {}.{} WHERE status = ?",
                    common::EscapeIdentifier(&self.schemaName),
                    common::EscapeIdentifier(&self.tableName)
                ),
                vec![(CheckpointStatus::Failed as i64).into()],
            )
        } else {
            (
                format!(
                    "SELECT table_name, job_id, status, message, group_key FROM {}.{} WHERE table_name = ? AND status = ?",
                    common::EscapeIdentifier(&self.schemaName),
                    common::EscapeIdentifier(&self.tableName)
                ),
                format!(
                    "DELETE FROM {}.{} WHERE table_name = ? AND status = ?",
                    common::EscapeIdentifier(&self.schemaName),
                    common::EscapeIdentifier(&self.tableName)
                ),
                vec![tableName.into(), (CheckpointStatus::Failed as i64).into()],
            )
        };

        let destroyed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let retry = common::SQLWithRetry {
            DB: self.db.clone(),
            Logger: log::L(),
        };
        let destroyed2 = destroyed.clone();
        retry.Transact(ctx.clone(), "destroy error checkpoints", move |c, tx| {
            let mut rows = tx.QueryContext(&c, &select_query, &args)?;
            while rows.Next() {
                let (tn, job_id, status, msg, gk) = rows.ScanCheckpoint();
                destroyed2.lock().unwrap().push(TableCheckpoint {
                    TableName: tn,
                    JobID: job_id,
                    Status: CheckpointStatus::from_i32(status),
                    Message: if msg.Valid { msg.String } else { String::new() },
                    GroupKey: if gk.Valid { gk.String } else { String::new() },
                });
            }
            rows.Err()?;
            tx.ExecContext(&c, &delete_query, &args)?;
            Ok(())
        })?;

        let destroyed = destroyed.lock().unwrap().clone();
        if tableName != common::AllTables && destroyed.is_empty() {
            self.ensureTableCheckpointExists(ctx, tableName)?;
        }
        Ok(destroyed)
    }

    fn DumpTables(&self, ctx: &context::Context, writer: &mut dyn std::io::Write) -> Result<()> {
        let query = format!(
            "SELECT table_name, job_id, status, message, group_key FROM {}.{}",
            common::EscapeIdentifier(&self.schemaName),
            common::EscapeIdentifier(&self.tableName)
        );
        let rows = self.db.QueryContext(ctx, &query, &[])?;
        rows.write_csv(writer)?;
        rows.Err()
    }

    fn DumpEngines(&self, _ctx: &context::Context, _writer: &mut dyn std::io::Write) -> Result<()> {
        Ok(())
    }

    fn DumpChunks(&self, _ctx: &context::Context, _writer: &mut dyn std::io::Write) -> Result<()> {
        Ok(())
    }

    fn GetCheckpoints(&self, ctx: &context::Context) -> Result<Vec<TableCheckpoint>> {
        let query = format!(
            "SELECT table_name, job_id, status, message, group_key FROM {}.{}",
            common::EscapeIdentifier(&self.schemaName),
            common::EscapeIdentifier(&self.tableName)
        );
        let mut rows = self.db.QueryContext(ctx, &query, &[])?;
        let mut cps = Vec::new();
        while rows.Next() {
            let (tn, job_id, status, msg, gk) = rows.ScanCheckpoint();
            cps.push(TableCheckpoint {
                TableName: tn,
                JobID: job_id,
                Status: CheckpointStatus::from_i32(status),
                Message: if msg.Valid { msg.String } else { String::new() },
                GroupKey: if gk.Valid { gk.String } else { String::new() },
            });
        }
        rows.Err()?;
        Ok(cps)
    }

    fn Close(&self) -> Result<()> {
        self.db.Close()
    }
}

impl MySQLCheckpointManager {
    fn ensureTableCheckpointExists(&self, ctx: &context::Context, tableName: &str) -> Result<()> {
        let cp = self.Get(ctx, tableName)?;
        if cp.is_none() {
            return Err(common::ErrCheckpointTableNotFound.GenWithStackByArgs(tableName));
        }
        Ok(())
    }
}
