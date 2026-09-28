// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// LOAD STATS 执行器：从 JSON 文件加载表统计信息。
//
// 统计信息（Statistics）供优化器估算代价；本执行器对应 `LOAD STATS` 语句，
// 先登记加载选项（路径），再由会话侧读入 JSON 并调用 domain 的 StatsHandle
// 写入。空表名且 version 为 0 视为 JSON `null` 兼容路径，直接成功返回。

#![allow(non_snake_case)]

use std::any::Any;
use std::sync::Arc;

use astersql_errors as errors;
use astersql_util_chunk as chunk;

/// LOAD STATS 操作的 Result 别名。
pub type LoadStatsResult<T = ()> = Result<T, errors::SharedError>;

/// 执行上下文占位：具体会话/域对象由上层以 `Any` 注入。
#[derive(Clone)]
pub struct LoadStatsContext(pub Arc<dyn Any + Send + Sync>);

impl Default for LoadStatsContext {
    fn default() -> Self {
        Self(Arc::new(()))
    }
}

/// 待加载统计文件的路径选项，安装到会话后供后续 Update 使用。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadStatsOption {
    pub path: String,
}

/// Decoded statistics remain owned by the domain adapter. The executor keeps
/// the two fields needed for Go's JSON `null` compatibility check.
///
/// 已解码的统计载荷：表名与版本用于 null 兼容判断，`value` 由 domain 适配器持有。
pub struct DecodedStats {
    pub table_name: String,
    pub version: u64,
    pub value: Box<dyn Any + Send>,
}

/// Mandatory boundary for session value storage, JSON decoding, domain stats
/// handle lookup, InfoSchema selection, and LoadStatsFromJSON.
///
/// 会话/域运行时边界：chunk 大小、选项安装、JSON 解码、StatsHandle 与落库。
pub trait LoadStatsRuntime {
    fn MaxChunkSize(&self) -> usize;
    fn HasLoadStatsOption(&self) -> bool;
    fn ClearLoadStatsOption(&mut self);
    fn InstallLoadStatsOption(&mut self, option: LoadStatsOption);
    fn DecodeStatsJSON(&mut self, data: &[u8]) -> LoadStatsResult<DecodedStats>;
    fn HasStatsHandle(&self) -> bool;
    fn LoadStatsFromJSON(&mut self, stats: DecodedStats) -> LoadStatsResult;
}

/// LOAD STATS 物理算子：Open/Next/Close 生命周期。
pub struct LoadStatsExec {
    pub info: LoadStatsInfo,
}

/// 加载路径与运行时依赖的聚合。
pub struct LoadStatsInfo {
    pub path: String,
    pub runtime: Box<dyn LoadStatsRuntime>,
}

/// 会话变量键类型（对应 Go 侧 load_stats_var）。
pub type LoadStatsVarKeyType = i32;

/// 返回会话变量键的稳定字符串名。
pub fn load_stats_var_key_type_string(_: LoadStatsVarKeyType) -> &'static str {
    "load_stats_var"
}

/// 默认的 LOAD STATS 会话变量键。
pub const LOAD_STATS_VAR_KEY: LoadStatsVarKeyType = 0;

impl LoadStatsExec {
    /// 推进算子：校验路径、清理残留选项并安装新的 LoadStatsOption。
    pub fn Next(&mut self, _ctx: LoadStatsContext, req: &mut chunk::Chunk) -> LoadStatsResult {
        req.GrowAndReset(self.info.runtime.MaxChunkSize());
        if self.info.path.is_empty() {
            return Err(errors::New("Load Stats: file path is empty"));
        }

        // 上一轮选项未正常关闭时拒绝继续，避免状态泄漏
        if self.info.runtime.HasLoadStatsOption() {
            self.info.runtime.ClearLoadStatsOption();
            return Err(errors::New(
                "Load Stats: previous load stats option isn't closed normally",
            ));
        }
        self.info.runtime.InstallLoadStatsOption(LoadStatsOption {
            path: self.info.path.clone(),
        });
        Ok(())
    }

    /// 关闭算子（当前无额外资源）。
    pub fn Close(&mut self) -> LoadStatsResult {
        Ok(())
    }

    /// 打开算子（当前无额外初始化）。
    pub fn Open(&mut self, _ctx: LoadStatsContext) -> LoadStatsResult {
        Ok(())
    }
}

impl LoadStatsInfo {
    /// 用读入的 JSON 字节更新统计：解码后交给 StatsHandle 落库。
    pub fn Update(&mut self, data: &[u8]) -> LoadStatsResult {
        let decoded = self.runtime.DecodeStatsJSON(data)?;
        // 空表名且 version==0：兼容 JSON null，视为无操作成功
        if decoded.table_name.is_empty() && decoded.version == 0 {
            return Ok(());
        }
        if !self.runtime.HasStatsHandle() {
            return Err(errors::New("Load Stats: handle is nil"));
        }
        self.runtime.LoadStatsFromJSON(decoded)
    }
}
