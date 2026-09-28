// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// Plan Replayer：导出/加载执行计划相关现场，便于复现优化器行为。
//
// DUMP 将 SQL、统计信息、绑定等打包；LOAD 在目标环境恢复 schema/统计/变量；
// Capture 按 SQL digest / plan digest 注册或移除捕获任务。
// 执行计划（execution plan）描述优化器选定的算子树；digest 为规范化摘要。

#![allow(non_snake_case)]

use std::collections::HashSet;

/// 会话变量键：Plan Replayer DUMP 相关状态。
pub const PLAN_REPLAYER_DUMP_VAR_KEY: &str = "plan_replayer_dump_var";
/// 会话变量键：Plan Replayer LOAD 相关状态。
pub const PLAN_REPLAYER_LOAD_VAR_KEY: &str = "plan_replayer_load_var";

#[derive(Clone, Debug, Eq, PartialEq)]
/// 捕获任务描述：SQL/Plan digest，以及是否为移除操作。
pub struct PlanReplayerCaptureInfo {
    pub sql_digest: String,
    pub plan_digest: String,
    pub remove: bool,
}

/// DUMP 任务状态：语句列表、是否 ANALYZE、时间戳与输出文件信息。
pub struct PlanReplayerDumpInfo<S, F> {
    pub statements: Vec<S>,
    pub analyze: bool,
    pub historical_stats_timestamp: u64,
    pub start_timestamp: u64,
    pub path: String,
    pub file: Option<F>,
    pub file_name: String,
}

/// LOAD 任务：待加载归档文件路径。
pub struct PlanReplayerLoadInfo {
    pub path: String,
}

/// Plan Replayer 后端边界：捕获、dump/load 文件、归档解析与 schema/统计恢复。
pub trait PlanReplayerBackend: Sized {
    type Context;
    type Request;
    type Statement;
    type File;
    type Archive;
    type Error;

    fn grow_and_reset(&self, request: &mut Self::Request);
    fn append_string(&self, request: &mut Self::Request, column: usize, value: &str);
    fn presigned_url_expiration(&self) -> String;
    fn remove_capture_task(
        &mut self,
        context: &mut Self::Context,
        capture: &PlanReplayerCaptureInfo,
    ) -> Result<(), Self::Error>;
    fn register_capture_task(
        &mut self,
        context: &mut Self::Context,
        capture: &PlanReplayerCaptureInfo,
    ) -> Result<(), Self::Error>;
    fn create_dump_file(
        &mut self,
        context: &mut Self::Context,
    ) -> Result<(Self::File, String), Self::Error>;
    fn close_dump_file(&mut self, file: Self::File);
    fn statement_read_timestamp(&mut self, context: &mut Self::Context)
    -> Result<u64, Self::Error>;
    fn prepare_dump_file_transfer(
        &mut self,
        dump: &PlanReplayerDumpInfo<Self::Statement, Self::File>,
    ) -> Result<(), Self::Error>;
    fn dump(
        &mut self,
        context: &mut Self::Context,
        dump: &mut PlanReplayerDumpInfo<Self::Statement, Self::File>,
    ) -> Result<String, Self::Error>;
    fn empty_sql_error(&self) -> Self::Error;
    fn parse_sql(
        &mut self,
        context: &mut Self::Context,
        sql: &str,
    ) -> Result<Self::Statement, Self::Error>;
    fn prepare_load_file_transfer(
        &mut self,
        load: &PlanReplayerLoadInfo,
    ) -> Result<(), Self::Error>;
    fn empty_path_error(&self) -> Self::Error;
    fn read_file(
        &mut self,
        context: &mut Self::Context,
        path: &str,
    ) -> Result<Vec<u8>, Self::Error>;
    fn open_archive(&mut self, data: &[u8]) -> Result<Self::Archive, Self::Error>;
    fn target_sql(&mut self, archive: &mut Self::Archive) -> Result<String, Self::Error>;
    fn load_variables(
        &mut self,
        context: &mut Self::Context,
        archive: &mut Self::Archive,
    ) -> Result<(), Self::Error>;
    fn disable_auto_analyze(&mut self, context: &mut Self::Context) -> Result<(), Self::Error>;
    fn create_tables(
        &mut self,
        context: &mut Self::Context,
        archive: &mut Self::Archive,
    ) -> Result<HashSet<String>, Self::Error>;
    fn load_tiflash_replicas(
        &mut self,
        context: &mut Self::Context,
        archive: &mut Self::Archive,
    ) -> Result<(), Self::Error>;
    fn create_views(
        &mut self,
        context: &mut Self::Context,
        archive: &mut Self::Archive,
    ) -> Result<(), Self::Error>;
    fn load_statistics(
        &mut self,
        context: &mut Self::Context,
        archive: &mut Self::Archive,
    ) -> Result<(), Self::Error>;
    fn load_bindings(
        &mut self,
        context: &mut Self::Context,
        archive: &mut Self::Archive,
        databases: &HashSet<String>,
    ) -> Result<(), Self::Error>;
    fn append_binding_warning(&mut self, error: &Self::Error);
    fn append_auto_analyze_warning(&mut self);
    fn load_stats_bytes(
        &mut self,
        context: &mut Self::Context,
        data: &[u8],
    ) -> Result<(), Self::Error>;
}

