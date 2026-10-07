# `pkg/expression/sessionexpr/sessionctx.rs` 逻辑说明

## 文件定位

`sessionctx.rs` 是 `astersql-expression-sessionexpr` crate 的核心实现文件。它位于会话子系统与表达式子系统之间，把一个满足本文件 `SessionContext` trait 的活动会话适配成 `exprctx::BuildContext`、`exprctx::ExprContext`、`exprctx::EvalContext` 及两种静态转换接口。模块入口 `pkg/expression/sessionexpr/lib.rs` 以私有模块加载本文件并 `pub use sessionctx::*`，所以本文件的公开类型和函数构成 crate 的主要 API。

crate 边界由 `pkg/expression/sessionexpr/Cargo.toml` 定义，直接依赖表达式上下文/可选属性/静态上下文、会话变量、InfoSchema、权限、类型、错误上下文、排序规则和数学工具等相邻 crate。根 `Cargo.toml` 通过 `facade_expression_sessionexpr` 注册该 crate，`pkg/lib.rs` 再将它纳入 facade；`pkg/session/Cargo.toml` 与 `pkg/executor/Cargo.toml` 也声明了依赖。不过，对全仓库 Rust 源码的精确搜索只找到 facade 再导出和本 crate 测试引用，没有找到生产代码直接构造本文件 `NewExprContext` 或 `NewEvalContext`。因此当前可确认的事实是“适配层实现与测试已存在”，不能据此断言 Rust 会话主链已经实际接线。

## 核心职责

本文件承担四类职责：

1. 用 `SessionContext` 把表达式所需的会话能力压缩成窄接口，避免 expression 叶包直接拥有完整 session、KV、InfoSchema 或 executor 实现。
2. 用 `ExprContext<C>` 转发表达式构建期配置，包括字符集/排序规则、计划缓存、随机数、列 ID、窗口精度、`group_concat` 上限、连接 ID 和只读用户变量；对应实现见 `NewExprContext` 及三个 `exprctx::*Context` trait impl。
3. 用 `EvalContext<C>` 转发语句求值期状态，并在 `NewEvalContext` 中一次装配完整可选属性集合：当前用户、会话变量、嵌入运行时会话、InfoSchema、KV Store、受限 SQL executor、序列、 advisory lock、DDL owner 和权限检查器。源码注释称“九类”，但实际注册包含 `SessionContextPropProvider` 在内共十次 `set_optional_prop`；完整性的权威判定是 `OptionalEvalPropKeySet::IsFull()`，而不是注释中的数量。
4. 用 `getStmtTimestamp`/`resolve_statement_timestamp` 实现语句时间优先级：非零 stale TSO 优先，其次 `timestamp` 系统变量，值为 `0` 时使用调用方提供的当前时间。

## 主要符号

- `SessionContext`：公开 trait，也是整个适配器的输入边界。三个关联类型分别约束 `InfoSchema`、KV Store 和受限 SQL executor；方法覆盖身份/角色、可选属性资源、构建配置、语句上下文、告警、系统变量、参数和用户变量。它还继承 `expropt::AdvisoryLockContext + Send + Sync + 'static`，使同一个会话可被并发安全地捕获到 `Arc` 和属性闭包中。
- `PrivilegeManager`：公开的最小权限接口，只保留静态权限和动态权限两种检查。对任何 `privilege::Manager + Send + Sync` 提供 blanket impl，便于复用完整权限管理器。
- `ContextPrivilegeChecker<C>`：私有适配器，实现 `expropt::PrivilegeChecker`。每次检查从会话取得当前角色；若 `privilege_manager()` 为 `None`，按 Go 行为直接放行。
- `ExprContext<C>` / `NewExprContext`：公开构建上下文和构造函数。结构体持有会话、会话提供的共享 RNG、共享计划缓存跟踪器，以及一个公开嵌入的 `EvalContext<C>`。
- `EvalContext<C>` / `NewEvalContext`：公开求值上下文和构造函数。结构体持有会话、`OptionalEvalPropProviders` 与共享告警处理器。
- `set_optional_prop`：私有注册辅助函数；按 provider 描述符中的 key 检查重复，再加入集合。构造末尾再次断言属性集合完整。
- `resolve_statement_timestamp`：公开、与具体会话解耦的时间解析函数，也是时间边界测试的直接入口。
- `getStmtTimestamp`：公开的会话入口；无会话时返回 UTC 当前时间，有会话时取得会话时区、stale TSO 和已按语句缓存处理的 `timestamp` 文本，再调用解析函数。
- `EmbeddingSession<C>`：私有新类型，只把 embedding runtime、取消标记和上下文值转发成 `expropt::SessionContext`，避免把本文件较宽的 `SessionContext` 直接暴露给推理属性。

