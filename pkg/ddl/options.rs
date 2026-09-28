// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// DDL 子系统构造选项（functional options）模块。
//
// 提供与 Go 侧一致的「选项闭包」模式：调用方组合若干 `with_*` 函数，
// 再由 `apply_options` 应用到默认 `Options`，用于注入 etcd 客户端、
// 存储引擎、自增 ID 客户端、信息模式缓存（info cache：缓存表/库元数据）、
// schema lease（模式租约：DDL 变更后等待各节点同步的时间窗口）等依赖。

use std::sync::Arc;
use std::time::Duration;

use ddl_systable::SchemaLoader;

/// DDL 执行器 / owner 相关依赖的可配置项集合。
#[derive(Clone, Default)]
pub struct Options {
    /// etcd 客户端标识；用于 DDL owner 竞选与元数据协调（PD 依赖 etcd）。
    pub etcd_client: Option<String>,
    /// 底层 KV / 表存储标识。
    pub store: Option<String>,
    /// 自增 ID（auto increment）分配客户端。
    pub auto_id_client: Option<String>,
    /// 信息模式缓存标识。
    pub info_cache: Option<String>,
    /// Schema lease 时长。
    pub lease: Duration,
    /// 模式加载器（从元存储装载 DB/Table 信息）。
    pub schema_loader: Option<Arc<dyn SchemaLoader>>,
    /// DDL 事件发布所用存储。
    pub event_publish_store: Option<String>,
}
/// 一次性修改 `Options` 的闭包类型，可跨线程传递。
pub type OptionFn = Box<dyn FnOnce(&mut Options) + Send>;
/// 设置 etcd 客户端选项。
pub fn with_etcd_client(value: impl Into<String> + Send + 'static) -> OptionFn {
    let value = value.into();
    Box::new(move |options| options.etcd_client = Some(value))
}
/// 设置存储引擎选项。
pub fn with_store(value: impl Into<String> + Send + 'static) -> OptionFn {
    let value = value.into();
    Box::new(move |options| options.store = Some(value))
}
/// 设置信息模式缓存选项。
pub fn with_info_cache(value: impl Into<String> + Send + 'static) -> OptionFn {
    let value = value.into();
    Box::new(move |options| options.info_cache = Some(value))
}
/// 设置自增 ID 客户端选项。
pub fn with_auto_id_client(value: impl Into<String> + Send + 'static) -> OptionFn {
    let value = value.into();
    Box::new(move |options| options.auto_id_client = Some(value))
}
/// 设置 schema lease 时长。
pub fn with_lease(value: Duration) -> OptionFn {
    Box::new(move |options| options.lease = value)
}
/// 设置模式加载器选项。
pub fn with_schema_loader(loader: Arc<dyn SchemaLoader>) -> OptionFn {
    Box::new(move |options| options.schema_loader = Some(loader))
}
/// 设置事件发布存储选项。
pub fn with_event_publish_store(value: impl Into<String> + Send + 'static) -> OptionFn {
    let value = value.into();
    Box::new(move |options| options.event_publish_store = Some(value))
}
/// 按顺序应用全部选项闭包，得到最终配置。
pub fn apply_options(options: impl IntoIterator<Item = OptionFn>) -> Options {
    let mut result = Options::default();
    for option in options {
        option(&mut result);
    }
    result
}
