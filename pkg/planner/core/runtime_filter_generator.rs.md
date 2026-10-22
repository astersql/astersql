# `pkg/planner/core/runtime_filter_generator.rs`

## 文件定位

文件不会改写 `PlanNode`，也不会把过滤器对象挂到 HashJoin 或 TableScan；生成结果仅保存在 `RuntimeFilterGenerator::filters`。

## 核心职责

- `RuntimeFilterGenerator::GenerateRuntimeFilter` 从给定计划根启动深度优先遍历。
- 私有方法 `visit` 在 TiFlash `HashJoin` 的非 `inner_child` 子树上携带每条等值条件，在到达 `TableScan` 时生成 `RuntimeFilter`。
- `visit` 把经过 `ExchangeReceiver` 的候选标记为 `Global`，但在扫描节点处直接丢弃 `Global` 候选，反映当前不支持跨 Fragment Runtime Filter 的限制。
- `matchRFJoinType` 表达 build 侧方向相关的连接类型门禁；`belongsToSameFragment` 表达以 `ExchangeReceiver` 为边界的子树归属判断。当前生产遍历 `visit` 并未调用这两个辅助函数，二者主要由独立 Rust 测试直接验证。

文件不会改写 `PlanNode`，也不会把过滤器对象挂到 HashJoin 或 TableScan；生成结果仅保存在 `RuntimeFilterGenerator::filters`。这一点与 Go 版 `assignRuntimeFilter` 的实际算子写回行为不同。

## 主要符号

- `RuntimeFilterMode::{Local, Global}`：过滤器作用域。`Local` 表示未跨越 `ExchangeReceiver`；`Global` 表示候选传播路径已跨 Fragment 边界。
- `RuntimeFilter`：不可变语义记录，字段包括连续编号 `id`、来源连接 `source_join`、目标扫描 `target_scan`、字符串形式的 `build_expr`/`probe_expr` 和 `mode`。
- `RuntimeFilterGenerator { next_id, filters }`：遍历状态。`next_id` 私有且从 `Default` 的 0 开始；`filters` 公开，供调用方读取结果。
- `GenerateRuntimeFilter(&mut self, plan: &PlanNode)`：公开入口。它不会清空既有状态，所以同一生成器多次调用时会累积过滤器并延续编号。
- `visit(&mut self, plan, sources)`：私有递归核心。`sources` 的元组是 `(source_join_id, build_expr, probe_expr, mode)`，按值传入，并在分支处克隆。
- `matchRFJoinType(join, right_is_build_side)`：公开静态辅助函数。右侧为 build 时拒绝 `LeftOuterJoin`、`AntiSemiJoin`、`LeftOuterSemiJoin`、`AntiLeftOuterSemiJoin`；左侧为 build 时仅拒绝 `RightOuterJoin`。
- `belongsToSameFragment(source, target)`：公开静态递归辅助函数。遇到 `ExchangeReceiver` 立即返回假；遇到 `TableScan` 只比较节点 ID；其他节点对子树做 `any` 搜索。

本文件没有模块级常量、trait、条件编译项或显式错误类型。

## 执行流程

1. `optimizer.rs::postOptimize` 在其他后优化步骤之后，根据 `enable_runtime_filter` 创建默认生成器并调用 `GenerateRuntimeFilter(plan)`。
2. `GenerateRuntimeFilter` 以空候选集合调用 `visit`。
3. `visit` 首先检查当前节点：若为 `ExchangeReceiver`，将当前路径上所有候选模式改为 `Global`，但仍继续遍历其子树。
4. 若当前节点为 `TableScan`，逐个消费候选。`Global` 候选被跳过；每个 `Local` 候选产生一条 `RuntimeFilter`，目标 ID 取当前扫描节点 ID，编号成功追加后递增。扫描是终点，不再遍历其子节点。
5. 若当前节点为 `HashJoin`，分别遍历每个孩子。每个孩子先获得已有候选的克隆；只有连接自身 `store_type == StoreType::TiFlash` 且孩子序号不等于 `inner_child` 时，才为 `equal_conditions` 的每一对字符串追加一个 `Local` 候选。因此过滤器被传向 probe 子树，而不会传向 inner/build 子树。
6. 非 HashJoin、非 TableScan 的普通节点把候选集合分别克隆给每个子树继续搜索。
7. 返回 `postOptimize` 后，调用方只读取 `generator.filters.len()`，追加 `runtime-filters:<数量>` 到根节点 `operator_info`；当前路径没有把 `filters` 本身写回计划树。