/// Plan Replayer DUMP/Capture 执行器；`end` 标记是否已完成单次产出。
pub struct PlanReplayerExec<B: PlanReplayerBackend> {
    pub backend: B,
    pub capture_info: Option<PlanReplayerCaptureInfo>,
    pub dump_info: Option<PlanReplayerDumpInfo<B::Statement, B::File>>,
    pub end: bool,
}

impl<B: PlanReplayerBackend> PlanReplayerExec<B> {
    /// 一次 Next：优先处理 Capture；否则创建 dump 文件并导出，结果写入 request。
    pub fn Next(
        &mut self,
        context: &mut B::Context,
        request: &mut B::Request,
    ) -> Result<(), B::Error> {
        self.backend.grow_and_reset(request);
        if self.end {
            return Ok(());
        }
        // Capture 路径：按 remove 标志注销或注册捕获任务。
        if let Some(capture) = &self.capture_info {
            let result = if capture.remove {
                self.backend.remove_capture_task(context, capture)
            } else {
                self.backend.register_capture_task(context, capture)
            };
            if result.is_ok() {
                self.end = true;
            }
            return result;
        }

        // DUMP 路径：创建文件，若指定外部 path 则只做传输准备。
        let dump = self.dump_info.as_mut().expect("dump info is required");
        let (file, name) = self.backend.create_dump_file(context)?;
        dump.file = Some(file);
        dump.file_name = name;
        dump.start_timestamp = match self.backend.statement_read_timestamp(context) {
            Ok(timestamp) => timestamp,
            Err(error) => {
                if let Some(file) = dump.file.take() {
                    self.backend.close_dump_file(file);
                }
                return Err(error);
            }
        };
        if !dump.path.is_empty() {
            if let Err(error) = self.backend.prepare_dump_file_transfer(dump) {
                if let Some(file) = dump.file.take() {
                    self.backend.close_dump_file(file);
                }
                return Err(error);
            }
            self.end = true;
            return Ok(());
        }
        // 无 SQL 可导出时直接报错。
        if dump.statements.is_empty() {
            let error = self.backend.empty_sql_error();
            if let Some(file) = dump.file.take() {
                self.backend.close_dump_file(file);
            }
            return Err(error);
        }
        let token = match self.backend.dump(context, dump) {
            Ok(token) => token,
            Err(error) => {
                if let Some(file) = dump.file.take() {
                    self.backend.close_dump_file(file);
                }
                return Err(error);
            }
        };
        appendPlanReplayerDumpResult(&self.backend, request, &token);
        self.end = true;
        Ok(())
    }

    /// 移除当前 capture_info 对应的捕获任务。
    pub fn removeCaptureTask(&mut self, context: &mut B::Context) -> Result<(), B::Error> {
        self.backend
            .remove_capture_task(context, self.capture_info.as_ref().expect("capture info"))?;
        self.end = true;
        Ok(())
    }

    /// 注册当前 capture_info 对应的捕获任务。
    pub fn registerCaptureTask(&mut self, context: &mut B::Context) -> Result<(), B::Error> {
        self.backend
            .register_capture_task(context, self.capture_info.as_ref().expect("capture info"))?;
        self.end = true;
        Ok(())
    }

    /// 为 dump_info 创建输出文件并记录文件名。
    pub fn createFile(&mut self, context: &mut B::Context) -> Result<(), B::Error> {
        let (file, name) = self.backend.create_dump_file(context)?;
        let dump = self.dump_info.as_mut().expect("dump info");
        dump.file = Some(file);
        dump.file_name = name;
        Ok(())
    }

