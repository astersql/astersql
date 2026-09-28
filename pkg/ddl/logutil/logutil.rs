// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// DDL 专用 Logger 工厂。
//
// 基于后台日志器（`BgLogger`）附加固定的 category 字段，区分普通 DDL、
// 升级过程与快速 DDL（ingest/回填）场景；另提供采样 Logger，
// 避免高频路径刷爆日志。

#![allow(non_snake_case)]

use crate::util::logutil::log::{
    BgLogger, LogField, LogFieldCategory, Logger, sample_logger_factory,
};
use std::sync::OnceLock;
use std::time::Duration;

/// 普通 DDL 日志类别。
const DDL_CATEGORY: &str = "ddl";
/// 集群升级期间的 DDL 日志类别。
const DDL_UPGRADING_CATEGORY: &str = "ddl-upgrading";
/// 快速 DDL / ingest（索引回填写入）日志类别。
const DDL_INGEST_CATEGORY: &str = "ddl-ingest";

/// 在后台 Logger 上附加指定 category 字段。
fn logger_with_category(category: &str) -> Logger {
    BgLogger().with_fields([LogField::String(LogFieldCategory.into(), category.into())])
}

// DDLLogger with category "ddl" is used to log DDL related messages. Do not use
// it to log the message that is not related to DDL.
/// 返回 category 为 `ddl` 的 Logger，仅用于 DDL 相关消息。
pub fn DDLLogger() -> Logger {
    logger_with_category(DDL_CATEGORY)
}

// DDLUpgradingLogger with category "ddl-upgrading" is used to log DDL related
// messages during the upgrading process. Do not use it to log the message that
// is not related to DDL.
/// 返回升级过程专用 Logger，category 为 `ddl-upgrading`。
pub fn DDLUpgradingLogger() -> Logger {
    logger_with_category(DDL_UPGRADING_CATEGORY)
}

// DDLIngestLogger with category "ddl-ingest" is used to log DDL related messages
// during the fast DDL process. Do not use it to log the message that is not
// related to DDL.
/// 返回快速 DDL / ingest 专用 Logger，category 为 `ddl-ingest`。
pub fn DDLIngestLogger() -> Logger {
    logger_with_category(DDL_INGEST_CATEGORY)
}

// SampleLogger returns a logger that samples logs to avoid too many logs.
/// 返回全局单例的采样 Logger：每 60 秒最多保留 3 条同类消息，category 仍为 `ddl`。
pub fn SampleLogger() -> Logger {
    static SAMPLE_LOGGER: OnceLock<Logger> = OnceLock::new();
    SAMPLE_LOGGER
        .get_or_init(|| {
            // 采样窗口 60s、容量 3，字段固定带上 DDL_CATEGORY。
            let factory = sample_logger_factory(
                BgLogger(),
                Duration::from_secs(60),
                3,
                vec![LogField::String(
                    LogFieldCategory.into(),
                    DDL_CATEGORY.into(),
                )],
            );
            factory()
        })
        .clone()
}
