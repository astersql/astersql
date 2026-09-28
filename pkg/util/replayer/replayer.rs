// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// Plan Replayer 捕获文件：命名规则、目录与对象存储写入封装。
//
// Plan Replayer 将执行计划、统计信息等打包为 zip，供异地复现优化器结果。
// 文件名前缀（`replayer` / `capture_replayer` 等）会被 HTTP dump 接口识别，
// 故分支顺序与 Go 保持一致。

#![allow(non_snake_case, non_upper_case_globals)]

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE;
use std::sync::{Mutex, Once};
use std::time::{SystemTime, UNIX_EPOCH};

/// 本包统一错误类型（Send + Sync 的动态错误）。
pub type Error = Box<dyn std::error::Error + Send + Sync>;

/// Carries the context used by object-storage operations.
///
/// The migration keeps this type opaque until TiDB's shared Rust context type
/// is connected; importantly, the same value is forwarded to create, write,
/// and close just as it is in Go.
/// 对象存储操作上下文占位；同一实例会转发到 create/write/close。
#[derive(Debug, Default)]
pub struct Context;

/// Minimal object-storage surface used by the plan replayer.
/// Plan Replayer 所需的最小对象存储接口：按路径创建写入器。
pub trait Storage {
    fn create(
        &self,
        ctx: &Context,
        path: String,
        options: Option<()>,
    ) -> Result<Box<dyn ObjectWriter>, Error>;
}

/// Context-aware writer returned by object storage.
/// 带上下文的对象写入器（对齐 Go 存储层的 Write/Close）。
pub trait ObjectWriter {
    fn write(&mut self, ctx: &Context, data: &[u8]) -> Result<usize, Error>;
    fn close(&mut self, ctx: &Context) -> Result<(), Error>;
}

/// Rust equivalent of Go's `io.WriteCloser` used by callers of this package.
/// 调用方使用的 WriteCloser，内部绑定固定 Context。
pub trait WriteCloser {
    fn write(&mut self, data: &[u8]) -> Result<usize, Error>;
    fn close(&mut self) -> Result<(), Error>;
}

/// 对象存储中的相对目录名（固定为 `replayer`）。
const planReplayerDirName: &str = "replayer";

/// Identifies one plan replayer task.
/// 标识一次 Plan Replayer 任务（SQL digest + Plan digest）。
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct PlanReplayerTaskKey {
    pub SQLDigest: String,
    pub PlanDigest: String,
}

/// Generates the capture name and creates its writer below the relative
/// `replayer` object-storage directory.
/// 生成捕获文件名，并在相对目录 `replayer/` 下创建写入器。
pub fn GeneratePlanReplayerFile(
    ctx: Context,
    storage: &dyn Storage,
    isCapture: bool,
    isContinuesCapture: bool,
    enableHistoricalStatsForCapture: bool,
) -> Result<(Box<dyn WriteCloser>, String), Error> {
    let file_name = generatePlanReplayerFileName(
        isCapture,
        isContinuesCapture,
        enableHistoricalStatsForCapture,
    )?;
    let object_path = format!("{}/{file_name}", GetPlanReplayerDirName());
    let writer = storage.create(&ctx, object_path, None)?;
    Ok((NewFileWriter(ctx, writer), file_name))
}

/// Binds an object writer to the context used for all subsequent operations.
/// 将 ObjectWriter 与 Context 绑定为对外 WriteCloser。
pub fn NewFileWriter(ctx: Context, writer: Box<dyn ObjectWriter>) -> Box<dyn WriteCloser> {
    Box::new(FileWriter { ctx, writer })
}

/// 内部写入适配器：每次 write/close 都带上绑定的 Context。
struct FileWriter {
    ctx: Context,
    writer: Box<dyn ObjectWriter>,
}

impl WriteCloser for FileWriter {
    fn write(&mut self, data: &[u8]) -> Result<usize, Error> {
        self.writer.write(&self.ctx, data)
    }

    fn close(&mut self) -> Result<(), Error> {
        self.writer.close(&self.ctx)
    }
}

/// Generates a plan-replayer capture task name.
/// 对外导出的文件名生成入口。
pub fn GeneratePlanReplayerFileName(
    isCapture: bool,
    isContinuesCapture: bool,
    enableHistoricalStatsForCapture: bool,
) -> Result<String, Error> {
    generatePlanReplayerFileName(
        isCapture,
        isContinuesCapture,
        enableHistoricalStatsForCapture,
    )
}

/// 按捕获模式选择前缀，并拼接随机 key 与纳秒时间戳。
fn generatePlanReplayerFileName(
    isCapture: bool,
    isContinuesCapture: bool,
    enableHistoricalStatsForCapture: bool,
) -> Result<String, Error> {
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos() as i64;

    let mut random_bytes = [0_u8; 16];
    getrandom::fill(&mut random_bytes).map_err(|error| {
        std::io::Error::other(format!("failed to read secure random bytes: {error}"))
    })?;
    let key = URL_SAFE.encode(random_bytes);

    // `capture_replayer` has special meaning to the /plan_replayer/dump/
    // HTTP handler, so preserve the Go branch order and exact prefixes.
    // 前缀分支顺序必须与 Go 一致，否则 dump HTTP 接口无法识别捕获文件。
    let prefix = if isContinuesCapture || isCapture && enableHistoricalStatsForCapture {
        "capture_replayer"
    } else if isCapture && !enableHistoricalStatsForCapture {
        "capture_normal_replayer"
    } else {
        "replayer"
    };
    Ok(format!("{prefix}_{key}_{timestamp}.zip"))
}

/// Plan replayer directory path, initially the Go string zero value.
/// 本地/外部存储中的 Plan Replayer 路径，初值为空串。
pub static PlanReplayerPath: Mutex<String> = Mutex::new(String::new());

/// Ensures `PlanReplayerPath` is initialized once by its eventual owner.
/// 保证 `PlanReplayerPath` 只由所有者初始化一次。
pub static PlanReplayerPathOnce: Once = Once::new();

/// Returns the relative directory used in external storage.
/// 返回对象存储中使用的相对目录名。
pub fn GetPlanReplayerDirName() -> &'static str {
    planReplayerDirName
}
