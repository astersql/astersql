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

// 规划器计划抽象与规划上下文边界。
//
// 定义 `Plan` / `LogicalPlan` / `PhysicalPlan`、规划上下文（PlanContext）、
// JoinType 与 PossibleProperties 等核心类型。逻辑计划（Logical Plan）是可改写的
// 算子树；物理计划（Physical Plan）绑定具体执行算法与代价。本文件只声明对象安全
// 接口，不实现具体算子。

// 外部模块类型暂沿用 Go 包的语义名称；本轮不进行 mod.rs 连线，也不声称该文件可以独立编译。

use std::any::Any;
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::{Arc, Mutex};

use crate::{Error, Task, types};

/// Object-safe alias for the ranger context exposed by [`PlanContext`].
/// 对象安全的 ranger 上下文别名，供 [`PlanContext`] 暴露范围构建能力。
/// Ranger 根据谓词生成索引/表扫描的 key 范围。
pub type RangerContext<'a> = planctx::rangerctx::RangerContext<'a>;

/// 统计信息异步加载完成后的同步等待接口。
pub trait StatsLoadWaiter: Send + Sync {
    /// 阻塞直到会话相关的统计加载完成；失败返回错误字符串。
    fn SyncWaitStatsLoad(
        &self,
        session_vars: &planctx::variable::SessionVars,
    ) -> Result<(), String>;
}

/// Object-safe planner context boundary shared by planner base and operator crates.
///
/// `planctx::PlanContext` keeps concrete infoschema types as associated types,
/// so it cannot be stored directly behind the Go-interface-like `Arc<dyn ...>`
/// used by plan nodes.  This boundary erases only that infoschema detail while
/// preserving the session and expression services consumed by planner code.
///
/// 规划器与算子 crate 共享的对象安全上下文边界。
/// `planctx::PlanContext` 用关联类型绑定具体 infoschema，无法直接放进
/// 类似 Go 接口的 `Arc<dyn ...>`；本边界抹去 infoschema 细节，同时保留
/// 会话变量与表达式求值等规划期服务。
pub trait PlanContext {
    /// 分配下一个计划节点 ID。
    fn alloc_plan_id(&self) -> i32;
    /// Snapshot the current plan-node allocation frontier when supported.
    fn plan_id_checkpoint(&self) -> Option<i32> {
        None
    }
    /// Restore a previously captured allocation frontier.
    fn restore_plan_id_checkpoint(&self, _checkpoint: i32) {}
    /// Reset the plan-node allocator before each physical optimization.
    fn reset_plan_id(&self) {}
    /// Resolve an AST parameter marker's SQL byte offset to its ordinal in
    /// the currently bound prepared parameter list.
    fn prepared_param_index(&self, _sql_offset: usize) -> Option<usize> {
        None
    }
    fn prepared_limit_value(&self, _parameter_index: usize) -> Result<u64, String> {
        Err("Incorrect arguments to LIMIT".to_owned())
    }
    /// 是否在 EXPLAIN ID 中省略后缀。
    fn ignore_explain_id_suffix(&self) -> bool;
    /// 返回会话变量（会话级配置与运行时状态）。
    fn GetSessionVars(&self) -> &planctx::variable::SessionVars;
    /// 返回表达式求值上下文。
    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext;
    /// 返回用于构建扫描范围的 Ranger 上下文。
    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_>;
    /// 返回用于空值拒绝（null-reject）检查的表达式上下文。
    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext;
    /// 返回物理计划转 protobuf 时使用的构建上下文。
    fn GetBuildPBCtx(&self) -> &BuildPBContext;
    /// 按 protobuf 标量函数签名累加内置函数调用次数。
    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str);
    /// 可选：返回统计加载等待器；默认不提供。
    fn GetStatsLoadWaiter(&self) -> Option<&dyn StatsLoadWaiter> {
        None
    }
}

