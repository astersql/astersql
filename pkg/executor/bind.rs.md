# `pkg/executor/bind.rs`

## 文件定位

[`pkg/executor/bind.rs`](bind.rs) 位于 `astersql-executor` crate。`pkg/executor/Cargo.toml` 将该 crate 的根设为 `lib.rs`；`pkg/executor/lib.rs` 通过 `pub mod bind` 暴露本模块，并仅在测试构建中通过 `mod bind_test` 挂载独立测试文件 [`pkg/executor/bind_test.rs`](bind_test.rs)。文件承担 SQL Plan Binding 管理语句的执行逻辑，而不是普通查询算子的逐行计算：`SQLBindExec::Next` 总是先清空输出 chunk，再执行创建、删除、刷新、重载、集群重载或状态变更，成功时不产生结果行。

当前 Rust 生产接线需要谨慎理解。`pkg/executor/builder.rs::build` 能把 `Plan::SqlBind` 分派给 `buildSQLBindExec`，但后者只是调用 `build_leaf(ExecutorKind::SqlBind, plan)`；仓库搜索到的 `SQLBindBackend` 实现只有测试中的 `PanicBackend`，也没有发现生产代码直接构造本文件的 `SQLBindExec`。因此可以确认模块已纳入 crate 且内部逻辑可独立使用/测试，但不能据此声称这里的泛型执行器已经由生产后端实例化。Go 生产接线则明确位于 `pkg/executor/builder.go::buildSQLBindExec`。

## 核心职责

- `SQLBindExec::Next` 是统一分派入口，将 `SQLBindOpType` 的八类已知操作映射到对应方法，并为 `Unknown(i32)` 返回带原值的“不支持操作”错误。
- `createSQLBind` 把规划阶段的 `SQLBindOpDetail` 转为执行/存储需要的 `Binding`，统一把新 binding 状态设为 `"enabled"`，再按 `isGlobal` 选择会话或全局后端。
- `dropSQLBind` 与 `dropSQLBindByDigest` 分别处理单条详情和 digest 列表；全局删除还把后端返回的影响行数计入 statement context。
- `setBindingStatus` 与 `setBindingStatusByDigest` 共享 `set_binding_status`：前者从规范化 SQL 重新计算 digest，后者直接使用显式 digest；成功但没有记录发生变化时追加 warning。
- `flushBindings`、`reloadBindings` 和 `reloadClusterBindings` 分别执行增量本地加载、全量加载和向集群广播 `ADMIN RELOAD BINDINGS`。
- `SQLBindBackend<C>` 把会话状态、domain binding handle、digest 计算、缓存加载和集群广播隔离为可注入边界，使本文件只保留分派与状态协调逻辑。

## 主要符号

- `SQLBindOpType`：执行器本地的操作枚举。`Create`、`Drop`、`DropByDigest`、`Flush`、`Reload`、`ReloadCluster`、`SetStatus`、`SetStatusByDigest` 对应已知路径；`Unknown(i32)` 保留无法识别的操作码以便错误报告。
- `SQLBindOpDetail`：单次操作的输入 DTO。字段按不同操作选择性使用：创建读取规范化原 SQL、数据库、binding SQL、字符集、排序规则、来源及两个 digest；状态修改读取 `NewStatus`；按 digest 删除读取 `SQLDigest`。
- `Binding`：传给创建后端的持久化语义记录。它与 `SQLBindOpDetail` 基本同构，但用 `OriginalSQL` 和 `Status` 表达创建后的记录形态。
- `ResettableChunk`：`Next` 对输出缓冲的最小约束，只有 `reset`；这体现管理语句不返回数据行。
- `SQLBindBackend<C>`：所有外部副作用的适配 trait。关联类型 `Error` 和 `StatementContext` 避免本文件绑定具体会话实现；方法覆盖错误构造、digest 计算、会话/全局增删、影响行数、warning、statement context 保存恢复、缓存加载和广播。
- `SQLBindExec<B>`：持有后端 `BaseExecutor`、作用域标志 `isGlobal`、操作类型 `sqlBindOp`、详情列表 `details` 和远端来源标志 `isFromRemote`。这些字段目前为公开字段，调用者必须维持操作类型与详情字段之间的契约。
- `require_one_detail`：单详情操作的共同守卫，被 `dropSQLBind`、`setBindingStatus` 和 `setBindingStatusByDigest` 使用。
- `set_binding_status`：两种状态变更入口的共同尾部，集中处理“调用成功但无匹配 binding”的 warning 语义。

