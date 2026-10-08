# `pkg/planner/core/plan_cache.rs`

## 文件定位

本文对应真实源文件 [`plan_cache.rs`](./plan_cache.rs)。它属于 `astersql-planner-core` crate 的会话级计划缓存辅助层。`pkg/planner/core/lib.rs` 以私有模块 `mod plan_cache` 装配它，再通过 `pub use plan_cache::*` 将其中三个函数作为 crate API 导出。它不负责缓存键生成、计划构建、LRU/实例缓存存储或计划重建；这些职责分别位于相邻的 `plan_cache_utils.rs`、`plan_cache_lru.rs`、`plan_cache_instance.rs` 和 `plan_cache_rebuild.rs`。

当前文件只有三个公开函数，没有模块级常量、自定义类型、trait、`impl` 或条件编译项。Rust 全仓引用搜索显示，三个函数目前只由独立测试直接调用；尚未发现 Rust 生产调用点。因此它们是已实现、已导出且有测试覆盖的迁移边界，而不能据此声称 Rust 运行主链已经完整接入。Go 对照实现则已接入计划缓存预处理、hint-only 策略和 Point Get executor 复用链路。

## 核心职责

1. `SetParameterValuesIntoSCtx` 在执行缓存语句前校验实参数量，并按位置把已求值的 `Datum` 写入 `PlanCacheStmt.Params`。
2. `containUsePlanCacheHintInPreparedSQLOrBinding` 合并 prepared SQL 自身的 `USE_PLAN_CACHE` 标记与已匹配 binding 的 hint 判定。
3. `IsSafeToReusePointGetExecutor` 将 Point Get executor 复用的四项安全条件收敛为一个纯布尔判定：自动提交、事务外、非 stale read、schema 版本相同。

这三个函数都不访问缓存容器，也不自行取得 session、binding 或 infoschema 状态；调用者必须先把运行时事实转换成参数。该设计令函数易于独立测试，但也意味着输入真实性由上游负责。

## 主要符号

- `pub fn SetParameterValuesIntoSCtx(stmt: &mut PlanCacheStmt, params: Vec<Datum>) -> Result<(), String>`：先比较 `stmt.Params.len()` 与 `params.len()`；不相等时返回 `"wrong parameter count"`，相等时逐项设置 `PlanCacheParamMarker.datum = Some(datum)` 和 `in_execute = true`。`PlanCacheParamMarker` 定义在 `plan_cache_utils.rs`，除执行值和状态外还保存源码 `offset` 与排序后的 `order`。
- `pub fn containUsePlanCacheHintInPreparedSQLOrBinding(stmt: &PlanCacheStmt, binding_hint: bool, matched: bool) -> bool`：表达式为 `stmt.HasUsePlanCacheHint || (binding_hint && matched)`。SQL 自带 hint 时无需 binding；否则 binding 必须既已匹配又被上游确认含对应 hint。
- `pub fn IsSafeToReusePointGetExecutor(autocommit: bool, in_txn: bool, stale: bool, stmt_version: i64, schema_version: i64) -> bool`：仅当 `autocommit && !in_txn && !stale && stmt_version == schema_version` 时返回 `true`。
- `PlanCacheStmt`：定义在 `plan_cache_utils.rs` 的泛型缓存语句元数据。本文函数只读取或修改其中的 `Params`、`HasUsePlanCacheHint`；Point Get 判定所需 schema 版本以独立参数传入，而不是直接读取 `PlanCacheStmt.SchemaVersion`。
- `Datum`：由 crate 根再导出的规划器值表示；本文件取得值的所有权并移动进各参数 marker，不克隆参数值。

## 执行流程

参数写入流程如下：调用者提供可变 `PlanCacheStmt` 和已经求值的 `Vec<Datum>`；函数先做完整长度检查；只有长度一致才进入 `zip` 循环；每个值按向量顺序移入对应 marker，并把该 marker 标为执行态；全部写入后返回 `Ok(())`。由于校验发生在第一次写入之前，参数数量错误不会造成部分 marker 被修改。

hint 判定没有副作用。它先利用 `||` 检查语句自身标记；只有该标记为假时，才要求 `binding_hint` 与 `matched` 同时为真。这里的两个 binding 布尔量应由上游在完成 binding 匹配及 hint 内容检查后提供。

