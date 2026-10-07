# `pkg/planner/core/encode.rs`

## 文件定位

本文件属于 `astersql-planner-core` crate。crate 根 `pkg/planner/core/lib.rs` 以私有模块 `mod encode` 装入它，再用 `pub use encode::*` 重导出公开符号，因此调用方从 `astersql_planner_core` 直接使用编码和归一化 API。`pkg/planner/core/Cargo.toml` 指定库入口为 `lib.rs`、关闭自动测试发现，并将 Go 对照包标为 `pkg/planner/core`；本文件直接使用的外部依赖只有 `sha2 = "0.10"`。

它位于物理计划生成之后、计划展示或摘要消费之前：输入是 `PlanNode`，或已经由 `FlattenPhysicalPlan` 转成的 `FlatPhysicalPlan`；输出是制表符分隔的文本计划，或者归一化文本及其 SHA-256 摘要。已确认的生产消费点是 `pkg/executor/adapter_slow_log.rs::session_plan_digest`，其调用 `NormalizeFlatPlan`，把摘要转换成 parser digest，缓存进 statement context，供慢日志记录计划摘要。

## 核心职责

- `EncodePlan` / `EncodeFlatPlan` 生成包含层级、节点 ID、角色标签、算子类型、估算行数、执行位置和算子信息的逐行文本。`EncodePlan` 是“先扁平化、再编码”的便利入口。
- `NormalizePlan` / `NormalizeFlatPlan` 隐藏节点 ID、估算行数和详细 `operator_info`，只保留计划结构相关字段，并计算稳定的 SHA-256 `PlanDigest`。
- `getSelectPlan` 和 `FlatPhysicalPlan::GetSelectPlan` 配合实现写语句语义：Update、Delete、Insert 的外层 DML 节点不进入 SELECT 计划摘要。
- `store_name` 将本 crate 的 `StoreType::{Root, TiKV, TiFlash}` 映射为稳定的小写输出。

这里的“稳定”只针对当前 Rust 模型中被隐藏的易变字段；不能据此推断它已经与 Go 的压缩编码字节协议完全兼容，差异见“与 Go 版本的对应关系”。

## 主要符号

- `pub struct PlanDigest([u8; 32])`：不可从 crate 外直接构造的 32 字节摘要值；派生 `Clone`、`Debug`、`Eq` 和 `PartialEq`。
- `PlanDigest::Bytes(&self) -> &[u8; 32]`：借用原始摘要字节，不分配。
- `PlanDigest::String(&self) -> String`：把 32 字节逐字节格式化为 64 个小写十六进制字符。
- `digest(&str) -> PlanDigest`：以 UTF-8 字节为输入调用 `Sha256::digest`，也是空计划摘要的统一生成点。
- `store_name(StoreType) -> &'static str`：穷举三个存储位置；没有默认分支，因此新增 `StoreType` 变体会迫使此处同步处理。
- `encode_operator(&FlatOperator, normalized, level_offset, &mut String)`：本文件的核心行编码器。普通模式写真实 ID、两位小数估算行数和 `operator_info`；归一化模式把 ID、行数写成 `?`，并以 `PlanKind::name()` 代替详细信息。两种模式都写入经偏移修正的层级、`OperatorLabel`、算子名和 store 名。
- `encodeFlatPlanTree`：保持切片顺序遍历 `FlatPlanTree`，逐项调用 `encode_operator`。
- `pub fn EncodeFlatPlan(&FlatPhysicalPlan) -> String`：按 `Main`、`CTE`、`ScalarSubQ` 顺序编码整个扁平计划；空主树或 `InExecute` 为真时返回空串。
- `pub fn EncodePlan(Option<&PlanNode>) -> String`：空输入返回空串，否则调用 `FlattenPhysicalPlan(plan, false)`，成功后转交 `EncodeFlatPlan`。
- `pub fn NormalizeFlatPlan(&FlatPhysicalPlan) -> (String, PlanDigest)`：只处理 `GetSelectPlan()` 返回的 SELECT 主树；首节点缺失或不是物理节点时返回空文本及 `SHA-256(空串)`。
- `pub fn NormalizePlan(Option<&PlanNode>) -> (String, PlanDigest)`：先用 `getSelectPlan` 去掉 DML 包装，再扁平化并归一化；任一入口无效时返回空文本及空串摘要。
- `pub fn getSelectPlan(&PlanNode) -> Option<&PlanNode>`：Update、Delete、Insert 返回第一个孩子；其他物理节点返回自身；非物理节点或无孩子的 DML 返回 `None`。