本文件没有模块级常量、枚举、宏或条件编译项；测试条件编译位于 `lib.rs`，生产实现本身不依赖 feature gate。

## 执行流程

构建上下文的入口是 `NewExprContext(sctx)`：先从会话取得 RNG 与计划缓存跟踪器的 `Arc`，再以同一会话调用 `NewEvalContext`，最后保留原会话。`GetEvalCtx` 返回内嵌求值上下文的 trait 引用，因此构建期与求值期共享同一会话状态。`BuildContext`、`ExprContext` 和 `StaticConvertibleExprContext` 的 trait 方法大多委托到同名固有方法；`IntoStatic` 则调用 `exprstatic::MakeExprContextStatic` 复制/冻结表达式需要的状态。

求值上下文的入口是 `NewEvalContext(sctx)`：

1. 先取得告警处理器，并创建空的 `OptionalEvalPropProviders`。
2. 依次注册当前用户/角色与会话变量。
3. 注册 `EmbeddingSession`，再注册按 `is_domain` 选择会话快照或最新域级快照的 InfoSchema provider。
4. 注册惰性取得 Store、受限 SQL executor 和序列 operator 的 provider。
5. 注册直接共享会话的 advisory-lock provider，以及惰性读取 DDL owner 状态的 provider。
6. 注册每次生成 `ContextPrivilegeChecker` 的权限 provider。
7. 以 `IsFull()` 断言所有可选属性都已覆盖；缺项或 key 重复会立即 panic，而不是返回一个部分可用上下文。

普通求值通过 `EvalContext` 的 trait impl读取 SQL mode、类型/错误上下文、时区、数据库、数据包上限、日志脱敏模式、周格式、小数除法精度、参数和用户变量；告警相关方法全部转发到构造时捕获的同一 `WarnHandler`。`CurrentTime` 调用 `getStmtTimestamp(Some(sctx))`：非零 stale TSO 右移 18 位得到物理毫秒；无有效 stale TSO 时解析 `timestamp` 为有限 `f64`；`timestamp == 0` 返回本次调用捕获的 `now`，否则拆成秒和纳秒并按会话时区返回。

## 数据与状态

`ExprContext` 和 `EvalContext` 都不拥有会话数据副本，而是通过 `Arc<C>` 共享会话。`ExprContext` 额外缓存会话返回的 RNG 与计划缓存跟踪器 `Arc`，保证静态转换能够取得同一共享对象；其他字段按方法调用实时向会话查询。计划缓存的 `SetSkipPlanCache` 会改变共享 tracker，列 ID 分配、`group_concat` 测试覆盖和 advisory lock 也会改变会话侧状态。

`EvalContext` 在构造时固定告警处理器和 provider 集合，但多数 provider 内的闭包持有会话 `Arc`，调用时读取最新的身份、角色、InfoSchema、Store、executor、DDL owner 或权限管理器。`SessionVarsPropProvider` 则在构造时取得 `Arc<SessionVars>`。参数读取调用 `parameter_values()`；当前接口返回完整 `Vec<Datum>`，所以单参数访问也可能复制整个参数集合，扩展时需留意性能。

时间状态不在本文件内缓存。稳定语句时间依赖 `SessionContext::stale_tso()` 与 `timestamp_system_var()` 的实现遵守语句级缓存约定；本 trait 的文档明确要求后者返回已经应用语句缓存的 `timestamp` 变量。Rust 测试夹具据此验证重复调用得到相同时间。

## 依赖与调用关系

上游边界如下：`lib.rs` 公开再导出本文件；根 facade 在 `pkg/lib.rs` 公开再导出 crate。`pkg/session` 与 `pkg/executor` 的 Cargo manifest 声明依赖该 crate，但 RustCodeGraph 对 `NewExprContext`、`NewEvalContext`、`getStmtTimestamp` 和 `resolve_statement_timestamp` 没有返回调用边，精确源码搜索也未发现测试之外的直接调用。因此生产上游目前只能确认到“可见且可依赖”，无法确认“已调用”。直接、可验证的调用者是 `sessionctx_test.rs` 与 `migration_aster_unit_test.rs`；`NewExprContext` 内部调用 `NewEvalContext`，`EvalContext::CurrentTime` 调用 `getStmtTimestamp`，后者调用 `resolve_statement_timestamp`。

下游依赖由 `Cargo.toml` 和源码共同确认：