## 数据与状态

核心持久状态只有 `next_id` 与 `filters`。编号只在 `Local` 候选真正到达扫描并追加成功时增长，被丢弃的 `Global` 候选不占用编号。遍历过程中的 `sources` 是路径局部值：HashJoin 和普通多子节点都会克隆它，从而避免兄弟子树互相修改候选模式或内容。

表达式仅以 `String` 保存，来源是 `PlanKind::HashJoin.equal_conditions: Vec<(String, String)>`；本文件不解析列、类型、NULL-safe equality 或表达式变换。节点身份也只使用 `i32` 的 `PlanNode.id`。`RuntimeFilter`、模式和生成器均不持有计划节点引用，因此遍历结束后没有借用或资源生命周期依赖。

重要不变量是：只有 TiFlash HashJoin 的非 inner 子树能够新增候选；跨过 ExchangeReceiver 的候选不会落到扫描；一次默认生成器运行中，成功生成的 ID 从 0 连续递增。独立测试 `runtime_filter_generator_requires_tiflash_and_keeps_all_equal_conditions` 验证 TiKV 不生成、多个等值条件保持全部结果与连续编号。

## 依赖与调用关系

上游生产调用边为 `pkg/planner/core/optimizer.rs::postOptimize -> RuntimeFilterGenerator::GenerateRuntimeFilter -> visit`。`postOptimize` 由同文件优化主流程调用，并以 `OptimizeOptions.enable_runtime_filter` 控制是否执行。模块装配与公开边界见 `pkg/planner/core/lib.rs`。

直接类型依赖均来自当前 crate：`JoinType`、`PlanKind`、`PlanNode`，以及以全限定路径使用的 `crate::StoreType::TiFlash`。这些轻量定义位于 `pkg/planner/core/common_plans.rs`。本文件没有直接使用 `Cargo.toml` 中列出的外部 crate；其 crate 边界仍由 `astersql-planner-core` 的 `lib.rs` 和 workspace 依赖共同决定。

RustCodeGraph 的精确查询能定位 Rust `RuntimeFilterGenerator` 结构和 `optimizer.rs` 的导入，但索引把同名方法查询解析到 Go 实现，且按文件过滤未返回目标文件；因此调用边同时由 `optimizer.rs`、`lib.rs` 的真实源码和 `rg` 交叉核对。当前没有其他非测试 Rust 调用者。

## 错误处理与边界

所有函数均不返回 `Result`，没有日志或显式错误传播；不支持的情形采用“不生成”策略：非 TiFlash HashJoin 不新增候选，Global 候选在 TableScan 被静默跳过，找不到目标扫描时 `belongsToSameFragment` 返回假。

需要注意的行为边界如下：

- `inner_child` 未校验是否落在 `children` 范围内；越界不会 panic，但会使所有实际孩子都被视为 probe 侧并接收新候选。
- `matchRFJoinType` 并未被 `visit` 调用，所以生产遍历不会根据连接类型过滤；而且轻量 `common_plans.rs::JoinType` 本身没有 `FullOuterJoin` 变体。不能把该辅助函数的单测当成生成路径已应用连接语义的证据。
- `belongsToSameFragment` 同样未接入 `visit`；实际模式判定由候选是否经过 `ExchangeReceiver` 完成。
- 字符串等值条件全部接受，未实现 Go 版 `matchEQPredicate` 对 NULL-safe equality、隐藏/派生目标列、JSON/BLOB/hybrid/array build 列的拒绝逻辑。
- 同一生成器重复调用不会重置状态；如调用方需要独立编号空间，应新建或显式替换生成器。