文件中没有 trait、模块级常量、条件编译项或可变静态状态。

## 执行流程

普通树入口的流程为：`EncodePlan` 检查 `Option` → `FlattenPhysicalPlan(Some(plan), false)` 构造主扁平树 → `EncodeFlatPlan` 检查 `Main` 和 `InExecute` → 预估每个算子约 80 字节容量 → 依次编码 `Main`、`CTE`、`ScalarSubQ`。每个输出行的字段顺序固定为 `level<TAB>id+label<TAB>kind<TAB>rows<TAB>store<TAB>info<LF>`。层级使用 `saturating_sub`，即错误或异常偏移不会产生无符号下溢，而会收敛到 0。

归一化树入口的流程为：`NormalizePlan` 检查输入 → `getSelectPlan` 提取物理 SELECT 节点 → `FlattenPhysicalPlan(..., false)` → `NormalizeFlatPlan`。后者调用 `FlatPhysicalPlan::GetSelectPlan` 得到切片和主树偏移，以该偏移把 DML 子树的首层重新基准化为 0；逐节点编码后对完整归一化字符串计算 SHA-256。

归一化输出不遍历 `CTE` 和 `ScalarSubQ`，也不因 `InExecute` 改变结果。`pkg/planner/core/encode_test.rs::normalize_flat_plan_only_uses_the_select_tree_like_go` 通过在取得基线后同时设置 `InExecute`、CTE 和标量子查询，验证结果保持不变。

## 数据与状态

`FlatPhysicalPlan` 在 `pkg/planner/core/flat_plan.rs` 中保存 `Main`、`CTE`、`ScalarSubQ` 三棵扁平树及 `InExecute` 等标志；`FlatOperator` 持有原始 `PlanNode`、角色标签、store 和层级。本文件只借用这些结构，不修改计划节点或扁平计划。

非归一化字段来源如下：ID 和估算行数来自 `operator.Origin`，角色来自 `operator.Label`，算子名来自 `operator.Origin.kind.name()`，store 来自 `operator.StoreType`，详细信息来自 `operator.Origin.operator_info`。归一化时，ID 与行数被替换为 `?`，详细信息被替换为算子名；因此节点 ID、估算行数、访问范围等 `operator_info` 变化不会改变摘要，而算子类型、标签、层级、store 或节点顺序变化会改变归一化文本，通常也会改变摘要。

所有中间状态都在栈上或函数局部拥有的 `String` 中。`PlanDigest` 固定为 32 字节；`Bytes` 返回借用，`String` 才产生新的堆分配字符串。空输入使用 `digest("")`，不是全零摘要。

## 依赖与调用关系

向下调用链由 RustCodeGraph 和源码共同确认：`EncodePlan → FlattenPhysicalPlan → EncodeFlatPlan → encodeFlatPlanTree → encode_operator`；`NormalizePlan → getSelectPlan → FlattenPhysicalPlan → NormalizeFlatPlan → encode_operator/digest`。`NormalizeFlatPlan` 还依赖 `FlatPhysicalPlan::GetSelectPlan`；`encode_operator` 依赖 `PlanKind::name`、`OperatorLabel::to_string` 和 `store_name`。