## 执行流程

1. 调用者以执行上下文 `&C` 和输出缓冲 `&mut Q` 调用 `SQLBindExec::Next`；入口首先执行 `Q::reset`。
2. `Next` 按 `sqlBindOp` 分派。未知操作立即通过 `SQLBindBackend::error` 构造错误，不触发其他后端副作用。
3. 创建路径先 `take_statement_context`，再把所有详情映射为状态为 `enabled` 的 `Binding` 列表。`isGlobal=true` 调用 `create_global_bindings`，否则调用 `create_session_bindings`。
4. 创建后，无论后端正常返回还是 panic，代码都会读取当前 context 的 `SET_VAR` 恢复项，把它们合并到保存的 context，再调用 `restore_statement_context`。正常返回传播后端的 `Result`；panic 则在恢复后用 `resume_unwind` 原样继续展开。
5. 单 digest 删除先通过 `require_one_detail` 取 `SQLDigest`；多 digest 删除遍历全部详情并拒绝任何空 digest。会话路径直接传播删除结果；全局路径先接收 `(affected_rows, result)`，无论 `result` 成败都先记录影响行数，再返回原结果。
6. 按 SQL 文本设置状态时，先对 `NormdOrigSQL` 调用 `normalize_digest_for_binding`；按 digest 设置时直接使用详情中的 `SQLDigest`。两者最终调用 `set_global_binding_status`。仅当后端结果为 `Ok` 且 `changed=false` 时追加固定 warning。
7. `flushBindings` 调用 `load_bindings(false, false)`；`reloadBindings` 调用 `load_bindings(true, isFromRemote)`；集群重载调用 `broadcast(context, "ADMIN RELOAD BINDINGS")`。

## 数据与状态

执行器自身没有缓存或持久化容器；可变状态主要在 `BaseExecutor` 后端中。`details` 是一次语句携带的操作集合，创建和按 digest 删除允许多条，`dropSQLBind` 与两种状态修改明确要求恰好一条。`isGlobal` 只影响创建和删除的目标；状态变更方法始终调用名为 `set_global_binding_status` 的后端能力，与 Go 版本使用 domain 全局 binding handle 一致。`isFromRemote` 只参与全量 `reloadBindings`，不会改变增量 flush。

创建路径最重要的瞬时状态是 `StatementContext`：旧 context 被取出，后端创建期间可能因内部 explain 或 `SET_VAR` hint 改写当前 context，结束时把当前 `SET_VAR` 恢复表合并回旧 context 后再恢复旧对象。`Binding` 列表和 digest 列表均为调用栈内拥有的 `Vec`；传给后端的是切片，生命周期不越过同步调用。

## 依赖与调用关系

上游结构证据如下：`pkg/executor/lib.rs` 注册 `bind` 模块和独立测试；`pkg/executor/builder.rs::build` 的 `Plan::SqlBind` 分支调用 `buildSQLBindExec`，该函数当前通过 `ExecutorDependencies::build_executor` 的通用叶子路径请求 `ExecutorKind::SqlBind`。RustCodeGraph 能定位这些符号，但对本文件方法执行 `callers`/`callees` 未返回边；配合仓库文本搜索，只能确认测试直接调用 `createSQLBind`，不能确认通用 builder 依赖最终实例化本文件类型。