Point Get 复用判定同样是无副作用的合取检查。任何一个运行状态不满足——关闭自动提交、已经处于事务、正在 stale read，或 prepared statement 记录的 schema 版本与当前版本不同——都会拒绝复用。函数只判断安全前提，不确认计划本身是否为 Point Get；函数名中的这一前置事实需要调用者保证。

## 数据与状态

本文件唯一的写操作发生在 `PlanCacheStmt.Params`。每个 `PlanCacheParamMarker` 的 `datum` 从原值（通常为 `None`）替换为本次执行值，`in_execute` 被置为 `true`；`offset`、`order` 及 `PlanCacheStmt` 的其余字段保持不变。函数不会清理旧值；成功调用会以新值覆盖全部对应 marker。

`containUsePlanCacheHintInPreparedSQLOrBinding` 只读取 `HasUsePlanCacheHint`。`IsSafeToReusePointGetExecutor` 只消费标量参数。两者都不保存跨调用状态。本文件也不更新 session 的参数列表、statement context、缓存命中状态或指标。

关键不变量是：成功的参数写入必然覆盖恰好全部 marker；参数数量不等时一个 marker 也不应变化。独立测试 `plan_cache_test.rs` 同时验证了有序写入和错误路径的无部分修改性质。

## 依赖与调用关系

直接 Rust 依赖只有 crate 根导出的 `Datum` 与 `PlanCacheStmt`。后者进一步把本文件连接到 `plan_cache_utils.rs` 中的 `PlanCacheParamMarker` 和 prepared statement 元数据。`pkg/planner/core/Cargo.toml` 指定 crate 名为 `astersql-planner-core`、入口为 `lib.rs`、`autotests = false`；因此根模块通过显式 `#[cfg(test)] mod plan_cache_test` 纳入同目录独立测试，而不是让 Cargo 自动发现内嵌测试。

已验证的 Rust 上游是：

- `pkg/planner/core/plan_cache_test.rs` 调用 `SetParameterValuesIntoSCtx`，覆盖成功与数量错误。
- `pkg/planner/core/casetest/plancache/plan_cache_test.rs` 调用三个函数，覆盖参数状态、hint 组合、事务/stale/schema 版本分支。
- `pkg/planner/core/tests/pointget/point_get_plan_test.rs` 调用 Point Get 安全判定，结合 Point Get 场景验证自动提交与事务分支。

全仓 Rust 精确引用搜索未发现上述函数的生产调用者。相对地，Go 的 `SetParameterValuesIntoSCtx` 被 `pkg/planner/optimize.go` 的非 prepared 规划和 `plan_cache.go` 的缓存预处理调用；Go hint 辅助被 `GetPlanFromPlanCache` 的 hint-only 策略调用；Go Point Get 判定被 `pkg/executor/compiler.go` 调用。上述 Go 调用边用于说明移植目标，不代表 Rust 已具备相同接线。

## 错误处理与边界

`SetParameterValuesIntoSCtx` 的唯一显式错误是参数数量不一致，错误类型为无结构的 `String`。长度检查先于修改，因此该错误具备原子失败性质。写入循环本身没有可失败操作，也不会验证 `Datum` 类型与 marker 的 SQL 类型兼容性。

hint 辅助把 binding 的存在性和 hint 解析结果压缩成调用方提供的 `binding_hint`；它无法自行防止调用者错误地把不存在或未解析的 binding 标为含 hint。布尔运算优先级使表达式等价于“SQL 自带 hint，或 binding 含 hint 且确实匹配”。

Point Get 辅助使用严格 schema 版本相等，保守拒绝任何版本变化；它不处理版本取得失败，因为版本已作为 `i64` 输入。它也不检查 executor 是否存在、计划是否为 Point Get、锁模式或其他 session 状态，这些均属于调用前置条件或尚未移植的上层逻辑。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络资源。参数写入要求 `&mut PlanCacheStmt`，Rust 借用规则保证该次修改期间不存在并发读写同一语句对象；两个判定函数只使用共享借用或按值标量。

`Vec<Datum>` 被函数取得所有权，元素随后逐个移动到 marker；调用结束后输入向量不再存在。函数不延长外部资源生命周期，也不触碰相邻 LRU/实例缓存中的 `Mutex`。Point Get 判定中的 `in_txn` 只是状态快照，不开始、提交或回滚事务；若上游在判定后改变 session 状态，是否仍可复用须由更高层同步协议保证，本文件没有提供这种保证。

