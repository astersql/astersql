# `pkg/executor/prepared.rs`

## 文件定位

本文件属于 `astersql-executor` crate；crate 根 `pkg/executor/lib.rs` 通过 `pub mod prepared` 公开该模块，独立测试则以 `#[path = "prepared_test.rs"] mod prepared_test` 接入。它抽象了 SQL 预处理语句生命周期中的三个阶段：`PREPARE`、`EXECUTE` 和 `DEALLOCATE`，对应 Go 实现 `pkg/executor/prepared.go`。

当前接线状态需要特别区分：仓库内 Rust 生产代码没有实现 `PrepareBackend`、`ExecuteBuilder` 或 `DeallocateBackend`，也没有直接构造这些 Rust 执行器；除模块导出外，直接使用只出现在 `pkg/executor/prepared_test.rs`。因此它是已实现并有单元测试的迁移边界，而不是已经替代 Go 生产链路的执行器。Go 生产链仍由 `pkg/executor/builder.go`、`pkg/executor/adapter.go` 和 `pkg/session/session.go` 接线。

## 核心职责

- `PrepareExec::Next` 编排一次预处理：防重试重复登记、解析 SQL、限制为单语句、重置语句上下文、生成计划缓存语句与计划、注册 TopSQL、计算结果字段，最后分配 ID 并登记。
- `ExecuteExec::Build` 从已保存的计划构建真正执行器，并在会话优先级未显式指定时计算是否应降级调度。`ExecuteExec::Next` 本身不执行计划，只返回成功。
- `DeallocateExec::Next` 按名称查找并校验预处理对象，移除名称映射，按配置清理计划缓存，再移除 ID 对应的语句。
- 三个 backend/builder trait 将会话、解析器、计划器、缓存和错误类型隔离在文件之外，使本文件只表达执行顺序和状态转换。

## 主要符号

- `GeneratedPreparedStatement<S, P>`：`generate_plan_cache_statement` 的返回载体，包含可登记语句 `statement`、用于字段/执行器构建的 `plan` 和 `parameter_count`。
- `PrepareBackend`：包含 PREPARE 所需的全部外部能力。其关联类型刻画上下文、AST、计划缓存语句、计划、结果字段和错误；方法覆盖解析、告警修整、上下文重置、计划生成、TopSQL、结果字段和会话登记。
- `PrepareExec<B>`：持有 backend、可选名称、SQL 文本、语句 ID、参数数、结果字段、已生成语句和 `need_reset`。`NewPrepareExec` 初始化为空名称、零 ID/参数、空字段、无语句且 `need_reset = true`。
- `ExecuteBuilder<P>`：从 `P` 构建真正执行器，并提供当前优先级是否为 `NoPriority` 及计划是否需要降级的判断。
- `ExecuteExec<P, E, V, S>`：保存名称、`USING` 变量、可选实际执行器、原语句、计划和降级标记。本文件只消费 `plan` 与 `statement_executor`；其他字段保留 Go 数据模型的迁移接口。
- `DeallocateBackend`：封装名称/ID 查找、对象类型校验的错误、缓存键生成与删除、名称映射和语句登记删除。
- `DeallocateExec<B>`：保存 backend 与待释放的语句名称。

所有公开函数沿用 Go 风格名称，文件级 `#![allow(non_snake_case)]` 明确允许 `NewPrepareExec`、`Next` 和 `Build`。

## 执行流程

`NewPrepareExec` 只建立初始状态。随后 `PrepareExec::Next` 按以下顺序执行：

1. 若 `id != 0` 且 backend 仍能找到该 ID，立即成功返回，保证重试幂等。
2. 记录解析前告警数，以 `need_reset` 作为解析协议上下文参数调用 `parse_sql`。
3. 解析失败时，非 restricted SQL 追加 statement error；`need_reset` 为真时从旧计数位置修整告警；最终通过 `syntax_error` 转换并返回错误。
4. 解析结果必须恰好一条，否则返回 `prepare_multiple_statements_error`；`need_reset` 为真时再重置该语句的执行上下文。
5. 调用 `generate_plan_cache_statement`。TopSQL profiling 开启时登记语句；之后无条件重置计划标识符。只有计划会返回结果时才生成 `fields`。
6. ID 为零时分配新 ID；名称非空时先绑定名称到 ID。再写入参数数和生成语句，最后调用 `add_prepared_statement`。

`ExecuteExec::Build` 先调用 `builder.build(&plan)`；成功结果写入 `statement_executor`。仅当 `builder.no_priority()` 为真时，才用 `need_lower_priority(&plan)` 更新 `lower_priority`。`Next` 是空操作，因为真实执行必须交给刚构建的执行器。