    /// 准备将 dump 文件传输到外部 path。
    pub fn prepare(&mut self) -> Result<(), B::Error> {
        self.backend
            .prepare_dump_file_transfer(self.dump_info.as_ref().expect("dump info"))
    }

    /// 从字节流按分号拆分 SQL，解析后执行 dump，返回 token。
    pub fn DumpSQLsFromFile(
        &mut self,
        context: &mut B::Context,
        bytes: &[u8],
    ) -> Result<String, B::Error> {
        let dump = self.dump_info.as_mut().expect("dump info");
        dump.statements.clear();
        for sql in String::from_utf8_lossy(bytes).split(';') {
            // Match Go's strings.Trim(sql, "\n"): spaces are significant input
            // to the SQL parser and must not be silently removed here.
            let sql = sql.trim_matches('\n');
            if !sql.is_empty() {
                dump.statements.push(self.backend.parse_sql(context, sql)?);
            }
        }
        self.backend.dump(context, dump)
    }
}

/// 将 dump 结果（预签名 URL 或文件 token）写入结果集，附带过期说明与 curl 示例。
pub fn appendPlanReplayerDumpResult<B: PlanReplayerBackend>(
    backend: &B,
    request: &mut B::Request,
    token: &str,
) {
    // URL 形态：多行说明 Download URL / Expires / Browser / curl / Note。
    if isPlanReplayerDownloadURL(token) {
        let rows = [
            ("Download URL", token.to_owned()),
            ("Expires in", backend.presigned_url_expiration()),
            (
                "Browser",
                "Open the Download URL directly before it expires".to_owned(),
            ),
            ("curl", format!("curl -L '{token}' -o plan_replayer.zip")),
            (
                "Note",
                "If the URL expires, rerun PLAN REPLAYER DUMP to get a new one".to_owned(),
            ),
        ];
        for (item, value) in rows {
            backend.append_string(request, 0, item);
            backend.append_string(request, 1, &value);
        }
    } else {
        backend.append_string(request, 0, "File token");
        backend.append_string(request, 1, token);
    }
}

/// 判断 token 是否为 http(s) 下载 URL（非空 host、非仅路径）。
pub fn isPlanReplayerDownloadURL(token: &str) -> bool {
    let Some(remainder) = token
        .strip_prefix("https://")
        .or_else(|| token.strip_prefix("http://"))
    else {
        return false;
    };
    // Go `url.Parse` exposes the authority as `URL.Host`; query/fragment/path
    // text after an empty authority must not turn a local token into a URL.
    let authority = remainder.split(['/', '?', '#']).next().unwrap_or_default();
    !authority.is_empty()
}

/// Plan Replayer LOAD 执行器：校验 path 后准备文件传输。
pub struct PlanReplayerLoadExec<B: PlanReplayerBackend> {
    pub backend: B,
    pub info: PlanReplayerLoadInfo,
}

impl<B: PlanReplayerBackend> PlanReplayerLoadExec<B> {
    /// path 为空报错；否则准备 LOAD 文件传输。
    pub fn Next(
        &mut self,
        _context: &mut B::Context,
        request: &mut B::Request,
    ) -> Result<(), B::Error> {
        self.backend.grow_and_reset(request);
        if self.info.path.is_empty() {
            return Err(self.backend.empty_path_error());
        }
        self.backend.prepare_load_file_transfer(&self.info)
    }
}

/// 处理统计信息字节：空数据为 no-op，否则交给 backend 加载。
pub fn handleLoadStats<B: PlanReplayerBackend>(
    backend: &mut B,
    context: &mut B::Context,
    data: &[u8],
) -> Result<(), B::Error> {
    if data.is_empty() {
        Ok(())
    } else {
        backend.load_stats_bytes(context, data)
    }
}

/// 处理 Plan Replayer 归档字节：空为 no-op，否则走完整 `updateLoadInfo`。
pub fn handlePlanReplayerLoad<B: PlanReplayerBackend>(
    backend: &mut B,
    context: &mut B::Context,
    data: &[u8],
) -> Result<(), B::Error> {
    if data.is_empty() {
        Ok(())
    } else {
        updateLoadInfo(backend, context, data)
    }
}