- `exprctx` 定义构建/求值、参数、可选属性和静态转换 trait；本文件实现这些接口。
- `expropt` 定义 provider/reader、序列、SQL executor、advisory lock、embedding session 与权限检查接口；`NewEvalContext` 负责组装它们。
- `exprstatic` 消费静态转换接口；两个 `IntoStatic` 方法将活动上下文转换成静态表达式上下文。
- `variable`、`vardef`、`types`、`errctx`、`contextutil` 提供会话变量、系统变量名/默认值、Datum/类型上下文、错误上下文、告警与计划缓存。
- `infoschema`、`privilege`、`auth`、`mysql` 定义元数据、权限、身份和 SQL mode/权限类型边界。
- `chrono`/`chrono-tz` 完成时间戳与时区转换，`collate` 提供全局新排序规则开关，`mathutil` 提供 MySQL RNG，`log` 只用于 stale TSO 读取失败后的错误日志。

## 错误处理与边界

构造阶段采用强不变量：重复可选属性或最终属性集合不完整会 `assert!` panic。这表示 `NewEvalContext` 只产生完整上下文，不提供可恢复的部分配置路径。

运行阶段的可恢复错误包括：序列 operator 构造直接传播 `anyhow::Result`；SQL executor provider 的当前适配总是返回 `Ok`；参数索引越界返回 `exprctx::ErrParamIndexExceedParamCounts`；`timestamp` 获取、浮点解析、非有限值、TSO/Unix 时间越界都转换为 `SharedError`。stale TSO provider 自身报错是特例：函数记录错误日志，然后继续尝试 `timestamp` 系统变量，这与 Go 实现一致。

权限管理器不存在时静态和动态权限检查均返回 `true`。这是兼容 Go 测试夹具的明确策略，不应误解成调用者已经做过权限校验；接入真实会话时必须保证需要权限控制的路径能返回实际 manager。

`GetDefaultWeekFormatMode` 对空字符串回退到 MySQL 默认值 `"0"`。`GetBlockEncryptionMode` 对缺失系统变量回退到 `vardef::DefBlockEncryptionMode`。`IsInNullRejectCheck` 与 `IsConstantPropagateCheck` 当前固定为 `false`，新增优化阶段状态时不能继续把它们当常量。

## 并发与资源生命周期

`SessionContext` 要求 `Send + Sync + 'static`，所有跨 provider 保存的会话引用均为 `Arc<C>`；InfoSchema、Store、SQL executor、RNG、计划缓存和告警处理器也通过 `Arc` 共享。provider 闭包随 `EvalContext` 生存，并通过自己的 `Arc` 延长会话生命周期。`EmbeddingSession`、权限 checker 和各惰性 provider 不创建后台任务或线程，也不持有裸引用。