`DeallocateExec::Next` 先按名查 ID，再按 ID 取得有效语句；两步都成功后立即删除名称映射。如果计划缓存开启，则生成缓存键；未配置保留关闭语句缓存时删除缓存条目。最后移除 ID 对应的预处理语句。

## 数据与状态

`PrepareExec` 的关键状态转换是 `id: 0 -> 新 ID`、`statement: None -> Some(...)`，以及同步写入 `parameter_count` 和可能的 `fields`。名称映射只在 `name` 非空时建立，支持 SQL 文本 `PREPARE name FROM ...`；协议层无名 prepare 可只依赖 ID。`need_reset` 区分协议入口与已经重置过上下文的文本 PREPARE 入口，但本构造函数默认设置为真。

状态写入并非事务性回滚：名称绑定、参数数和 `statement` 都发生在最终 `add_prepared_statement` 之前。若登记失败，这些字段仍保留；`prepared_test.rs::prepare_retains_generated_statement_when_registration_fails` 明确锁定该行为。

`ExecuteExec` 在构建前以 `statement_executor: None` 表示尚无可运行执行器；builder 失败时 `?` 提前返回，因此仍为 `None`。优先级判断只在构建成功后发生。

`DeallocateExec` 的删除顺序同样可观察：对象校验完成后先删名称，再生成缓存键。因此缓存键生成失败会留下“名称已删、语句登记仍在”的部分状态；Rust 测试对此有断言。缓存关闭时不生成缓存键，但仍必须先取得有效的 prepared statement。

## 依赖与调用关系

Rust 侧的直接结构关系为：`pkg/executor/lib.rs` 导出 `prepared`，并在测试配置下挂入 `pkg/executor/prepared_test.rs`。本文件自身只使用标准库的 `std::convert::Infallible`，其会话、计划、缓存等依赖全部经 trait 关联类型和方法注入，因此 `pkg/executor/Cargo.toml` 没有为该文件增加专属第三方依赖。Cargo 元数据把该 crate 标为 `astersql-executor`，并声明对应 Go package 为 `pkg/executor`。

RustCodeGraph 可定位 `GeneratedPreparedStatement`、三个 trait、三个执行器以及对应方法，但精确 callers/callees 查询没有返回可用的生产 Rust 调用边；全仓 Rust 符号搜索也只发现 `prepared_test.rs` 的直接引用。这与“尚未接入生产 Rust 主链”的结论一致。

Go 对照链路是：

- 协议入口 `pkg/session/session.go` 的 `PrepareStmt` 调用 `executor.NewPrepareExec` 并执行其 `Next`。
- SQL 计划入口 `pkg/executor/builder.go::buildPrepare/buildExecute/buildDeallocate` 构造对应 Go 执行器。
- `pkg/executor/adapter.go::ExecStmt.buildExecutor` 识别 `*ExecuteExec`，调用 `Build`，应用低优先级标记，然后用 `stmtExec` 替换门面执行器。

这些是 Go 生产证据，不能视为 Rust 类型已经接线。

## 错误处理与边界

- PREPARE 的解析错误会按 restricted SQL 条件决定是否记录到 statement context，并包装为语法错误；多语句、上下文重置失败、计划生成失败和最终登记失败均直接返回 backend 错误。
- `statements.len() == 1` 后的 `unwrap` 由紧邻的长度检查保证，不会在正常实现契约下处理空值或多值。
- `statement.as_ref().unwrap()` 之前刚写入 `Some(generated.statement)`，不变量由同一顺序保证。
- `ExecuteExec::Build` 原样传播 builder 错误且不留下半成品执行器；`Next` 的错误类型是 `Infallible`，表示该门面没有自身运行时失败路径。
- DEALLOCATE 区分“名称不存在”和“ID 下对象无效”两类错误。缓存键生成失败发生在名称删除之后、语句删除之前；缓存条目删除和语句删除接口本身不返回错误。
- 泛型 trait 没有强制 backend 的实现具有事务性或线程安全性；调用方必须遵守上述有序副作用契约。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道或显式事务，也没有 `unsafe`。所有修改方法都要求 `&mut self` 或 `&mut backend`，单次调用期间由 Rust 可变借用保证同一执行器实例不会被并发修改；跨线程能力取决于具体泛型类型是否实现 `Send`/`Sync`，本文件未施加这些边界。

资源生命周期以 backend 管理的会话登记和计划缓存为主：PREPARE 生成并登记语句，EXECUTE 持有并替换实际执行器，DEALLOCATE 删除名称、可选缓存条目和语句登记。`ExecuteExec` 没有在本文件实现 `Drop` 或 close；实际执行器的打开、执行、关闭必须由未来生产接线负责。DEALLOCATE 的 `keep_plan_cache_on_close` 允许名称和会话登记消失时仍保留计划缓存内容。

