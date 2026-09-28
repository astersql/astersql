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

// SQL 绑定（SQL Binding / Plan Binding）总控句柄模块。
//
// “SQL 绑定”是一种执行计划管理机制：数据库管理员可以把某条 SQL
// 固定到一条指定的提示（Hint）语句上，使优化器在生成执行计划
// （执行计划：数据库为一条 SQL 选择的具体执行步骤与访问路径）时
// 强制采用绑定的写法，从而稳定查询性能、规避优化器误判。
//
// 本模块定义 [`BindingHandle`] trait 及其默认实现 [`bindingHandle`]，
// 它把绑定子系统的三大组件聚合在一起：
// - [`BindingCacheUpdater`]：绑定信息的内存缓存及其后台刷新逻辑；
// - [`BindingOperator`]：绑定的增删改查操作（读写系统表 `mysql.bind_info`）；
// - [`BindingPlanEvolution`]：绑定的自动演进（自动尝试并采纳更优计划）。
//
// 另外还定义了绑定子系统使用的若干常量，例如缓存刷新周期、
// 分布式属主选举（owner election）使用的 etcd 键，以及维护
// `mysql.bind_info` 系统表所需的内置 SQL 语句。

use crate::{
    BindingCacheUpdater, BindingOperator, BindingPlanEvolution, BindingStore,
    NewBindingCacheUpdater, PlanRuntime, newBindingAuto, newBindingOperator,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// 绑定信息的租约周期：后台任务每隔该时长从存储层重新加载一次绑定，
/// 保证各节点内存缓存与系统表中的绑定数据最终一致。
pub const Lease: Duration = Duration::from_secs(3);
/// 绑定子系统在 etcd 中进行属主（owner）选举所用的键。
/// 集群中只有当选属主的节点负责执行绑定相关的全局后台任务。
pub const OwnerKey: &str = "/tidb/bindinfo/owner";
/// 日志与属主管理器中标识本子系统的名称前缀。
pub const Prompt: &str = "bindinfo";
/// 用作“绑定锁”的伪 SQL 文本。它并非真实查询，而是写入
/// `mysql.bind_info` 表中的一条特殊记录，通过更新这行记录
/// 来获取行锁，从而在多个节点之间互斥地执行绑定维护操作。
pub const BuiltinPseudoSQL4BindLock: &str = "builtin_pseudo_sql_for_bind_lock";
/// 获取绑定锁的 SQL：更新上述伪记录以持有其行锁，
/// 在事务提交前阻塞其他节点的并发绑定维护操作。
pub const LockBindInfoSQL: &str = "UPDATE mysql.bind_info SET source= 'builtin' WHERE original_sql= 'builtin_pseudo_sql_for_bind_lock'";
/// 清理重复伪绑定记录的 SQL：若历史原因导致伪记录存在多行，
/// 仅保留一行（按 `_tidb_rowid` 隐式行号取第一条），删除其余重复行。
pub const StmtRemoveDuplicatedPseudoBinding: &str = r#"DELETE FROM mysql.bind_info
       WHERE original_sql='builtin_pseudo_sql_for_bind_lock' AND
       _tidb_rowid NOT IN ( -- keep one arbitrary pseudo binding
         SELECT _tidb_rowid FROM mysql.bind_info WHERE original_sql='builtin_pseudo_sql_for_bind_lock' limit 1)"#;

/// 绑定子系统的统一入口 trait，聚合缓存、操作与计划演进三类能力。
///
/// 上层（如会话、优化器）通过该 trait 访问绑定功能，而无需关心
/// 各组件的具体实现与组装方式。
pub trait BindingHandle: Send + Sync {
    /// 返回绑定缓存更新器：负责维护绑定的内存缓存并与存储层同步。
    fn cache(&self) -> &Arc<dyn BindingCacheUpdater>;
    /// 返回绑定操作器：负责创建、删除、设置状态等绑定的读写操作。
    fn operator(&self) -> &Arc<dyn BindingOperator>;
    /// 返回计划演进组件：负责自动探索并采纳更优的执行计划绑定。
    fn evolution(&self) -> &Arc<dyn BindingPlanEvolution>;
    /// 返回指定系统变量的作用域。绑定相关变量均为会话（session）级。
    fn GetScope(&self, _name: &str) -> &'static str;
    /// 返回绑定子系统的统计信息（键值对形式），用于状态变量展示。
    fn Stats(&self) -> HashMap<String, String>;
}

/// [`BindingHandle`] 的默认实现：以组合方式持有三大组件的共享引用。
pub struct bindingHandle {
    /// 绑定缓存更新器组件。
    pub BindingCacheUpdater: Arc<dyn BindingCacheUpdater>,
    /// 绑定操作器组件。
    pub BindingOperator: Arc<dyn BindingOperator>,
    /// 绑定计划自动演进组件。
    pub BindingPlanEvolution: Arc<dyn BindingPlanEvolution>,
}

/// 构造并组装绑定句柄。
///
/// 参数说明：
/// - `store`：绑定的持久化存储（对应系统表 `mysql.bind_info`）；
/// - `runtime`：执行计划运行时接口，供计划演进组件试跑候选计划；
/// - `cache_capacity`：绑定内存缓存的容量上限（字节数）。
pub fn NewBindingHandle(
    store: Arc<dyn BindingStore>,
    runtime: Arc<dyn PlanRuntime>,
    cache_capacity: i64,
) -> Arc<dyn BindingHandle> {
    // 依次创建三个组件：缓存更新器直接对接存储层；
    // 操作器同时依赖存储与缓存（写入后需刷新缓存），并以 Lease 作为同步周期；
    // 计划演进组件依赖计划运行时来验证候选执行计划。
    let cache = NewBindingCacheUpdater(Arc::clone(&store), cache_capacity);
    let operator = newBindingOperator(store, Arc::clone(&cache), Lease);
    let evolution = newBindingAuto(runtime);
    Arc::new(bindingHandle {
        BindingCacheUpdater: cache,
        BindingOperator: operator,
        BindingPlanEvolution: evolution,
    })
}

/// 统计信息中的键名：记录绑定缓存最近一次成功更新的时间。
pub const lastPlanBindingUpdateTime: &str = "last_plan_binding_update_time";

impl BindingHandle for bindingHandle {
    fn cache(&self) -> &Arc<dyn BindingCacheUpdater> {
        &self.BindingCacheUpdater
    }

    fn operator(&self) -> &Arc<dyn BindingOperator> {
        &self.BindingOperator
    }

    fn evolution(&self) -> &Arc<dyn BindingPlanEvolution> {
        &self.BindingPlanEvolution
    }

    fn GetScope(&self, _name: &str) -> &'static str {
        "session"
    }

    fn Stats(&self) -> HashMap<String, String> {
        // 仅暴露一项统计：缓存最近一次更新的时间戳，
        // 便于运维判断绑定信息是否及时同步。
        HashMap::from([(
            lastPlanBindingUpdateTime.to_owned(),
            self.BindingCacheUpdater.LastUpdateTime().0.to_string(),
        )])
    }
}
