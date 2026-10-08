# `pkg/planner/core/operator/physicalop/physical_indexlookup.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-planner-core-operator-physicalop`，由同目录 `lib.rs` 以 `pub mod physical_indexlookup` 暴露。它描述本地索引回表（local index lookup）的一个轻量 Rust 表示：索引侧计划先产生行句柄，表侧计划再按句柄取完整行。直接依赖 `physical_common_plans.rs` 中的简化计划树类型，并通过 `kv`、`model` 和 `tipb` 编码存储类型、额外句柄列 ID 与下推协议。

它目前不是 Go `PhysicalLocalIndexLookUp` 的完整 `base::PhysicalPlan` 移植。仓库中的说明和引用共同表明它仍是独立骨架：`physical_utils_test.rs` 明确说该类型尚未加入 `base::PhysicalPlan` 体系；RustCodeGraph 对本文件公开构建/拆分入口未找到生产调用者；现有非本文件引用集中在独立测试、计划缓存克隆和缓存快照。因而它当前服务于局部语义保存和缓存表示，而不是已经接入 SQL 优化、执行器构建的完整主链。

## 核心职责

- `PhysicalLocalIndexLookup` 同时保存 `index_plan` 与 `table_plan` 两棵 `PhysicalPlanNode` 子树，并记录输出 `schema`、`keep_order` 和 `index_handle_offsets`。
- `index_handle_offsets_for_schema` 按 Go 规则为整数句柄寻找索引输出中的句柄列；公共句柄无需偏移，因为键可从索引值读取。
- `build_push_down_plan` 为表侧子树重分配节点 ID，并可依次包裹 `Selection`、`Projection`，最后生成 `PhysicalKind::LocalIndexLookup` 二元节点。
- `to_pb` 只为 TiKV 生成 `tipb::Executor(TypeIndexLookUp)`，携带句柄偏移。
- `detach_root_table_scan` 检查表侧是否为以叶子 `Scan` 结尾的严格一元链，并把叶子扫描从父节点摘下。
- `explain_info` 与 `memory_usage` 分别提供偏移说明和近似内存计量。

## 主要符号

- `pub struct PhysicalLocalIndexLookup`：可克隆、可调试、可比较的本地回表描述。其五个字段均公开；当前没有构造器负责联合校验两棵子树、schema 和偏移。
- `PhysicalLocalIndexLookup::index_handle_offsets_for_schema(index_schema, is_common_handle) -> Result<Vec<u32>, String>`：公共句柄返回空向量；整数句柄从尾到头寻找首个非负列 ID 或 `model::ExtraHandleID`，返回单元素偏移，否则报错。
- `PhysicalLocalIndexLookup::build_push_down_plan(self, table_filters, projection, next_plan_id) -> Result<PhysicalPlanNode, String>`：消费 `self`，改写表侧并返回简化计划树节点。当前函数体没有可失败分支，`Result` 为将来或对齐接口保留。
- `PhysicalLocalIndexLookup::explain_info(&self) -> String`：输出 `index handle offsets:[…]`。
- `PhysicalLocalIndexLookup::to_pb(&self, store_type) -> Result<tipb::Executor, String>`：仅接受 `kv::StoreType::TiKV`。
- `PhysicalLocalIndexLookup::memory_usage(&self) -> i64`：累计结构体静态大小、两棵子计划递归计量以及偏移元素字节数。
- `pub fn detach_root_table_scan(plan) -> Result<(PhysicalPlanNode, Option<Vec<usize>>), String>`：返回扫描节点和其父节点路径；内部函数 `locate` 只接受 `Scan` 叶子或单孩子链。

文件中没有模块级常量、trait、条件编译项或异步入口。

## 执行流程

整数句柄偏移计算从 schema 尾部逆向扫描。负的非 `ExtraHandleID` 列会被跳过，这覆盖分区表在句柄后追加 `ExtraPhysTblID` 等隐藏列的情形；找到合法句柄后把位置转成 `u32`。公共句柄直接返回空列表。此辅助函数不会被 `build_push_down_plan` 自动调用，调用方必须把所得偏移写入结构体。

`build_push_down_plan` 先调用 `table_plan.reset_ids(next_plan_id)`，以前序遍历只重编号表侧克隆/所有权树，索引侧 ID 保持不变。若有表过滤条件，则以原表侧 schema、stats 包一层 `Selection`；若提供投影（即使表达式列表为空），再用 lookup 自身的输出 schema 和当前表侧 stats 包一层 `Projection`。最后创建 `LocalIndexLookup` 节点，孩子顺序固定为 `[index_plan, table_plan]`，统计取最终表侧节点，所需属性置空。