/// Thread-safe built-in function usage counts keyed by protobuf signature name.
/// 按 protobuf 签名名统计内置函数用量的线程安全计数器。
#[derive(Default)]
pub struct BuiltinFunctionUsageCounter {
    /// 签名名 → 调用次数。
    counts: Mutex<HashMap<String, u64>>,
}

impl BuiltinFunctionUsageCounter {
    /// 将指定签名的计数加一。
    pub fn Inc(&self, scalar_func_sig_name: &str) {
        let mut counts = self
            .counts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *counts.entry(scalar_func_sig_name.to_owned()).or_default() += 1;
    }

    /// 读取指定签名的累计次数；不存在时返回 0。
    pub fn Get(&self, scalar_func_sig_name: &str) -> u64 {
        self.counts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(scalar_func_sig_name)
            .copied()
            .unwrap_or_default()
    }
}

/// Shared Go-interface semantics for planner contexts.
/// 共享的规划上下文引用，对应 Go 侧以接口传递的 PlanContext。
pub type ContextRef = Arc<dyn PlanContext>;

/// 对应 Go 的 `planctx.BuildPBContext`，仅在物理计划转换 protobuf 时传递构建状态。
pub use planctx::BuildPBContext;

/// 对应 Go 的 `Plan`：描述从 AST 生成、经优化后交给执行器的一段执行流。
/// 新增公共方法时应追加在末尾，便于其它包中的实现按统一顺序核对。
pub trait Plan: Any {
    /// Rust runtime type boundary corresponding to Go interface assertions.
    /// 运行时类型边界，对应 Go 的接口断言（type assertion）。
    fn as_any(&self) -> &dyn Any;
    /// Mutable runtime type boundary for in-place physical-plan rewrites.
    /// 可变运行时类型边界，供物理计划路由原地替换具体算子。
    fn as_any_mut(&mut self) -> &mut dyn Any;
    /// Expose the physical tree without erasing concrete operator types.
    fn as_physical_plan(&self) -> Option<&dyn PhysicalPlan> {
        None
    }
    /// 返回计划输出 schema；Go 指针语义机械映射为共享借用。
    fn schema(&self) -> &expression::Schema;
    /// 返回计划节点 ID。
    fn id(&self) -> i32;
    /// 更新计划节点 ID。
    fn set_id(&mut self, id: i32);
    /// 返回计划类型；Go 可变参数映射为布尔切片。
    fn tp(&self, flags: &[bool]) -> String;
    /// 返回 EXPLAIN 使用的 ID 格式化对象。
    fn explain_id(&self, is_child_of_inl: &[bool]) -> Box<dyn Display + '_>;
    /// 返回算子自身的 EXPLAIN 信息。
    fn explain_info(&self) -> String;

    /// 按字符串键替换计划表达式中的列引用。
    /// Go map 中的列指针由共享引用表达，具体生命周期留给跨模块接线处理。
    fn replace_expr_columns(&mut self, replace: &HashMap<String, expression::Column>);

    /// 返回当前计划的构建上下文。
    fn s_ctx(&self) -> &ContextRef;
    /// 返回计划统计信息。
    fn stats_info(&self) -> &property::StatsInfo;
    /// 返回每个输出列对应的字段名。
    fn output_names(&self) -> types::NameSlice;
    /// 设置输出字段名。
    fn set_output_names(&mut self, names: types::NameSlice);

    /// 返回查询块偏移，例如 hint 中的 `@sel_2` 应作用于编号为 2 的子查询。
    fn query_block_offset(&self) -> i32;

    /// 为计划缓存克隆计划。带 `plan-cache-shallow-clone` 标记的字段允许共享，其余字段应深拷贝；
    /// bool 明确表示当前实现是否支持该克隆路径。
    fn clone_for_plan_cache(&self, new_ctx: ContextRef) -> (Option<Box<dyn Plan>>, bool);