## 与 Go 版本的对应关系

Rust `PrepareExec::Next` 的步骤顺序基本对应 `pkg/executor/prepared.go::(*PrepareExec).Next`：重试幂等、选择解析参数、保留/修整告警、拒绝多语句、重置 statement context、调用 `GeneratePlanCacheStmtWithAST`、TopSQL 登记、清理 PlanID/PlanColumnID、生成结果字段、分配 ID、绑定名称并登记。Rust 将具体 TiDB 类型和函数拆到 `PrepareBackend`，未直接表现 Go 的 charset/collation 参数数组、SQLParser fallback 或扩展列哈希表字段；这些责任必须由 backend 实现承担。

Rust `ExecuteExec::Build` 对应 Go 的 `Build`：先构建真实执行器，构建失败即返回；只有 `NoPriority` 时计算降级。差异是 Go 会记录 warning log 并用 `errors.Trace` 包装 builder 错误，Rust trait 当前只原样传播错误；Rust `Next` 和 Go `Next` 都是空操作。

Rust `DeallocateExec::Next` 保留 Go 的关键顺序：名称查 ID、断言对象是 `PlanCacheStmt`、删除名称、可选构造并删除缓存键、移除语句。Rust 测试特别确认即使缓存关闭也不能跳过对象有效性检查，以及缓存键错误发生时名称已经删除。

Go 版本实现 `exec.Executor` 并连接真实 session/planner/cache 类型；Rust 版本尚无这些 trait 的生产实现。因此语义对应不等于迁移已完成。

## 扩展指南

- 接入 Rust 生产链时，应在会话/执行器边界实现三个 trait，而不是把具体 session、parser 或 plan-cache 类型重新塞回本文件；同时在 builder/adapter 等价路径中完成构造和 `ExecuteExec` 解包。
- 扩展 PREPARE 行为时优先修改 `PrepareBackend` 能力和 `PrepareExec::Next` 的明确步骤，并保持 Go 的副作用顺序。尤其要覆盖解析告警、restricted SQL、多语句、`need_reset`、无结果计划、TopSQL 和最终登记失败。
- 改变 `ExecuteExec::Build` 时，应保持“先成功构建，再安装执行器，再判断优先级”；如需日志或错误上下文，应在 trait 契约中明确，而不是吞掉错误。
- 改变 DEALLOCATE 时必须审视部分失败状态以及 `IgnorePreparedCacheCloseStmt` 对应语义；若要做原子化清理，那会偏离当前 Go 行为，需要兼容性说明和两端同步。
- Rust 测试应继续放在独立的 `pkg/executor/prepared_test.rs`，不要内嵌到生产文件。新增行为至少增加 trait mock 的成功、失败与副作用顺序断言；Go 兼容行为应同步核对 `pkg/executor/prepared_test.go` 和 `pkg/executor/test/seqtest/prepared_test.go`。
- 性能风险集中在重复解析/计划生成、结果字段物化及缓存键构造；兼容风险集中在错误类型、告警裁剪、ID/名称映射时序和缓存关闭策略。

## 验证依据

- 源文件：`pkg/executor/prepared.rs`，逐项核对全部公开结构、trait、构造函数及三个 impl；文件无条件编译分支、常量、静态变量或 `unsafe`。
- crate 边界：`pkg/executor/Cargo.toml` 的 package/lib/porting 元数据；`pkg/executor/lib.rs` 的 `pub mod prepared` 与独立测试模块声明。
- Rust 独立测试：`pkg/executor/prepared_test.rs`，覆盖 EXECUTE 构建成功/失败及降优先级、DEALLOCATE 的无效对象与缓存键失败顺序、PREPARE 登记失败后的状态保留。
- Go 实现与生产接线：`pkg/executor/prepared.go`、`pkg/executor/builder.go::buildPrepare/buildExecute/buildDeallocate`、`pkg/executor/adapter.go::ExecStmt.buildExecutor`、`pkg/session/session.go::PrepareStmt`。
- Go 行为测试：`pkg/executor/prepared_test.go` 与 `pkg/executor/test/seqtest/prepared_test.go`，后者包含多语句错误、参数与结果字段、EXECUTE 以及 DEALLOCATE/计划缓存场景。
- RustCodeGraph：`status` 显示索引包含 `pkg/executor/prepared.rs`；`query PrepareExec`、`query DeallocateExec`、`query NewPrepareExec` 和文件 `node` 核对符号及源码。精确 callers/callees 未给出可用的生产 Rust 边，随后用全仓 Rust 符号搜索确认直接引用仅见独立测试。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以任务指定命令验证恰有十一个固定二级章节，并人工复核当前接线状态与 Go/Rust 差异没有被写成未证实的支持结论。
