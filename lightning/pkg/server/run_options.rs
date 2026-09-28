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

//! RunOnceWithOptions option bag — ported from `run_options.go`.
//!
//!
//! 这一层复刻 Go `server.Option` 的闭包注入模式，避免 `RunOnceWithOptions` 的参数表继续膨胀。
//! 所有字段都服务于“单次任务”这条调用链，而不是进程级全局状态。
//! 外部存储、checkpoint 存储与指标注册器都允许调用方按需覆盖默认实现。
//! 这样 HTTP server 模式和测试代码都能复用同一套任务启动入口。
//! `options` 结构体本身不做校验，真正的约束由 `run()` 和后续初始化流程消费。
//! 这与 Go 版本一致：Option 只是延迟赋值，不在这里提前触发副作用。
//! 日志器被包装成 Lightning 自己的 `log::Logger`，从而保持统一的字段记录格式。
//! `dupIndicator` 只是把去重检测的布尔开关透传给下游 controller。
//! `db` 字段刻意保留为测试注入槽位，避免正常路径误把伪造数据库当作生产依赖。
//! 每个 `With*` 函数都只写入对应槽位，保持闭包副作用可预测。
//! 调用方可以通过组合多个闭包精确覆盖默认值，而不必复制整份配置。
//! 因此阅读这些 helper 时，重点不是语法，而是它们如何为 server 层保留可注入边界。
//! 当 Rust 与 Go 行为对比时，最需要保持一致的是字段默认值与覆盖优先级。
//! 这也是为什么本文件只承载参数拼装，不直接参与任务执行或资源释放。

use crate::atomic;
use crate::log;
use crate::promutil;
use crate::sql;
use crate::storeapi;
use crate::zap;

/// options holds optional injections for a single Lightning task.
pub struct options {
    pub dumpFileStorage: Option<storeapi::StorageRef>,
    pub checkpointStorage: Option<storeapi::StorageRef>,
    pub checkpointName: String,
    pub promFactory: Option<promutil::Factory>,
    pub promRegistry: Option<promutil::Registry>,
    pub logger: log::Logger,
    pub dupIndicator: Option<atomic::Bool>,
    /// only used in tests
    pub db: Option<sql::DB>,
}

impl Default for options {
    fn default() -> Self {
        Self {
            dumpFileStorage: None,
            checkpointStorage: None,
            checkpointName: String::new(),
            promFactory: None,
            promRegistry: None,
            logger: log::L(),
            dupIndicator: None,
            db: None,
        }
    }
}

/// RunOption configures a lightning task (Go `server.Option`).
pub type RunOption = Box<dyn FnOnce(&mut options) + Send>;

/// WithDumpFileStorage sets the external storage to a lightning task.
pub fn WithDumpFileStorage(s: storeapi::StorageRef) -> RunOption {
    Box::new(move |o: &mut options| {
        o.dumpFileStorage = Some(s);
    })
}

/// WithCheckpointStorage sets the checkpoint name in external storage.
pub fn WithCheckpointStorage(s: storeapi::StorageRef, cpName: String) -> RunOption {
    Box::new(move |o: &mut options| {
        o.checkpointStorage = Some(s);
        o.checkpointName = cpName;
    })
}

/// WithPromFactory sets the prometheus factory to a lightning task.
pub fn WithPromFactory(f: promutil::Factory) -> RunOption {
    Box::new(move |o: &mut options| {
        o.promFactory = Some(f);
    })
}

/// WithPromRegistry sets the prometheus registry to a lightning task.
pub fn WithPromRegistry(r: promutil::Registry) -> RunOption {
    Box::new(move |o: &mut options| {
        o.promRegistry = Some(r);
    })
}

/// WithLogger sets the logger to a lightning task.
pub fn WithLogger(logger: zap::Logger) -> RunOption {
    Box::new(move |o: &mut options| {
        o.logger = log::Logger {
            Logger: logger,
            fields: Vec::new(),
            entries: Default::default(),
        };
    })
}

/// WithDupIndicator sets a bool indicator for duplicate detection.
pub fn WithDupIndicator(b: atomic::Bool) -> RunOption {
    Box::new(move |o: &mut options| {
        o.dupIndicator = Some(b);
    })
}

/// WithDB injects a DB (test helper, mirrors Go unexported field usage).
pub fn WithDB(db: sql::DB) -> RunOption {
    Box::new(move |o: &mut options| {
        o.db = Some(db);
    })
}