    /// 标记包含该算子的计划不可缓存，并保存最终展示给用户的原因。
    fn set_noncacheable_reason(&mut self, reason: String);
    /// 读取不可缓存原因。
    fn get_noncacheable_reason(&self) -> String;
}

/// 对应 Go 的 `PhysicalPlan`：由物理算子组成的树。
pub trait PhysicalPlan: Plan {
    /// 使用成本模型 v1 计算指定任务类型的成本；实现可缓存此前结果。
    fn get_plan_cost_ver1(
        &mut self,
        task_type: property::TaskType,
        option: &costusage::PlanCostOption,
    ) -> Result<f64, Error>;

    /// 使用成本模型 v2 计算成本，附加布尔切片保留 Index Nested Loop 子节点语义。
    fn get_plan_cost_ver2(
        &mut self,
        task_type: property::TaskType,
        option: &costusage::PlanCostOption,
        is_child_of_inl: &[bool],
    ) -> Result<costusage::CostVer2, Error>;

    /// 把当前物理计划挂到任务之上并更新成本；子任务是 cop task 时实现可关闭它并返回 root task。
    fn attach_to_task(&self, tasks: Vec<Box<dyn Task>>) -> Box<dyn Task>;

    /// 转换为 tipb executor。此处仅保留潜在失败和存储类型参数，不执行网络或存储 IO。
    fn to_pb(
        &self,
        ctx: &mut BuildPBContext,
        store_type: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, Error>;

    /// 按子节点下标取得其所需物理属性。
    fn get_child_req_props(&self, idx: usize) -> &property::PhysicalProperty;
    /// 返回统计信息中的估算行数。
    fn stats_count(&self) -> f64;
    /// 收集物理计划内部的关联列。
    fn extract_correlated_cols(&self) -> Vec<expression::CorrelatedColumn>;
    /// 返回全部物理子计划。
    fn children(&self) -> Vec<&dyn PhysicalPlan>;
    /// 一次替换全部物理子计划。
    fn set_children(&mut self, children: Vec<Box<dyn PhysicalPlan>>);
    /// 替换指定下标的物理子计划。
    fn set_child(&mut self, index: usize, child: Box<dyn PhysicalPlan>);

    /// 解析表达式列下标，使后续执行能直接按下标读取行；错误必须向调用方传播。
    fn resolve_indices(&mut self) -> Result<(), Error>;
    /// 更新 basePhysicalPlan 内的统计信息。
    fn set_stats(&mut self, stats: property::StatsInfo);
    /// 返回生成计划摘要所需的归一化算子信息。
    fn explain_normalized_info(&self) -> String;
    /// 克隆物理计划并切换到新的规划上下文。
    fn clone_physical(&self, new_ctx: ContextRef) -> Result<Box<dyn PhysicalPlan>, Error>;
    /// 估算当前物理计划持有的内存字节数。
    fn memory_usage(&self) -> i64;

    /// 记录会把“单次 probe 行数”放大为“全部 probe 总行数”的父算子。
    /// 该信息用于 Index Join/Apply 内侧子树的展示换算，不改变 StatsInfo 本身。
    fn set_probe_parents(&mut self, parents: Vec<Box<dyn PhysicalPlan>>);
    /// 根据单次 probe 估算和 probeParents 计算 EXPLAIN 应展示的全部 probe 行数。
    fn get_est_row_count_for_display(&self) -> f64;
    /// 结合运行时统计和 probeParents 计算实际 probe 次数。
    fn get_actual_probe_count(&self, stats: &execdetails::execdetails::RuntimeStatsColl) -> i64;
}

/// 对应 Go 的 `LogicalPlan`：由逻辑算子组成、可参与谓词下推和列裁剪等优化的树。
pub trait LogicalPlan: Plan + cascades_base::HashEquals {
    /// Returns the cascades wrapper view used by Go's GroupExpression assertion.
    /// 返回 Cascades 包装视图，对应 Go 对 GroupExpression 的接口断言。
    fn as_group_expression(&self) -> Option<&dyn GroupExpression>;
    /// 严格编码逻辑计划，用于快速比较且要求无哈希冲突。
    fn hash_code(&self) -> Vec<u8>;