## 与 Go 版本的对应关系

Go 对照文件为 `pkg/planner/core/plan_cache.go`，三个同名/同用途函数的核心意图一致，但 Rust 当前是聚焦后的边界实现：

- Go `SetParameterValuesIntoSCtx` 接收 plan context、non-prepared 标记、marker 和待求值表达式；它逐个求值、处理 `getvar` binary literal、更新 session `PlanCacheParams` 并记录缓存种类。Rust 接收已经求值的 `Datum`，只更新 `PlanCacheStmt.Params`，且额外在自身入口做长度检查。Go 的数量检查位于调用它的 `planCachePreprocess`，不是该函数内部。
- Go hint 辅助接收实际 `Binding` 指针，检查 binding 已匹配、非空、hint 非空且包含 `HintUsePlanCache`。Rust 用 `binding_hint` 和 `matched` 两个布尔量表达后半段结果；实际 binding 解析和空值检查必须在上游完成。
- Go `IsSafeToReusePointGetExecutor` 从 session 与 infoschema 读取 stale-read、自动提交/事务组合和 schema 版本。Rust 把这些环境查询外提为五个值，并显式区分 `autocommit` 与 `in_txn`。

因此，Rust 保留了三个判定/写入核心，但尚不能替代 Go 函数的全部 session 副作用、表达式求值、binding 解析和生产链路接线。扩展时应以 Go 行为作为语义对照，同时保持 Rust 逻辑与测试位于独立文件。

## 扩展指南

- 若接入 prepared/non-prepared 执行主链，应在上游完成表达式求值、binary literal 语义、session 参数列表更新后调用 `SetParameterValuesIntoSCtx`，或扩充明确的运行时抽象；不要让本函数悄悄遗漏 Go 的副作用。
- 若接入 hint-only 策略，应由 binding 模块产生可信的“已匹配且含 `USE_PLAN_CACHE`”事实，再调用现有纯判定；若改为传递 binding 对象，需要同步空 binding、空 hint、错误 hint 与 SQL 自带 hint 的组合测试。
- 若增加 Point Get 安全条件，应修改 `IsSafeToReusePointGetExecutor` 的参数/逻辑，并同步 `pkg/planner/core/casetest/plancache/plan_cache_test.rs` 与 `pkg/planner/core/tests/pointget/point_get_plan_test.rs`。新增 session 状态必须注意判定时刻与 executor 复用时刻之间的竞态。
- 参数写入行为变化应优先扩展独立的 `pkg/planner/core/plan_cache_test.rs`，尤其覆盖零参数、重复执行覆盖旧值、类型边界与失败不修改；Rust 单元测试不要内嵌回生产文件。
- 对外签名已经由 `lib.rs` 再导出，改名或改变错误类型会影响 crate 消费者。迁移到结构化错误时应评估与 planner error 体系的兼容性；加入克隆或额外分配时应评估高频 EXECUTE 路径性能。

## 验证依据

- 源文件：`pkg/planner/core/plan_cache.rs`，核对三个公开函数的全部实现及无其他符号/条件编译项。
- crate 边界：`pkg/planner/core/Cargo.toml` 与 `pkg/planner/core/lib.rs`，核对 crate 名、显式测试装配、私有模块和公开再导出。
- 数据定义：`pkg/planner/core/plan_cache_utils.rs` 中 `PlanCacheParamMarker`、`PlanCacheStmt`，核对被读写字段及其状态含义。
- Rust 测试：`pkg/planner/core/plan_cache_test.rs`、`pkg/planner/core/casetest/plancache/plan_cache_test.rs`、`pkg/planner/core/tests/pointget/point_get_plan_test.rs`。
- Go 对照与调用者：`pkg/planner/core/plan_cache.go`、`pkg/planner/optimize.go`、`pkg/executor/compiler.go`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点；精确 `query` 将三个 Rust 函数定位到本文件，并定位同名 Go 实现。`files --filter pkg/planner/core/plan_cache` 未返回目标文件，批量 callers/callees 查询超时，因此调用关系又以全仓精确符号引用搜索复核；当前未发现 Rust 生产调用点。
- 结构验收使用任务规定的命令，要求本文恰好出现十一个固定二级标题；本任务为纯文档分析，未运行 Cargo 或代码测试。