`detach_root_table_scan` 先只读定位路径：叶子必须是 `PhysicalKind::Scan`，中间每层必须恰有一个孩子。根本身就是叶子扫描时返回它的克隆和 `None`，不修改传入值；存在父节点时，沿路径找到父节点并 `remove` 扫描孩子，返回扫描所有权和父路径。注意当前 `build_push_down_plan` 并未调用该拆分函数，也不会像 Go 实现那样把 lookup 插回原表侧包装链；两者仍是分离的骨架能力。

## 数据与状态

`PhysicalLocalIndexLookup` 拥有两棵计划树而非共享引用。`index_plan` 应产生句柄，`table_plan` 应负责回表；`schema` 是 lookup 输出列 ID，当前实现不会检查它是否等于表侧 schema。`keep_order` 保存顺序意图，但本文件的构建、解释和 protobuf 编码均未消费该字段。

`index_handle_offsets` 是进入索引侧输出的 `u32` 下标：公共句柄通常为空，整数句柄通常仅一个元素。类型本身允许任意长度和越界值，只有辅助计算函数提供局部约束，`to_pb` 会原样编码。

计划 ID 由调用者提供的 `&mut i64` 单调分配。表侧递归重编号、可选包装节点和最终 lookup 都从同一个计数器取号；索引侧保持原 ID。新建节点的 `required_properties` 均为空，stats 只克隆表侧数据，不重新估算。

## 依赖与调用关系

模块入口是 `physicalop/lib.rs` 的 `pub mod physical_indexlookup`。核心下游依赖为：`PhysicalPlanNode::reset_ids` 和 `memory_usage`、`PhysicalKind::{Scan,Selection,Projection,LocalIndexLookup}`、`model::ExtraHandleID`、`kv::StoreType`，以及 `tipb::{Executor,IndexLookUp,ExecType}`。`Cargo.toml` 将 `kv`、`model` 声明为工作区路径依赖，将 `tipb` 固定到 Git revision `07f0ea6b6bffa9d8ac100d81ee51dbbfe4dda3bf` 并启用 `protobuf-codec`。

RustCodeGraph 查询到本文件 10 个符号，但 `index_handle_offsets_for_schema`、`build_push_down_plan`、`to_pb`、`memory_usage` 和 `detach_root_table_scan` 均没有生产调用边。文本引用补充了图未覆盖的接线：`plan_clone_generated.rs` 为该类型实现 `CloneForPlanCache`，分别克隆 index/table 子计划；`CachePlan::LocalIndexLookup` 持有该类型；`cache_snapshot.rs::CachedLocalIndexLookup` 当前以整体 `clone` 捕获和恢复；`lib.rs` 的缓存契约表将其与 Go `PhysicalLocalIndexLookUp` 对应。

SQL 主链中的成熟近邻是另一类型 `PhysicalIndexLookUpReader`，不能与本地 lookup 混同。当前 `PhysicalLocalIndexLookup` 未实现 `base::PhysicalPlan`，也没有证据表明 `planbuilder.rs` 会构造它；`physical_utils_test.rs` 因此用已注册的 reader 代替它测试通用树展开。

## 错误处理与边界

- 整数句柄 schema 若全是负的非 `ExtraHandleID` 列，偏移计算返回 `cannot find handle column in index schema`；空 schema 同样报此错。公共句柄无论 schema 内容都成功返回空列表。
- `to_pb` 对非 TiKV store 返回含 store 调试值的错误；它不校验偏移是否合法，也不编码两棵子计划、schema 或 `keep_order`。
- `detach_root_table_scan` 拒绝非 Scan 叶子、Scan 自带孩子、以及任一中间节点孩子数不是 1 的树，统一返回 `table-side lookup plan has no PhysicalTableScan root`。这里的“PhysicalTableScan”实际由简化枚举 `PhysicalKind::Scan` 判定，不携带 Go 具体类型约束。
- `build_push_down_plan` 不验证 table plan 的形状、不调用 detach、不计算 handle offsets，也不检查 ID 溢出或 schema/投影一致性。其 `Result` 目前不会产生 `Err`。
- `memory_usage` 是近似值：子计划自行按 schema capacity 和孩子递归计量；lookup 自己的 `schema` 后备存储未额外计入，偏移按 `len` 而非 capacity 计入。它也没有 Go 的 nil receiver 语义。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部 I/O。所有状态由值所有权管理；`build_push_down_plan` 消费 lookup，避免原地共享修改，只有 `next_plan_id` 是调用期间独占的可变借用。`detach_root_table_scan` 独占借用树并在成功时转移叶子扫描所有权；失败发生在结构检查阶段，不会先做部分删除。

protobuf 对象完全在栈上的局部构造流程中创建后移交给调用者。计划缓存路径依赖派生 `Clone` 或 `CloneForPlanCache` 深复制两棵值语义子树，没有共享可变资源；当前类型因此没有额外清理或取消协议。