## 并发与资源生命周期

生成器通过 `&mut self` 串行修改状态，文件内没有锁、原子变量、异步任务、通道、事务或 I/O。`PlanNode` 只通过共享引用读取，过滤器持有克隆后的字符串和标量 ID，不会延长计划树生命周期。

主要资源成本来自候选向量和字符串克隆：遍历每个分支都会克隆当前 `sources`，每遇到符合条件的 TiFlash HashJoin 还会克隆所有等值条件字符串。嵌套连接和分叉较多时，时间/内存开销会随“路径候选数 × 分支数”增长。若未来并行化遍历，必须重新设计全局连续 ID 分配与结果顺序；当前测试隐含依赖深度优先、子节点顺序和等值条件顺序所形成的确定性编号。

## 与 Go 版本的对应关系

- Go 用 `columnUniqueIDToRF` 按目标列唯一 ID 匹配具体扫描输出列；Rust 把条件字符串沿整条 probe 子树传播，所以该子树内每个可达 TableScan 都可能收到同一候选。
- Go 的 `generateRuntimeFilterInterval` 在构造前调用 `matchRFJoinType` 和 `matchEQPredicate`；Rust `visit` 未调用对应连接门禁，也没有等值谓词门禁。
- Go 的 `physicalop.NewRuntimeFilter` 把过滤器关联到真实 HashJoin，并由 `Assign` 写入 TableScan；Rust 只生成独立记录，生产调用者仅消费数量。
- Go 对 `PhysicalTableReader` 有专门递归与 TiFlash `TablePlans` 重建；Rust 轻量树按普通子节点统一遍历。
- Go 通过 `IDGenerator`、列映射和日志记录管理状态；Rust 用 `usize` 计数器与值类型向量，不记录拒绝原因。
- Go 明确拒绝 FullOuterJoin；Rust 的轻量 `JoinType` 不含该变体，且 `matchRFJoinType` 不在生产路径上。

Go 测试 `pkg/planner/core/runtime_filter_generator_test.go` 通过 SQL、TiFlash 模拟副本、failpoint 和 EXPLAIN golden 数据覆盖完整规划器行为；Rust 测试 `runtime_filter_generator_test.rs` 只覆盖轻量树收集、TiFlash/多条件、部分连接类型门禁和 Fragment 搜索。两套测试层级不同，不能用 Rust 单测推断 Go 的全部兼容语义已实现。

## 扩展指南

若要把 Rust 行为向 Go 靠齐，优先在 `GenerateRuntimeFilter/visit` 的候选创建阶段接入连接类型门禁，并为轻量计划表示补足明确的 join type/build-side 信息；随后再引入可验证的列身份与谓词类型，而不是继续依赖字符串。若要真正影响执行计划，需要设计将 `RuntimeFilter` 关联回 HashJoin/TableScan 的结构，并同步调整 `optimizer.rs::postOptimize`，不能只保留数量标记。

修改 Fragment 规则时，应同时检查 `visit` 对 `ExchangeReceiver` 的模式转换和 `belongsToSameFragment`，避免两个公开语义分叉。支持 Global RF 时，应替换 TableScan 分支中的 `continue`，并明确跨 Fragment 的序列化、发送和接收生命周期。优化克隆开销时，必须保持兄弟分支隔离及稳定编号，或明确改变顺序契约。

测试必须继续放在独立文件 `pkg/planner/core/runtime_filter_generator_test.rs`，不要内嵌到生产源文件。至少应补充：生产路径应用连接门禁、ExchangeReceiver 下 Global 丢弃、嵌套/多个扫描的目标匹配、重复调用状态、异常 `inner_child`、以及未来谓词类型/列身份规则。若改变完整 SQL 计划语义，还应同步 Go 测试意图和 `pkg/planner/core/testdata/runtime_filter_generator_suite_{in,out}.json`，并评估兼容性（EXPLAIN 输出）、正确性（过滤目标与外连接语义）及性能（候选复制和扫描数量）。