    /// 尽可能下推 WHERE/ON/HAVING 谓词，返回未下推谓词和可能变化的新根。
    fn predicate_push_down(
        &mut self,
        predicates: Vec<expression::ExprBox>,
    ) -> Result<(Vec<expression::ExprBox>, Box<dyn LogicalPlan>), Error>;

    /// 裁剪未使用列；无变化时实现可以返回当前逻辑计划的等价包装。
    fn prune_columns(
        &mut self,
        columns: Vec<expression::Column>,
    ) -> Result<Box<dyn LogicalPlan>, Error>;

    /// 根据自身及子节点 schema 收集唯一键信息。参数显式传入以兼容不持有 children/schema 的 cascades planner。
    fn build_key_info(
        &mut self,
        self_schema: &expression::Schema,
        child_schema: &[expression::Schema],
    );

    /// 在逻辑优化期间下推 TopN/Limit，并返回新的逻辑根。
    fn push_down_top_n(&mut self, top_n: Box<dyn LogicalPlan>) -> Box<dyn LogicalPlan>;
    /// 从 row_number 窗口函数过滤条件推导隐式 TopN。
    fn derive_top_n(&mut self) -> Box<dyn LogicalPlan>;
    /// 合并同一列及其等价类上的谓词。
    fn predicate_simplification(&mut self) -> Box<dyn LogicalPlan>;
    /// 根据列等价关系生成常量谓词。
    fn constant_propagation(
        &mut self,
        parent_plan: &dyn LogicalPlan,
        current_child_idx: usize,
    ) -> Box<dyn LogicalPlan>;
    /// 递归上拉供常量传播规则使用的常量谓词。
    fn pull_up_constant_predicates(&self) -> Vec<expression::ExprBox>;

    /// 递归推导当前树的统计信息，bool 表示统计是否发生变化。
    fn recursive_derive_stats(
        &mut self,
        column_groups: &[Vec<expression::Column>],
    ) -> Result<(property::StatsInfo, bool), Error>;

    /// 由子节点统计与 schema 推导当前节点统计；显式参数同样用于兼容 cascades planner。
    fn derive_stats(
        &mut self,
        child_stats: &[property::StatsInfo],
        self_schema: &expression::Schema,
        child_schema: &[expression::Schema],
        reloads: &[bool],
    ) -> Result<(property::StatsInfo, bool), Error>;

    /// 提取当前算子需要维护的列组 NDV，并尽量把父层列组需求继续传给子节点。
    fn extract_col_groups(
        &self,
        column_groups: &[Vec<expression::Column>],
    ) -> Vec<Vec<expression::Column>>;

    /// 为 join/aggregation 预计算可用排序属性；例如 Group By(a,b,c) 的排列受叶子有序索引限制。
    fn prepare_possible_properties(
        &self,
        schema: &expression::Schema,
        children_properties: &[PossiblePropertiesInfo],
    ) -> PossiblePropertiesInfo;

    /// 收集逻辑计划内部的关联列。
    fn extract_correlated_cols(&self) -> Vec<expression::CorrelatedColumn>;
    /// 返回该算子是否至多产生一行。
    fn max_one_row(&self) -> bool;
    /// 返回全部逻辑子计划。
    fn logical_children(&self) -> Vec<&dyn LogicalPlan>;
    /// 返回全部可变逻辑子计划，供自底向上的规则递归更新子节点元数据。
    fn logical_children_mut(&mut self) -> Vec<&mut dyn LogicalPlan>;
    /// 一次替换全部逻辑子计划。
    fn set_logical_children(&mut self, children: Vec<Box<dyn LogicalPlan>>);
    /// 替换指定下标的逻辑子计划。
    fn set_logical_child(&mut self, index: usize, child: Box<dyn LogicalPlan>);
    /// 回滚时间戳之后的 taskMap 日志。
    fn roll_back_task_map(&mut self, timestamp: u64);

