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

// 系统进程（sys process）跟踪契约。
//
// 对应 Go `sysproctrack`：注册/注销内部系统会话（如 DDL、统计信息后台任务），
// 供 `SHOW PROCESSLIST` 与 kill 管理。实现方自行加锁，语义对齐 Go 的 `sync.RWMutex`。

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use sessmgr::ProcessInfo;
use variable::session::SessionVars;

/// Rust counterpart of Go's `error` result from `Tracker.Track`.
/// Tracker.Track 返回的错误类型，对应 Go 的 `error`。
pub type TrackError = anyhow::Error;

/// An owned Go-interface value. Cloning this handle keeps the same process
/// identity, which lets tracker implementations apply Go's duplicate-ID rule.
/// 可克隆的进程句柄；克隆后仍指向同一进程身份，
/// 以便实现方按 Go 规则拒绝重复 ID 注册。
pub type TrackProcRef = Arc<dyn TrackProc>;

/// A process that can be registered with the system-process tracker.
///
/// The session variables remain shared and mutable just like Go's
/// `*variable.SessionVars`. `ShowProcess` preserves both the nullable pointer
/// and the shared snapshot identity of Go's `*sessmgr.ProcessInfo`.
/// 可向系统进程跟踪器注册的会话进程。
///
/// `GetSessionVars` 返回共享可变的会话变量（对齐 Go `*variable.SessionVars`）；
/// `ShowProcess` 返回可空的进程快照（对齐 Go `*sessmgr.ProcessInfo`）。
pub trait TrackProc: Send + Sync {
    /// 取得该进程绑定的会话变量（SessionVars，会话级配置与状态）。
    fn GetSessionVars(&self) -> &Mutex<SessionVars>;
    /// 构造用于展示的进程信息快照；无进程时返回 None。
    fn ShowProcess(&self) -> Option<Arc<ProcessInfo>>;
}

/// Concurrent system-process tracking contract.
///
/// Implementations own their synchronization, matching the Go implementation
/// whose methods all operate through an internal `sync.RWMutex`.
/// 并发安全的系统进程跟踪器接口。
///
/// `Track`/`UnTrack` 管理注册表；`GetSysProcessList` 列出当前系统进程；
/// `KillSysProcess` 按 ID 终止。
pub trait Tracker: Send + Sync {
    /// 以给定 ID 注册进程；ID 冲突时返回错误。
    fn Track(&self, id: u64, proc: TrackProcRef) -> Result<(), TrackError>;
    /// 注销指定 ID 的进程。
    fn UnTrack(&self, id: u64);
    /// 返回当前系统进程 ID 到 ProcessInfo 的映射。
    fn GetSysProcessList(&self) -> HashMap<u64, Arc<ProcessInfo>>;
    /// 请求终止指定 ID 的系统进程。
    fn KillSysProcess(&self, id: u64);
}
