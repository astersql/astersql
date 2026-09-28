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

// IMPORT INTO 任务级 Prometheus 指标管理。
//
// 按分布式任务 ID 引用计数注册/注销 Lightning Common 指标集，
// 与 Go 侧 scheduler/executor 生命周期对齐，避免任务结束后泄漏全局 registry。

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use astersql_dxf_framework_proto as dxfproto;
use astersql_lightning_metric::Common;
use astersql_metrics as tidbmetrics;
use astersql_util_promutil as promutil;

/// 单个任务的已注册指标及引用计数。
struct TaskMetrics {
    metrics: Arc<Common>,
    counter: usize,
}

/// Owns one registered import metric set per distributed task and mirrors the
/// Go reference-counted scheduler/executor lifetime.
/// 按任务 ID 持有一套已注册的导入指标，引用计数镜像 Go 调度器/执行器生命周期。
pub struct TaskMetricManager {
    metrics_map: Mutex<HashMap<i64, TaskMetrics>>,
}

impl Default for TaskMetricManager {
    fn default() -> Self {
        Self {
            metrics_map: Mutex::new(HashMap::new()),
        }
    }
}

/// 进程内全局指标管理器单例。
pub static METRICS_MANAGER: LazyLock<TaskMetricManager> = LazyLock::new(TaskMetricManager::default);

impl TaskMetricManager {
    /// 获取或创建 task_id 对应的指标集，并递增引用计数。
    pub fn get_or_create_metrics(&self, task_id: i64) -> Arc<Common> {
        let mut metrics_map = self
            .metrics_map
            .lock()
            .expect("import task metric mutex poisoned");
        // 首次出现时用 TaskID 标签注册一套 ImportMetrics。
        let task_metrics = metrics_map.entry(task_id).or_insert_with(|| {
            let labels =
                HashMap::from([(dxfproto::TaskIDLabelName.to_owned(), task_id.to_string())]);
            TaskMetrics {
                metrics: Arc::new(tidbmetrics::import::GetRegisteredImportMetrics(
                    promutil::NewDefaultFactory(),
                    labels,
                )),
                counter: 0,
            }
        });
        task_metrics.counter += 1;
        Arc::clone(&task_metrics.metrics)
    }

    /// 递减引用计数；归零时从 map 移除并 UnregisterImportMetrics。
    pub fn unregister(&self, task_id: i64) {
        let mut metrics_map = self
            .metrics_map
            .lock()
            .expect("import task metric mutex poisoned");
        let should_remove = metrics_map
            .get_mut(&task_id)
            .map(|task_metrics| {
                task_metrics.counter = task_metrics.counter.saturating_sub(1);
                task_metrics.counter == 0
            })
            .unwrap_or(false);
        if should_remove {
            if let Some(task_metrics) = metrics_map.remove(&task_id) {
                let mut metrics = (*task_metrics.metrics).clone();
                tidbmetrics::import::UnregisterImportMetrics(&mut metrics);
            }
        }
    }

    /// 当前仍持有指标的任务数量（用于测试断言）。
    pub fn registered_task_count(&self) -> usize {
        self.metrics_map
            .lock()
            .expect("import task metric mutex poisoned")
            .len()
    }
}

// Go-name compatibility for the scheduler/task-executor migration.
/// 与 Go `metricsManager` 命名兼容的全局别名。
pub use METRICS_MANAGER as metricsManager;