    /// 判断当前子树能否下推到指定存储。Go 已标记此子树级检查为 deprecated；新代码应使用算子自身检查。
    fn can_push_to_cop(&self, store: kv::StoreType) -> bool;
    /// 自底向上推导函数依赖集合。
    fn extract_fd(&self) -> fd::FDSet;
    /// 返回具体算子内部的 baseLogicalPlan 抽象。
    fn get_base_logical_plan(&self) -> &dyn LogicalPlan;
    /// 保存子树计划 ID 的 hash64。
    fn set_plan_ids_hash(&mut self, hash: u64);
    /// 读取子树计划 ID 的 hash64。
    fn get_plan_ids_hash(&self) -> u64;
    /// GroupExpression 返回其包装的具体逻辑计划，普通逻辑计划返回自身。
    fn get_wrapped_logical_plan(&self) -> &dyn LogicalPlan;
    /// 返回首个子节点的统计与 schema。
    fn get_child_stats_and_schema(&self) -> (&property::StatsInfo, &expression::Schema);
    /// 返回 join 左右子节点各自的统计与 schema。
    fn get_join_child_stats_and_schema(
        &self,
    ) -> (
        &property::StatsInfo,
        &property::StatsInfo,
        &expression::Schema,
        &expression::Schema,
    );
}

/// 对应 Go 的 `GroupExpression`，在 cascades memo 中包装一个逻辑计划及其输入组。
pub trait GroupExpression: LogicalPlan {
    /// 判断当前 group expression 是否已被编号为 i 的规则探索。
    fn is_explored(&self, index: usize) -> bool;
    /// 返回输入组数量。
    fn inputs_len(&self) -> usize;
    /// 按下标返回输入逻辑计划的 schema。
    fn get_input_schema(&self, index: usize) -> &expression::Schema;
}

/// 对应 Go 的泛型 `GetGEAndLogicalOp`：从公共逻辑计划引用中同时识别 group expression 和具体算子。
/// 普通逻辑计划直接向下转换为 T；若输入是 GroupExpression，则先取其包装计划再转换为 T。
pub fn get_ge_and_logical_op<T: LogicalPlan + 'static>(
    super_plan: &dyn LogicalPlan,
) -> (Option<&dyn GroupExpression>, Option<&T>) {
    if let Some(logical_op) = super_plan.as_any().downcast_ref::<T>() {
        return (None, Some(logical_op));
    }
    if let Some(group_expression) = super_plan.as_group_expression() {
        let logical_op = group_expression
            .get_wrapped_logical_plan()
            .as_any()
            .downcast_ref::<T>();
        return (Some(group_expression), logical_op);
    }
    (None, None)
}

/// 对应 Go 的 `JoinType`。显式判别值被 conflict_detector.go 使用，禁止重排或插值。
#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JoinType {
    /// 内连接。
    InnerJoin = 0,
    /// 左外连接。
    LeftOuterJoin = 1,
    /// 右外连接。
    RightOuterJoin = 2,
    /// 匹配右表任意行时只输出左表行。
    SemiJoin = 3,
    /// 右表无匹配行时输出左表行。
    AntiSemiJoin = 4,
    /// 输出左表行并附加是否匹配的布尔值。
    LeftOuterSemiJoin = 5,
    /// 输出左表行并附加是否未匹配的布尔值。
    AntiLeftOuterSemiJoin = 6,
    /// 保留左右两侧未匹配行。
    FullOuterJoin = 7,
}

impl JoinType {
    /// 判断是否属于会保留外侧行的连接类型。
    pub fn is_outer_join(self) -> bool {
        matches!(
            self,
            Self::LeftOuterJoin
                | Self::RightOuterJoin
                | Self::FullOuterJoin
                | Self::LeftOuterSemiJoin
                | Self::AntiLeftOuterSemiJoin
        )
    }

