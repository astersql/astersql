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

// Prometheus 指标注册表抽象：统一注册、强制注册与注销接口。
//
// 对应 Go `pkg/util/promutil` 的 Registry。提供空操作实现（指标已在 factory 自动注册时使用）
// 与默认 `prometheus::Registry` 包装实现。

#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

// Registry is the interface to register or unregister metrics.
/// 指标注册表接口：注册、强制注册与注销 Collector。
pub trait Registry: Send + Sync {
    /// 注册单个 Collector；失败时返回错误。
    fn Register(&self, collector: Box<dyn prometheus::core::Collector>) -> prometheus::Result<()>;
    /// 批量强制注册；任一失败时按实现约定 panic。
    fn MustRegister(&self, collectors: Vec<Box<dyn prometheus::core::Collector>>);
    /// 注销 Collector；返回是否成功移除。
    fn Unregister(&self, collector: Box<dyn prometheus::core::Collector>) -> bool;
}

/// 空操作注册表：所有注册忽略输入并成功，注销恒为 true。
struct noopRegistry;

impl Registry for noopRegistry {
    fn Register(&self, _collector: Box<dyn prometheus::core::Collector>) -> prometheus::Result<()> {
        Ok(())
    }

    fn MustRegister(&self, _collectors: Vec<Box<dyn prometheus::core::Collector>>) {}

    fn Unregister(&self, _collector: Box<dyn prometheus::core::Collector>) -> bool {
        true
    }
}

/// 默认注册表：包装 `prometheus::Registry` 做真实注册。
struct defaultRegistry(prometheus::Registry);

impl Registry for defaultRegistry {
    fn Register(&self, collector: Box<dyn prometheus::core::Collector>) -> prometheus::Result<()> {
        self.0.register(collector)
    }

    fn MustRegister(&self, collectors: Vec<Box<dyn prometheus::core::Collector>>) {
        // 逐个注册；失败即 panic，对齐 Go MustRegister 语义。
        for collector in collectors {
            self.0
                .register(collector)
                .expect("metric registration failed");
        }
    }

    fn Unregister(&self, collector: Box<dyn prometheus::core::Collector>) -> bool {
        self.0.unregister(collector).is_ok()
    }
}

// NewNoopRegistry returns a Registry that does nothing.
// It is used for the case where metrics have been registered
// in factory automatically.
/// 构造空操作 Registry；用于指标已由 factory 自动注册的场景。
pub fn NewNoopRegistry() -> Box<dyn Registry> {
    Box::new(noopRegistry)
}

// NewDefaultRegistry returns a default implementation of Registry.
/// 构造基于新建 `prometheus::Registry` 的默认实现。
pub fn NewDefaultRegistry() -> Box<dyn Registry> {
    Box::new(defaultRegistry(prometheus::Registry::new()))
}