下游全部经 `SQLBindBackend` 表达：会话/全局 binding 创建与删除、全局状态更新、缓存加载、digest 规范化、statement context 管理及广播。这样本文件没有直接依赖 `bindinfo`、`domain`、parser 或 chunk 的具体 Rust 类型；`pkg/executor/Cargo.toml` 虽声明了对应 workspace crate（如 `astersql-bindinfo`、`astersql-domain`、`astersql-parser`、`astersql-util-chunk`），本文件自身仅使用标准库 panic API，实际集成依赖由后端实现承担。

规划侧存在另一组 `pkg/planner/core/common_plans.rs::{SQLBindOpType, SQLBindOpDetail, SQLBindPlan}`，其枚举和字段集合与本文件同名类型并不一致；当前 builder 只接收抽象 `PlanData` 并交给依赖层。因此新增真实生产接线时必须显式定义两组类型的转换，不能假定它们可直接互换。

## 错误处理与边界

- `Unknown(value)` 返回 `unsupported SQL bind operation: {value}`；不会静默忽略未知规划值。
- 单详情路径对零条或多条统一报错。按 digest 批量删除允许空详情列表（会向后端传空切片），但任何一项的 `SQLDigest` 为空都会在执行副作用前失败。
- `setBindingStatusByDigest` 没有在本层拒绝空 digest；是否接受由后端决定。创建也不在本层校验 SQL、字符集、排序规则或 digest，验证职责留给规划器/后端。
- 全局删除先记录后端返回的 `affected_rows`，再传播同一次调用的错误；因此后端在错误情况下返回的行数也会被记录，这是当前代码的明确顺序。
- 状态更新只有 `result.is_ok() && !changed` 才追加 warning；后端错误优先传播且不会额外 warning。
- 创建路径用 `catch_unwind(AssertUnwindSafe(...))` 模拟 Go `defer` 的清理保障。它不把 panic 转为普通错误，而是在恢复 statement context 后继续 panic；进程采用 abort panic 策略时该保证不成立。
- 后端的 trait 方法多为同步接口，本文件没有重试、超时、幂等去重或补偿逻辑；这些属于具体后端或上层职责。

## 并发与资源生命周期

`Next` 和所有操作方法都要求 `&mut self`，同一个执行器实例不能被安全 Rust 同时可变调用；文件内没有线程、异步任务、锁或 channel。并发一致性、缓存锁和全局 binding 存储事务均隐藏在 `SQLBindBackend` 实现之后，本文件没有证据说明其策略。

