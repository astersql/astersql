// Copyright 2017 PingCAP, Inc.
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

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

// TiDB 实例级系统变量副作用钩子（hook）注册表。
//
// Go 侧部分 `SET GLOBAL` 会触发跨模块行为（如切换 MDL、启用/禁用 DDL、
// 调整 PD 客户端动态选项、外部时间戳等）。本文件用可注入的函数槽位
// 解耦变量包与具体实现，便于测试替换与延迟绑定。

use std::sync::{Arc, LazyLock, RwLock};

use crate::{Context, VariableError};

/// 设置 `i64` 值的钩子类型。
pub type Int64Setter = Arc<dyn Fn(i64) + Send + Sync>;
/// 读取 `i64` 值的钩子类型。
pub type Int64Getter = Arc<dyn Fn() -> i64 + Send + Sync>;
/// 设置字符串键值对并可能失败的钩子。
pub type StringPairSetter = Arc<dyn Fn(&str, &str) -> Result<(), VariableError> + Send + Sync>;
/// 设置布尔值的钩子。
pub type BoolSetter = Arc<dyn Fn(bool) + Send + Sync>;
/// 设置布尔值且可能失败的钩子。
pub type FallibleBoolSetter = Arc<dyn Fn(bool) -> Result<(), VariableError> + Send + Sync>;
/// 无参、可能失败的副作用钩子。
pub type FallibleVoidHook = Arc<dyn Fn() -> Result<(), VariableError> + Send + Sync>;
/// 带执行上下文设置时间戳的钩子。
pub type ContextTimestampSetter =
    Arc<dyn Fn(&Context, u64) -> Result<(), VariableError> + Send + Sync>;
/// 带执行上下文读取时间戳的钩子。
pub type ContextTimestampGetter = Arc<dyn Fn(&Context) -> Result<u64, VariableError> + Send + Sync>;
/// 带上下文校验字符串（如云存储 URI）的钩子。
pub type ContextStringValidator =
    Arc<dyn Fn(&Context, &str) -> Result<(), VariableError> + Send + Sync>;
/// 设置时长（通常为纳秒）的钩子。
pub type DurationSetter = Arc<dyn Fn(i64) -> Result<(), VariableError> + Send + Sync>;
/// 带上下文设置大小（字节）的钩子。
pub type ContextSizeSetter = Arc<dyn Fn(&Context, u64) -> Result<(), VariableError> + Send + Sync>;
/// 设置 `u32` 值的钩子。
pub type U32Setter = Arc<dyn Fn(u32) + Send + Sync>;
/// 无参、无返回值的副作用钩子。
pub type VoidHook = Arc<dyn Fn() + Send + Sync>;

/// 生成一对 set/get 钩子槽位：底层为 `LazyLock<RwLock<Option<T>>>`。
macro_rules! hook_slot {
    ($slot:ident, $ty:ty, $setter:ident, $getter:ident) => {
        static $slot: LazyLock<RwLock<Option<$ty>>> = LazyLock::new(|| RwLock::new(None));

        pub fn $setter(hook: Option<$ty>) {
            *$slot.write().expect("variable hook poisoned") = hook;
        }

        pub fn $getter() -> Option<$ty> {
            $slot.read().expect("variable hook poisoned").clone()
        }
    };
}

