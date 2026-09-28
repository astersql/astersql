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

// `mysql.tidb` 内部变量键名定义。
//
// 这些键由集群控制器与 DXF（分布式执行框架）读写，用于协调后台任务与资源伸缩。

/// The `mysql.tidb` key used to pause scaling in DXF worker resources.
///
/// The cluster controller uses this flag to avoid conflicts between scaling in
/// and scheduling.
///
/// `mysql.tidb` 中用于暂停 DXF worker 缩容（scale-in）的键。
/// 集群控制器置位后可避免缩容与任务调度同时进行产生冲突。
pub const DXF_SCHEDULE_PAUSE_SCALE_IN: &str = "dxf_schedule_pause_scale_in";

/// Go-compatible name retained for migrated call sites.
///
/// 保留 Go 风格别名，供已迁移调用点继续使用 PascalCase 名称。
pub use DXF_SCHEDULE_PAUSE_SCALE_IN as DXFSchedulePauseScaleIn;