crate 边界上，`pkg/planner/core/lib.rs` 重导出本文件 API，也重导出 `flat_plan.rs` 和 `common_plans.rs` 中的输入类型。Cargo 中的 `sha2` 是本文件摘要算法的直接外部依赖，其余相关类型均来自当前 crate。

已核实的上游包括：

- `pkg/executor/adapter_slow_log.rs::session_plan_digest`：调用 `NormalizeFlatPlan`，缓存规范化文本和摘要，服务慢日志。
- `pkg/planner/core/plan_test.rs`：直接比较 `EncodePlan`/`EncodeFlatPlan` 与 `NormalizePlan`/`NormalizeFlatPlan` 两条路径。
- `pkg/sessionctx/variable/tests/slowlog/slow_log_test.rs`：以 `NormalizeFlatPlan` 的摘要字符串校验慢日志 `PlanDigest`。

RustCodeGraph 对这些符号的查询能定位 Rust/Go 同名定义，文件节点显示 `encode.rs` 被 22 个文件使用；但当前 `callers/callees` 在同名跨语言符号上产生歧义，因此跨文件调用者采用精确 Rust 搜索复核，未把噪声边纳入结论。

## 错误处理与边界

该 API 不返回 `Result`，预期边界通过空输出表达：`EncodePlan(None)`、扁平化失败、空 `Main` 或 `InExecute` 均得到空编码；`NormalizePlan(None)`、DML 无 SELECT 子节点、非物理计划或扁平化失败均得到 `(空串, SHA-256(空串))`；`NormalizeFlatPlan` 的 SELECT 切片为空或首节点非物理时同样返回该哨兵结果。

`getSelectPlan` 对 DML 只取第一个孩子，这是当前 `PlanNode` 简化模型的显式约定。普通非 DML 节点只有 `IsPhysical()` 为真才被接受；目前 `PlanNode::IsPhysical` 仅排除逻辑 `DataSource` 和 `Join`。调用方若新增逻辑节点，必须同时检查该判定，不能假定所有新增变体天然是物理节点。

格式化和 `String` 扩容可能因进程级内存耗尽而终止，但没有可恢复的业务错误。SHA-256 crate API 在这里没有错误返回。层级偏移以 `saturating_sub` 防止下溢。摘要没有防碰撞业务兜底，不应被当作认证或授权凭据。

## 并发与资源生命周期

本文件没有锁、通道、异步任务、线程局部变量、全局池或事务。所有函数只读借用输入，并新建、填充、返回自有的 `String`/`PlanDigest`，因而不同线程调用之间没有共享的可变状态。`Sha256::digest` 是一次性计算，hasher 不跨调用复用。

资源生命周期由 Rust 所有权自然限定：输出缓冲在函数结束时随返回值转移或释放；摘要字节内嵌在 `PlanDigest` 中；`Bytes` 的借用不能长于摘要对象。容量预分配只是一项性能优化，不改变输出内容。

## 与 Go 版本的对应关系

Go 对照位于 `pkg/planner/core/encode.go`。两边共同保留的意图包括：空主树和 Execute 路径不收集普通编码；普通路径覆盖 Main、CTE 和标量子查询；归一化只取 SELECT 主树；写计划跳过 DML 包装；归一化摘要使用 SHA-256。Go `pkg/planner/core/plan_test.go` 还以旧树路径和新扁平路径相互对照，并测试相同/不同 SQL 的摘要关系。

当前 Rust 并非 Go 编码器的字节级复刻：Go 调用 `plancodec.EncodePlanNode` 后压缩输出，包含运行时执行、内存和磁盘信息，并对反转驱动侧、CTE 定义等做专门处理；Rust 输出可读的 TSV 文本，不压缩，也没有 Go 的 `sync.Pool`、failpoint、运行时统计编码或 `plancodec` 协议。Go 归一化使用 `TP(...)`、task type 编码和 `ExplainNormalizedInfo()`，Rust 则使用 `PlanKind::name()`、简单 store 名和同一算子名作为 info。Rust 的 `getSelectPlan` 不处理 Go 的 `Explain` 包装，DML 子计划也以第一个 child 表示，而非 Go 的具体物理类型字段。