/// 为 EXPLAIN EXPLORE 从 path 读取归档，提取目标 SQL 并执行加载。
pub fn loadPlanReplayerForExplainExplore<B: PlanReplayerBackend>(
    backend: &mut B,
    context: &mut B::Context,
    path: &str,
) -> Result<String, B::Error> {
    if path.trim().is_empty() {
        return Err(backend.empty_path_error());
    }
    let data = backend.read_file(context, path)?;
    let target = extractPlanReplayerTargetSQL(backend, &data)?;
    updateLoadInfo(backend, context, &data)?;
    Ok(target)
}

/// 打开归档并取出其中记录的目标 SQL 文本。
pub fn extractPlanReplayerTargetSQL<B: PlanReplayerBackend>(
    backend: &mut B,
    data: &[u8],
) -> Result<String, B::Error> {
    let mut archive = backend.open_archive(data)?;
    backend.target_sql(&mut archive)
}

/// 从归档恢复 TiFlash 副本设置。
pub fn loadSetTiFlashReplica<B: PlanReplayerBackend>(
    backend: &mut B,
    context: &mut B::Context,
    archive: &mut B::Archive,
) -> Result<(), B::Error> {
    backend.load_tiflash_replicas(context, archive)
}

/// 识别“集群无 TiFlash 节点”类错误消息。
pub fn isNoTiFlashStoreErr(message: &str) -> bool {
    message.contains("total tiflash server count: 0")
}

/// 加载归档中的全部 SQL Binding（执行计划绑定）。
pub fn loadAllBindings<B: PlanReplayerBackend>(
    backend: &mut B,
    context: &mut B::Context,
    archive: &mut B::Archive,
    databases: &HashSet<String>,
) -> Result<(), B::Error> {
    backend.load_bindings(context, archive, databases)
}

/// 加载 Binding；`_session` 保留与 Go 签名对齐，当前统一走 backend。
pub fn loadBindings<B: PlanReplayerBackend>(
    backend: &mut B,
    context: &mut B::Context,
    archive: &mut B::Archive,
    databases: &HashSet<String>,
    _session: bool,
) -> Result<(), B::Error> {
    backend.load_bindings(context, archive, databases)
}

/// 从归档恢复会话/系统变量。
pub fn loadVariables<B: PlanReplayerBackend>(
    backend: &mut B,
    context: &mut B::Context,
    archive: &mut B::Archive,
) -> Result<(), B::Error> {
    backend.load_variables(context, archive)
}

/// LOAD 期间禁用自动 ANALYZE，避免统计被后台任务改写。
pub fn disableAutoAnalyzeForPlanReplayerLoad<B: PlanReplayerBackend>(
    backend: &mut B,
    context: &mut B::Context,
) -> Result<(), B::Error> {
    backend.disable_auto_analyze(context)
}

/// 在已建表基础上创建视图等 schema 对象。
pub fn createSchemaAndItems<B: PlanReplayerBackend>(
    backend: &mut B,
    context: &mut B::Context,
    archive: &mut B::Archive,
) -> Result<(), B::Error> {
    backend.create_views(context, archive)
}

/// 从归档加载表统计信息。
pub fn loadStats<B: PlanReplayerBackend>(
    backend: &mut B,
    context: &mut B::Context,
    archive: &mut B::Archive,
) -> Result<(), B::Error> {
    backend.load_statistics(context, archive)
}

/// LOAD 主流程：变量 → 关自动分析 → 建表 → TiFlash → 视图 → 统计 → Binding。
pub fn updateLoadInfo<B: PlanReplayerBackend>(
    backend: &mut B,
    context: &mut B::Context,
    data: &[u8],
) -> Result<(), B::Error> {
    // 按依赖顺序恢复现场；Binding 失败只记 warning，不中断整体 LOAD。
    let mut archive = backend.open_archive(data)?;
    backend.load_variables(context, &mut archive)?;
    backend.disable_auto_analyze(context)?;
    let databases = backend.create_tables(context, &mut archive)?;
    backend.load_tiflash_replicas(context, &mut archive)?;
    backend.create_views(context, &mut archive)?;
    backend.load_statistics(context, &mut archive)?;
    if let Err(error) = backend.load_bindings(context, &mut archive, &databases) {
        backend.append_binding_warning(&error);
    }
    backend.append_auto_analyze_warning();
    Ok(())
}

/// 从归档创建表，返回涉及的数据库名集合。
pub fn createTable<B: PlanReplayerBackend>(
    backend: &mut B,
    context: &mut B::Context,
    archive: &mut B::Archive,
) -> Result<HashSet<String>, B::Error> {
    backend.create_tables(context, archive)
}
