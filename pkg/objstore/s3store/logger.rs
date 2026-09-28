// Copyright 2026 PingCAP, Inc.
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

// AWS SDK / Smithy 日志桥接到仓库 `tracing` 门面。
//
// 将 SDK 侧的 Warn/Debug/Info 分级消息统一写入 `aws_smithy` target，
// 便于对象存储客户端调试与运维观测。

/// SDK 日志级别分类，对应 AWS logging Classification。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Classification {
    Warn,
    Debug,
    #[default]
    Info,
}

/// PingCAP 风格的 AWS SDK 日志接收器（无状态）。
#[derive(Clone, Debug, Default)]
pub struct PingcapLogger;

/// 构造默认的 `PingcapLogger` 实例。
pub fn newLogger() -> PingcapLogger {
    PingcapLogger
}

impl PingcapLogger {
    /// 按分级将消息转发到 tracing 的 warn/debug/info。
    pub fn Logf(&self, classification: Classification, message: &str) {
        match classification {
            Classification::Warn => tracing::warn!(target: "aws_smithy", "{message}"),
            Classification::Debug => tracing::debug!(target: "aws_smithy", "{message}"),
            Classification::Info => tracing::info!(target: "aws_smithy", "{message}"),
        }
    }
}