因此，现有 Rust 测试证明的是当前 Rust 模型内部的两条路径一致和若干 Go 语义意图对齐；它们没有证明 Rust 输出可由 Go `plancodec.DecodePlan` 解码，也没有证明跨语言 digest 完全相同。迁移时必须保留这一区分。

## 扩展指南

- 新增 `StoreType` 时同步更新 `store_name`，并在独立测试文件覆盖普通和归一化输出；否则穷举匹配会直接阻止编译。
- 改动输出字段、分隔符、精度、角色标签或归一化掩码时，应同时修改 `encode_operator`，并扩展 `pkg/planner/core/plan_test.rs` 的编码一致性与摘要稳定性测试。摘要文本本身是兼容面，字段变化会令已有 plan digest 整体变化。
- 新增写计划包装类型或改变子计划布局时，同步修改 `getSelectPlan` 和 `FlatPhysicalPlan::GetSelectPlan`，并在 `pkg/planner/core/encode_test.rs` 增加对应 DML/边界测试；不要把测试嵌入生产文件。
- 若要继续追平 Go，应按可验证的小步引入 `plancodec` 兼容编码、运行时字段、CTE/驱动侧次序和 Explain 包装语义，并以 Go `pkg/planner/core/plan_test.go` 的解码及摘要用例作为对照。不能仅改摘要算法或表面格式就宣称协议兼容。
- 若归一化未来纳入 CTE 或标量子查询，必须评估慢日志、statement context 缓存和 plan digest 稳定性，并更新当前明确验证“忽略二者”的 `normalize_flat_plan_only_uses_the_select_tree_like_go`。
- 性能修改应保留单次线性遍历和容量预分配；当前 `PlanDigest::String` 每字节执行一次格式化，若优化需用等价的固定宽度小写十六进制测试锁定行为。

## 验证依据

- 目标源码：`pkg/planner/core/encode.rs`，核对了全部 161 行及所有结构、函数、impl；文件无条件编译项。
- 类型与扁平化：`pkg/planner/core/flat_plan.rs` 的 `FlatPlanTree`、`FlatPhysicalPlan`、`GetSelectPlan`、`FlatOperator`、`FlattenPhysicalPlan`；`pkg/planner/core/common_plans.rs` 的 `StoreType`、`PlanKind::name`、`PlanNode`、`IsPhysical`。
- crate 边界：`pkg/planner/core/Cargo.toml` 和 `pkg/planner/core/lib.rs`；后者声明并重导出 `encode`，同时以 `#[cfg(test)] mod encode_test`、`mod plan_test` 装载独立测试。
- Rust 测试：`pkg/planner/core/encode_test.rs` 验证 DML 包装和仅 SELECT 主树归一化；`pkg/planner/core/plan_test.rs` 验证树/扁平编码一致、摘要忽略 ID/行数/详情、不同算子摘要不同及空输入边界；`pkg/sessionctx/variable/tests/slowlog/slow_log_test.rs` 验证摘要进入慢日志字段。
- Go 对照：`pkg/planner/core/encode.go` 与 `pkg/planner/core/plan_test.go`，用于确认原始压缩协议、归一化字段、对象池、DML/Explain 处理及旧/新路径一致性测试意图。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点、1,848,419 条边；`node --file pkg/planner/core/encode.rs` 返回完整源码及 22 个使用文件；`query EncodePlan`、`query NormalizeFlatPlan`、`query NormalizePlan` 定位了 Rust 与 Go 同名定义；图的精确跨语言 callers/callees 有歧义，故上游边再以 `rg` 精确核验。
- 本任务是纯文档分析，依计划未运行 Cargo。结构验收使用任务指定命令，要求目标文件存在且恰有 11 个固定二级标题。