资源生命周期以一次同步方法调用为界。输出 chunk 在每次 `Next` 开始时重置；临时 `Vec<Binding>`/`Vec<String>` 在后端返回后释放；statement context 在创建前移出并在返回或 unwind 前恢复。`reloadClusterBindings` 只把借用的 context 和静态 SQL 字符串交给同步 `broadcast`，本层不持有网络连接或后台广播句柄。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/executor/bind.go`](bind.go)。Rust 的 `Next` 分派、单详情限制、空 digest 拒绝、会话/全局分流、全局删除影响行数、无匹配状态修改 warning、flush/reload 参数和集群广播 SQL 均保持 Go 语义。`createSQLBind` 也保留了 Go 注释所述审计语义：内部 explain 不应把外层 create binding 记录成 explain，并且必须带回 `SetVarHintRestore`。

实现形式有三项显著差异。第一，Go 直接依赖 `BaseExecutor`、session binding handle、domain binding handle、parser 和 `chunk.Chunk`；Rust 把这些能力抽象到 `SQLBindBackend`/`ResettableChunk`。第二，Go 的 `defer` 天然覆盖正常返回和 panic；Rust 使用 `catch_unwind`、恢复 context、再 `resume_unwind` 明确复刻该行为，`pkg/executor/bind_test.rs::create_binding_restores_statement_context_when_backend_panics` 专门验证这一点。第三，Go builder 明确把 `SQLBindPlan` 字段复制进 `SQLBindExec`，而 Rust builder 当前只走通用 `ExecutorKind::SqlBind` 工厂；Rust 生产后端与规划类型转换尚未由本文件及直接调用证据证实。

Go 的端到端行为还由 `pkg/bindinfo/tests/bind_test.go`、`pkg/bindinfo/tests/cross_db_binding_test.go`、`pkg/bindinfo/session_handle_test.go` 等测试覆盖创建、删除、session/global 隔离、digest 删除和 reload。它们能证明 Go 基线语义，但不能替代 Rust 本文件的生产接线测试。

## 扩展指南

- 新增操作类型时，应同步修改 `SQLBindOpType`、`SQLBindExec::Next` 分派、必要的详情约束和 `SQLBindBackend` 能力；同时核对规划侧 `pkg/planner/core/common_plans.rs` 的类型及 builder 转换，避免出现仅本地枚举可表达、规划层无法产生的操作。
- 增加或改变创建字段时，应同步更新 `SQLBindOpDetail -> Binding` 映射，并对照 `pkg/executor/bind.go::createSQLBind` 和 `bindinfo.Binding` 的语义。状态默认值、SQL/plan digest、字符集与排序规则属于兼容性敏感字段。
- 修改 statement context 处理时，必须保留正常错误和 panic 两条路径的恢复保证，并把 Rust 回归测试放在独立的 `pkg/executor/bind_test.rs`，不要内嵌到生产文件。建议补充正常成功、普通 `Err`、带 `SET_VAR` 恢复项及全局/会话两种路径。
- 修改删除逻辑时，应覆盖零/一/多详情、空 digest、全局影响行数及后端错误顺序；修改状态逻辑时，应覆盖 digest 计算、显式 digest、`changed=false` warning 和后端错误不告警。
- 若完成生产接线，应为真实 `SQLBindBackend` 实现增加集成测试，并核实 builder 的 `ExecutorKind::SqlBind` 最终产物确实调用本文件 `Next`。主要正确性风险是规划 DTO 转换遗漏；兼容性风险是 Go 错误/warning/影响行数语义漂移；性能风险主要在批量详情复制、缓存全量 reload 和集群广播，当前本层没有并行或限流。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `pkg/executor/bind.rs`；`node --file pkg/executor/bind.rs` 读取了 304 行完整源码；`query SQLBindExec/createSQLBind/dropSQLBind/reloadClusterBindings` 定位 Rust/Go 对照符号；对关键 Rust 函数执行 `callers`/`callees` 未得到调用边，因此再用文本搜索核对直接接线。
- Rust 源与接线：`pkg/executor/bind.rs`（全部类型、trait 和流程）、`pkg/executor/lib.rs`（`pub mod bind` 与独立 `bind_test`）、`pkg/executor/builder.rs`（`Plan::SqlBind -> buildSQLBindExec -> build_leaf(ExecutorKind::SqlBind, ...)`）、`pkg/planner/core/common_plans.rs`（规划侧同名但不等价的 DTO）。
- crate 边界：`pkg/executor/Cargo.toml`（crate 名 `astersql-executor`、根 `lib.rs`、workspace 依赖与 `nextgen` feature；本文件没有条件编译项）。
- Rust 测试：`pkg/executor/bind_test.rs` 的 `PanicBackend` 是搜索到的唯一 `SQLBindBackend` 实现；`create_binding_restores_statement_context_when_backend_panics` 验证 panic 后 context 已恢复。
- Go 对照与测试：`pkg/executor/bind.go`、`pkg/executor/builder.go::buildSQLBindExec`；相关端到端测试见 `pkg/bindinfo/tests/bind_test.go`、`pkg/bindinfo/tests/cross_db_binding_test.go`、`pkg/bindinfo/session_handle_test.go`，`pkg/executor/explainfor_test.go` 也覆盖 create/reload 后 binding 生效。
- 本任务是纯文档分析，按计划不运行 Cargo 或运行时测试；验收使用任务指定的 11 章节结构命令，并人工复核文档对定位、流程、扩展点及未验证生产接线的表述。