hook_slot!(
    SET_MEM_QUOTA_ANALYZE,
    Int64Setter,
    set_mem_quota_analyze_hook,
    get_mem_quota_analyze_setter
);
// ANALYZE（收集统计信息）内存配额
hook_slot!(
    GET_MEM_QUOTA_ANALYZE,
    Int64Getter,
    set_get_mem_quota_analyze_hook,
    get_mem_quota_analyze_hook
);
// 统计信息缓存容量
hook_slot!(
    SET_STATS_CACHE_CAPACITY,
    Int64Setter,
    set_stats_cache_capacity_hook,
    get_stats_cache_capacity_hook
);
// PD（Placement Driver）客户端动态选项
hook_slot!(
    SET_PD_CLIENT_DYNAMIC_OPTION,
    StringPairSetter,
    set_pd_client_dynamic_option_hook,
    get_pd_client_dynamic_option_hook
);
// MDL：Metadata Lock，元数据锁
hook_slot!(
    SWITCH_MDL,
    FallibleBoolSetter,
    set_switch_mdl_hook,
    get_switch_mdl_hook
);
// DDL：Data Definition Language，数据定义语言（建表/改表等）
hook_slot!(
    ENABLE_DDL,
    FallibleVoidHook,
    set_enable_ddl_hook,
    get_enable_ddl_hook
);
hook_slot!(
    DISABLE_DDL,
    FallibleVoidHook,
    set_disable_ddl_hook,
    get_disable_ddl_hook
);
hook_slot!(
    SWITCH_FAST_CREATE_TABLE,
    FallibleBoolSetter,
    set_switch_fast_create_table_hook,
    get_switch_fast_create_table_hook
);
// 外部时间戳：用于跨系统对齐读视图
hook_slot!(
    SET_EXTERNAL_TIMESTAMP,
    ContextTimestampSetter,
    set_external_timestamp_hook,
    get_external_timestamp_setter
);
hook_slot!(
    GET_EXTERNAL_TIMESTAMP,
    ContextTimestampGetter,
    set_get_external_timestamp_hook,
    get_external_timestamp_hook
);
hook_slot!(
    SET_GLOBAL_RESOURCE_CONTROL,
    BoolSetter,
    set_global_resource_control_hook,
    get_global_resource_control_hook
);
hook_slot!(
    VALIDATE_CLOUD_STORAGE_URI,
    ContextStringValidator,
    set_validate_cloud_storage_uri_hook,
    get_validate_cloud_storage_uri_hook
);
// 低精度 TSO 更新间隔
hook_slot!(
    SET_LOW_RESOLUTION_TSO_UPDATE_INTERVAL,
    DurationSetter,
    set_low_resolution_tso_update_interval_hook,
    get_low_resolution_tso_update_interval_hook
);
// Schema 缓存大小（InfoSchema 内存占用上限一类）
hook_slot!(
    CHANGE_SCHEMA_CACHE_SIZE,
    ContextSizeSetter,
    set_change_schema_cache_size_hook,
    get_change_schema_cache_size_hook
);
hook_slot!(
    ENABLE_STATS_OWNER,
    FallibleVoidHook,
    set_enable_stats_owner_hook,
    get_enable_stats_owner_hook
);
hook_slot!(
    DISABLE_STATS_OWNER,
    FallibleVoidHook,
    set_disable_stats_owner_hook,
    get_disable_stats_owner_hook
);
hook_slot!(
    CHANGE_PD_METADATA_CIRCUIT_BREAKER_ERROR_RATE_THRESHOLD_RATIO,
    U32Setter,
    set_change_pd_metadata_circuit_breaker_error_rate_threshold_ratio_hook,
    get_change_pd_metadata_circuit_breaker_error_rate_threshold_ratio_hook
);

/// 启用全局资源控制的默认空实现钩子槽。
static ENABLE_GLOBAL_RESOURCE_CONTROL: LazyLock<RwLock<VoidHook>> =
    LazyLock::new(|| RwLock::new(Arc::new(|| {})));
/// 禁用全局资源控制的默认空实现钩子槽。
static DISABLE_GLOBAL_RESOURCE_CONTROL: LazyLock<RwLock<VoidHook>> =
    LazyLock::new(|| RwLock::new(Arc::new(|| {})));

/// 注册“启用全局资源控制”钩子（非 Option，始终有实现）。
pub fn set_enable_global_resource_control_hook(hook: VoidHook) {
    *ENABLE_GLOBAL_RESOURCE_CONTROL
        .write()
        .expect("variable hook poisoned") = hook;
}

/// 注册“禁用全局资源控制”钩子。
pub fn set_disable_global_resource_control_hook(hook: VoidHook) {
    *DISABLE_GLOBAL_RESOURCE_CONTROL
        .write()
        .expect("variable hook poisoned") = hook;
}

/// 调用已注册的启用全局资源控制钩子。
pub fn enable_global_resource_control() {
    let hook = ENABLE_GLOBAL_RESOURCE_CONTROL
        .read()
        .expect("variable hook poisoned")
        .clone();
    hook();
}

/// 调用已注册的禁用全局资源控制钩子。
pub fn disable_global_resource_control() {
    let hook = DISABLE_GLOBAL_RESOURCE_CONTROL
        .read()
        .expect("variable hook poisoned")
        .clone();
    hook();
}

/// 若已注入则调用启用 DDL 钩子，否则视为成功。
pub(crate) fn call_enable_ddl_hook() -> Result<(), VariableError> {
    get_enable_ddl_hook().map_or(Ok(()), |hook| hook())
}

/// 若已注入则调用禁用 DDL 钩子，否则视为成功。
pub(crate) fn call_disable_ddl_hook() -> Result<(), VariableError> {
    get_disable_ddl_hook().map_or(Ok(()), |hook| hook())
}

/// 若已注入则调用启用统计信息 Owner 钩子。
pub(crate) fn call_enable_stats_owner_hook() -> Result<(), VariableError> {
    get_enable_stats_owner_hook().map_or(Ok(()), |hook| hook())
}

/// 若已注入则调用禁用统计信息 Owner 钩子。
pub(crate) fn call_disable_stats_owner_hook() -> Result<(), VariableError> {
    get_disable_stats_owner_hook().map_or(Ok(()), |hook| hook())
}

/// 测试清理：清空 DDL/Stats Owner 相关可选钩子，避免用例间串扰。
pub fn clear_instance_hooks_for_test() {
    set_enable_ddl_hook(None);
    set_disable_ddl_hook(None);
    set_enable_stats_owner_hook(None);
    set_disable_stats_owner_hook(None);
}