    /// 判断是否属于 semi/anti-semi 家族。
    pub fn is_semi_join(self) -> bool {
        matches!(
            self,
            Self::SemiJoin
                | Self::AntiSemiJoin
                | Self::LeftOuterSemiJoin
                | Self::AntiLeftOuterSemiJoin
        )
    }

    /// 判断是否为普通内连接。
    pub fn is_inner_join(self) -> bool {
        self == Self::InnerJoin
    }
}

impl std::fmt::Display for JoinType {
    /// 保留 Go `String` 方法的稳定英文名称，供 EXPLAIN 等展示路径使用。
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::InnerJoin => "inner join",
            Self::LeftOuterJoin => "left outer join",
            Self::RightOuterJoin => "right outer join",
            Self::FullOuterJoin => "full outer join",
            Self::SemiJoin => "semi join",
            Self::AntiSemiJoin => "anti semi join",
            Self::LeftOuterSemiJoin => "left outer semi join",
            Self::AntiLeftOuterSemiJoin => "anti left outer semi join",
        };
        formatter.write_str(text)
    }
}

/// 对应 Go 的 `PhysicalJoin`，提供物理 join 的公共能力；PhysicalApply 被有意排除。
pub trait PhysicalJoin: PhysicalPlan {
    /// 标记实现者确实是物理 join，保留 Go 的显式实现约束。
    fn physical_join_implement(&self);
    /// 返回内侧子节点下标。
    fn get_inner_child_idx(&self) -> usize;
    /// 返回具体连接类型。
    fn get_join_type(&self) -> JoinType;
}

/// 对应 Go 的 `PossiblePropertiesInfo`，保存逻辑子树可提供的全部排序属性。
#[derive(Clone)]
pub struct PossiblePropertiesInfo {
    /// 每个内层 Vec 是一组按顺序排列的列；None 保留 Go 的 nil 列指针语义。
    pub orders: Option<Vec<Vec<Option<expression::Column>>>>,
    /// 运行时裁剪信号；与 Go 一致，故意不参与 hash/equals。
    pub has_tiflash: bool,
}

impl PossiblePropertiesInfo {
    /// 对应 Go `Hash64`。先编码 nil/非 nil 标记，再按二维顺序编码长度和每一列。
    pub fn hash64(&self, hasher: &mut dyn cascades_base::Hasher) {
        // A Rust reference is always non-null, but Go includes the receiver's
        // non-nil marker in the stable hash stream before encoding Orders.
        hasher.HashByte(cascades_base::NotNilFlag);
        match &self.orders {
            None => hasher.HashByte(cascades_base::NilFlag),
            Some(orders) => {
                hasher.HashByte(cascades_base::NotNilFlag);
                hasher.HashInt(orders.len() as isize);
                for order in orders {
                    hasher.HashInt(order.len() as isize);
                    for column in order {
                        match column {
                            Some(column) => column.Hash64(hasher),
                            None => hasher.HashByte(cascades_base::NilFlag),
                        }
                    }
                }
            }
        }
    }

    /// 对应 Go `Equals`：严格比较 orders 的 nil 状态、二维长度及每个列对象；忽略 has_tiflash。
    pub fn equals(&self, other: &Self) -> bool {
        match (&self.orders, &other.orders) {
            (None, None) => true,
            (Some(left), Some(right)) => {
                left.len() == right.len()
                    && left.iter().zip(right).all(|(left_order, right_order)| {
                        left_order.len() == right_order.len()
                            && left_order.iter().zip(right_order).all(
                                |(left_col, right_col)| match (left_col, right_col) {
                                    (None, None) => true,
                                    (Some(left_col), Some(right_col)) => left_col.Equals(right_col),
                                    _ => false,
                                },
                            )
                    })
            }
            _ => false,
        }
    }
}