## 与 Go 版本的对应关系

直接对照文件是 `physical_indexlookup.go`。两边一致的局部语义包括：公共句柄不写 offset；整数句柄从尾部跳过特殊负列，选择最后一个非负列或 `ExtraHandleID`；仅 TiKV 可编码 `TypeIndexLookUp`；EXPLAIN 输出句柄偏移；表侧计划重编号而索引侧保持原 ID。

Rust 版本仍有重要结构差异。Go 类型嵌入 `PhysicalSchemaProducer`，通过 `Init` 注册为完整物理计划并从 table scan 设置 children/stats/schema；Rust 类型只持有简化 `PhysicalPlanNode`。Go `buildPushDownIndexLookUpPlan` 先克隆 table plan、调用 `detachRootTableScanPlan`，将 lookup 插入 Selection/Projection 等包装链的扫描位置；Rust `build_push_down_plan` 消费已有表计划，不克隆、不拆扫描，直接把整个表侧作为 lookup 第二个孩子。Go 还实现 `Clone`、nil 安全的 `MemoryUsage`，Rust 的缓存克隆在 `plan_clone_generated.rs` 中另行提供，计量公式也不完全等价。

Go 的 `PhysicalLocalIndexLookUp` 已可作为 `base.PhysicalPlan` 用于 `FlattenTreePushDownPlan` 测试；Rust 对应测试明确改用 `PhysicalIndexLookUpReader`，证明本地 lookup 尚未完整接线。因此本文只把已由 Rust 代码/测试证明的局部行为称为“支持”，不推断其已进入生产 SQL 执行链。

## 扩展指南

若补齐完整 Go 迁移，最可能修改 `PhysicalLocalIndexLookup`、`build_push_down_plan` 与 `detach_root_table_scan`：需要先决定是继续使用 `PhysicalPlanNode` 骨架，还是接入 `base::PhysicalPlan`/`PhysicalSchemaProducer`；随后忠实实现 table plan 克隆、扫描拆出、lookup 插回父包装链、stats/schema 继承及 `keep_order` 传播。不得仅让测试通过而省略这些 Go 语义。

新增句柄类型或隐藏列时，应集中更新 `index_handle_offsets_for_schema`，并在独立的 `physical_indexlookup_test.rs` 增加正常、空 schema、分区隐藏列、公共句柄和错误路径用例。改变 protobuf 时同步验证 TiKV 正例、TiFlash 拒绝以及 tipb 字段；改变树改写时至少覆盖裸 Scan、一元包装链、分叉、非 Scan 叶子、ID 顺序和原索引侧不重编号。

若字段布局或克隆规则变化，还必须同步 `plan_clone_generated.rs`、`cache_snapshot.rs`、`cache_snapshot_test.rs` 与 `lib.rs` 中的缓存契约。测试逻辑应继续放在独立 `*_test.rs` 文件，不嵌入生产源文件。性能风险主要是无谓深克隆和递归树遍历；兼容风险主要是句柄 offset、孩子顺序、计划 ID、schema/stats 与 Go 不一致。

## 验证依据

- 目标实现：`pkg/planner/core/operator/physicalop/physical_indexlookup.rs`，核对结构体、六个公开方法/函数与内部 `locate`。
- 模块和 crate：`pkg/planner/core/operator/physicalop/lib.rs`、`Cargo.toml`，核对公开模块、测试模块、缓存契约及 `kv`/`model`/`tipb` 依赖。
- 公共计划模型：`physical_common_plans.rs`，核对 `PhysicalKind`、`PhysicalPlanNode`、`Stats`、`reset_ids` 和递归内存计量。
- Go 对照：`physical_indexlookup.go`，核对构建、拆扫描、offset、PB、EXPLAIN、内存和克隆语义；`physical_utils_test.go` 与 Rust 的 `physical_utils_test.rs` 证明两侧主计划体系的当前接线差异。
- 独立 Rust 测试：`physical_indexlookup_test.rs` 覆盖解释文本、仅重编号表侧、非法 Scan 形状、分区/公共句柄和 TiKV/TiFlash；`cache_snapshot_test.rs` 覆盖缓存往返字段保持。
- 缓存接线：`plan_clone_generated.rs`、`cache_snapshot.rs`，核对深克隆变体与快照捕获/恢复。
- RustCodeGraph：索引状态为 11,467 文件、307,296 节点、1,848,419 边；查询识别 `PhysicalLocalIndexLookup`、`index_handle_offsets_for_schema`、`build_push_down_plan`、`explain_info`、`to_pb`、`memory_usage`、`detach_root_table_scan`/`locate`。对关键入口执行 callers/callees 后未发现生产调用者；其中泛型方法名的 callee 解析出现跨文件误匹配，故调用结论以直接引用和源码复核补强。
