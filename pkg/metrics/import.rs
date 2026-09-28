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

// 数据导入（import）相关 Prometheus 指标的注册与注销入口。
//
// 导入路径复用 Lightning 风格的 `metric::Common` 指标组；调用方提供 Factory 与
// 常量标签，本模块合并全局常量标签后注册到默认 Registerer，任务结束时再整体注销。

use crate::bindinfo::compat_prometheus::{
    CounterCompat as _, GaugeCompat as _, MetricCompat as _, ObserverCompat as _,
};
use crate::bindinfo::{compat_metricscommon as metricscommon, compat_prometheus as prometheus};
use crate::*;
use crate::{metric, promutil};

/// 导入指标使用的 Prometheus subsystem 名，与 Go 包内常量一致。
// importMetricSubsystem 对应 Go 的包内常量，作为所有导入指标的 subsystem。
const importMetricSubsystem: &str = "import";

/// 创建并注册一组导入通用指标，返回可观测的 `metric::Common` 句柄。
// GetRegisteredImportMetrics 对应 Go 的同名函数：合并调用方标签，创建通用导入指标并注册到默认注册器。
// factory 和 constLabels 都来自外部依赖；保留调用形状，不在文件加载时自动执行注册副作用。
pub fn GetRegisteredImportMetrics(
    factory: Box<dyn promutil::Factory>,
    constLabels: prometheus::Labels,
) -> metric::Common {
    // 公共标签在创建 collector 前合并，避免调用方标签漏掉 metrics 包统一附加的常量标签。
    let mergedCstLabels = metricscommon::GetMergedConstLabels(constLabels);
    let metrics = metric::new_common(
        factory.as_ref(),
        TiDB,
        importMetricSubsystem,
        mergedCstLabels,
    );

    // 与 Go 一致注册到进程级默认 Registerer；重复注册等错误语义由 metric::Common 封装处理。
    metrics.register_to(&prometheus::DefaultRegisterer);
    metrics
}

/// 从默认注册器注销此前注册的整组导入指标。
// UnregisterImportMetrics 对应 Go 的同名函数，从默认注册器移除此前注册的整组导入指标。
// Go 接收 *metric.Common；这里用可变借用表达调用期间独占访问，而不接管指标对象的生命周期。
pub fn UnregisterImportMetrics(metrics: &mut metric::Common) {
    metrics.unregister_from(&prometheus::DefaultRegisterer);
}
