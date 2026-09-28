// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 这段逻辑只保存 DDL owner 检查 backfill job 的时间间隔配置。

// 分布式 DDL owner 相关的时间间隔配置。
//
// DDL（数据定义语言）owner 是集群中唯一负责执行 DDL 任务的节点；
// backfill（回填）指在加索引等 DDL 过程中，后台批量补写历史数据的任务。
// 本模块只定义 owner 轮询 backfill job 状态时使用的两个时间间隔，
// 以可变全局变量形式存在，便于测试时缩短等待时间。

use std::time::Duration;

// CheckBackfillJobFinishInterval is export for test.
// CheckBackfillJobFinishInterval 对应 Go 包变量，供测试调整 backfill 完成检查频率。
/// DDL owner 检查 backfill job 是否完成的轮询间隔（默认 300 毫秒）。
///
/// 导出为可变全局变量是为了在测试中调小间隔、加快用例执行；
/// 生产代码只应读取该值。
pub static mut CheckBackfillJobFinishInterval: Duration = Duration::from_millis(300);

// UpdateBackfillJobRowCountInterval is the interval of updating the job row count.
// UpdateBackfillJobRowCountInterval 对应 Go 包变量，表示更新 backfill job row count 的间隔。
/// 更新 backfill job 已处理行数（row count）的间隔（默认 3 秒）。
///
/// row count 用于向用户展示 DDL 回填进度，周期性刷新可避免频繁写元数据。
pub static mut UpdateBackfillJobRowCountInterval: Duration = Duration::from_secs(3);