并发正确性由下游共享对象和 `SessionContext` 实现负责：本文件不会为列 ID、计划缓存、告警、角色、DDL owner、序列或 advisory lock 额外加锁。`active_roles()` 每次返回 `Vec<Arc<RoleIdentity>>`，权限适配器再克隆为值列表后调用 manager，避免把临时会话借用带入权限接口，但有分配与克隆成本。告警处理器在构造时固定，因此会话若在上下文存活期替换 handler，既有 `EvalContext` 不会自动切换。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/expression/sessionexpr/sessionctx.go`，对应测试是 `sessionctx_test.go`。Rust 保留了 Go 的两个上下文、两个构造入口、完整 optional-property 组装、会话字段转发、权限默认放行、stale TSO 优先级、`timestamp` 小数秒和静态转换语义。

主要结构差异是：Go 直接持有 `sessionctx.Context` 接口并用嵌入指针暴露 `EvalContext`；Rust 以泛型 `C: SessionContext` 和 `Arc<C>` 表达同一关系。Go 的序列 operator 在本文件内通过 InfoSchema 查表后包装 `SequenceTable`，Rust 则把构造责任下推到 `SessionContext::sequence_operator`。Go 从全局绑定表取得权限 manager，Rust 从会话 trait 方法取得，并用窄 `PrivilegeManager` 适配。Rust 还显式暴露 embedding session provider，并为静态转换提供 RNG/tracker/warn-handler 的 `Arc` 获取方法。

时间解析存在实现层面的强化：Rust 拒绝非有限浮点数，检查 TSO 与 Unix 时间范围，并正确归一化负数小数秒；Go 使用 `types.StrToFloat`、`math.Modf` 和 `time.Unix`。这些差异已有 Rust 的 `resolve_statement_timestamp` 独立测试覆盖部分边界，但任何进一步变更仍应同时核对 Go 的兼容语义。

Rust `sessionctx_test.rs` 的五个测试与 Go 五个测试一一对应：基础字段/告警、当前时间、权限、可选属性和构建上下文。`migration_aster_unit_test.rs` 还补充了 block encryption 默认值、静态转换、参数越界、所有 provider、序列、stale TSO 失败回退及错误时间文本等证据。

## 扩展指南

- 新增会话侧表达式能力时，先判断它属于构建期固定接口、求值期基本字段还是 optional property。分别修改 `SessionContext` 与 `ExprContext` trait impl、`EvalContext` trait impl，或 `NewEvalContext` provider 装配；不要让 expression crate 反向依赖完整 session 实现。
- 新增 optional property 必须同步 `expropt` 的 key/集合、provider/reader，并在 `NewEvalContext` 中恰好注册一次；更新 `sessionctx_test.rs` 的“集合完整且 key/描述符一致”断言，以及 `migration_aster_unit_test.rs` 的真实读取路径。还应修正当前“九类”注释与实际十个注册项的数量漂移。
- 调整时间行为应优先修改可纯测的 `resolve_statement_timestamp`，并同步 Rust 的优先级、错误、负数/小数/越界测试及 Go `TestSessionEvalContextCurrentTime` 语义。语句级稳定性必须由 `timestamp_system_var`/stale TSO 的会话实现保证。
- 调整权限时同步 `PrivilegeManager`、`ContextPrivilegeChecker`、直接检查方法和 optional provider；重点覆盖 manager 缺失默认放行、活动角色传递、静态/动态权限及 `grantable`。
- 调整共享状态时审查 `Arc` 快照还是实时查询的选择。RNG、plan-cache tracker、warn handler 和 session vars 当前在构造期捕获；角色、InfoSchema、Store、executor、DDL owner 与权限 manager 多数在调用期查询。改变时序可能影响并发可见性和语句一致性。
- 把本 crate 接入生产 Rust 会话主链时，最可能新增的是某个真实会话类型的 `SessionContext` impl，以及创建 `NewExprContext/NewEvalContext` 的调用点。应增加独立集成测试证明真实 session、planner/executor 与本适配层连通；当前单元夹具只能证明适配逻辑，不能证明生产接线。
- 性能风险集中在 `readonly_user_vars()` 每次返回集合、`parameter_values()` 每次返回 `Vec`、权限角色值克隆以及大量 provider 动态分派。若改成借用或缓存，需要保持 trait object 生命周期、静态转换快照和并发安全不变量。

## 验证依据

- 生产源码：`pkg/expression/sessionexpr/sessionctx.rs`；模块入口：`pkg/expression/sessionexpr/lib.rs`；crate 声明：`pkg/expression/sessionexpr/Cargo.toml`。
- Go 对照：`pkg/expression/sessionexpr/sessionctx.go`；独立测试：`pkg/expression/sessionexpr/sessionctx_test.rs`、`pkg/expression/sessionexpr/sessionctx_test.go`、`pkg/expression/sessionexpr/migration_aster_unit_test.rs`。
- RustCodeGraph：`status` 确认索引包含 11,467 个文件，其中 Rust 7,032 个；`files --filter pkg/expression/sessionexpr` 确认目标源、模块入口、Go 文件及测试均已索引；`query` 定位 `sessionctx.rs::NewExprContext`（第 209 行）、`NewEvalContext`（第 375 行）、`resolve_statement_timestamp`（第 670 行）和本文件 `SessionContext` trait（第 39 行）。对这些符号执行 `callers/callees` 未返回边，因此没有把缺失图边解释成生产调用关系。
- 图未覆盖后的精确搜索：全仓库 Rust 搜索表明本文件构造函数的直接调用仅在 `sessionctx_test.rs` 和 `migration_aster_unit_test.rs`，内部边为 `NewExprContext -> NewEvalContext`、`EvalContext::CurrentTime -> getStmtTimestamp -> resolve_statement_timestamp`；Cargo 搜索确认根 facade、session 与 executor manifest 的依赖声明。
- 测试证据（仅阅读，按任务要求未运行 Cargo）：Rust/Go 五组对应测试覆盖基本字段与告警、语句时间、权限、optional properties 和 build context；迁移测试额外覆盖静态化、序列、参数越界、时间优先级、非法文本与 stale TSO 错误回退。
- 人工复核结论：本文件存在的原因是隔离完整会话与 expression 叶包；主要运行方式是构造完整 provider 集合并通过 trait 转发状态；安全扩展要求保持 provider 完整性、时间/权限兼容语义、`Arc` 生命周期以及测试文件独立。生产接线未由当前 Rust 调用证据确认，文档已明确保留这一限制。
