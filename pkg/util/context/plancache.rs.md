# `pkg/util/context/plancache.rs`

## 文件定位

`plancache.rs` 属于 `astersql-util-context` crate，crate 入口 `pkg/util/context/lib.rs` 通过 `pub mod plancache` 公开该模块。它不存放执行计划本身，而是提供“当前规划过程是否仍可使用计划缓存”的线程安全状态以及 range 构建回退时的告警协调。主要持有者是 `pkg/sessionctx/stmtctx/stmtctx.rs` 的 `StatementContext::PlanCacheTracker`；range 构建上下文则在 `pkg/util/ranger/context/context.rs` 中借用 tracker 和 fallback handler。

`pkg/util/context/Cargo.toml` 将该目录定义为独立 crate，并用 `[package.metadata.porting].go-package = "pkg/util/context"` 记录 Go 来源。本文件的直接 crate 内依赖是 `crate::errors` 和 `crate::warn::WarnAppender`；前者是 crate 入口重导出的 `astersql-errors`，后者是警告追加抽象。

## 核心职责

- `PlanCacheTracker` 以一个原子锁定的状态单元跟踪缓存启用标志、缓存类型、不合格原因、强制缓存标志和非预处理语句的告警策略。
- 当规划器或表达式/ranger 发现高风险优化时，`SetSkipPlanCache` 在普通模式关闭缓存并记录原因；在强制模式仅告警，不关闭缓存。
- `Save`/`Restore` 把五个可变字段作为整体快照，供嵌套或候选逻辑计划构建后恢复原状态。
- `RangeFallbackHandler` 把“range 超出容量后禁用计划缓存”与“同一 handler 只报一次容量告警”绑定在一起。

## 主要符号

- `PlanCacheType`：可复制枚举，取值为 `DefaultNoCache` (0)、`SessionPrepared` (1) 和 `SessionNonPrepared` (2)，区分未设置、会话预处理与会话非预处理缓存。
- `PlanCacheState`：私有状态容器，包含 `useCache`、`cacheType`、`planCacheUnqualified`、`forcePlanCache` 和 `alwaysWarnSkipCache`；外部不能绕过 tracker 直接改写。
- `PlanCacheTracker`：公开跟踪器，由 `Mutex<PlanCacheState>` 和 `Arc<dyn WarnAppender + Send + Sync>` 组成。`NewPlanCacheTracker` 构造时五个状态字段均为关闭/空值/默认类型。
- `EnablePlanCache`、`SetCacheType`、`SetForcePlanCache` 和 `SetAlwaysWarnSkipCache`：独立设置状态字段；这些方法不会隐式清理其他字段。
- `SetSkipPlanCache`：会改变可用性的跳过入口。`WarnSkipPlanCache` 则只在已选择具体缓存类型时记录/告警，不改变 `useCache`。两者共用私有 `warnSkipPlanCache`。
- `UseCache` 和 `PlanCacheUnqualified`：分别返回布尔标志和克隆后的原因字符串。`Save` 返回五元组，`Restore` 用对应参数一次替换整个 `PlanCacheState`。
- `RangeFallbackHandler<'a>`：借用 `PlanCacheTracker` 和 `WarnAppender`，内含 `std::sync::Once`。`NewRangeFallbackHandler` 建立借用关系，`RecordRangeFallback` 执行回退处理。

## 执行流程

1. `StatementContext::build` 在 `pkg/sessionctx/stmtctx/stmtctx.rs` 中把其 warning handler 转成 `Arc<dyn WarnAppender + Send + Sync>`，再调用 `NewPlanCacheTracker`。新 tracker 默认不使用缓存。
2. 规划入口根据语句种类调用 `SetCacheType`，并在进入缓存候选路径时调用 `EnablePlanCache`。例如 `pkg/session/runtime/planning.rs` 为 prepared 路径设置 `SessionPrepared`。
3. 下游逻辑发现不可安全缓存的条件时调用 `SetSkipPlanCache(reason)`。如果 `useCache` 已为 `false`，调用直接返回，不覆盖先前原因，也不重复告警。
4. 若 `forcePlanCache` 为 `true`，方法追加 `force plan-cache: may use risky cached plan: ...` 后返回；`useCache` 和 `planCacheUnqualified` 均不改变。
5. 普通路径先把 `useCache` 置为 `false`，再由 `warnSkipPlanCache` 保存原因。prepared 语句总是追加 `skip prepared plan-cache` 告警；non-prepared 语句仅当 `alwaysWarnSkipCache` 为真时追加告警。
6. range 构建超出 `tidb_opt_range_max_size` 时，`RangerContext::RecordRangeFallback` 转发到 handler。handler 每次都先用固定原因 `in-list is too long` 调用 `SetSkipPlanCache`，再通过 `Once::call_once` 仅用首次的 `rangeMaxSize` 生成一条容量超限告警。
7. 需要试探规划分支时，`StatementContext::SaveLogicalPlanBuildState` 调用 `Save`，`RestoreLogicalPlanBuildState` 以保存的五个值调用 `Restore`，使候选分支对 tracker 的修改可整体回滚。

## 数据与状态

`useCache` 是最终可用性开关，但它不包含缓存类型；`cacheType` 同时决定跳过告警的文案与非预处理语句的默认静默策略。`planCacheUnqualified` 保留首次有效关闭缓存的原因：关闭后再次调用 `SetSkipPlanCache` 会早返；强制缓存路径也不写该字段。`WarnSkipPlanCache` 是例外，它可在 `useCache` 不变时覆盖原因，但 `DefaultNoCache` 会直接忽略。

`Save` 对原因字符串做 clone，因此快照不借用内部锁卫；`Restore` 把五个字段同时替换，避免读者看到部分恢复状态。`RangeFallbackHandler` 不拥有 tracker 或 warning sink，其生命期 `'a` 保证两个借用目标比 handler 存活更久；`Once` 的“已报告”位只在 handler 实例内有效。

## 依赖与调用关系

上游直接证据包括：

- `pkg/sessionctx/stmtctx/stmtctx.rs` 使用 `NewPlanCacheTracker`、`Save`、`Restore`、`NewRangeFallbackHandler` 和 `RecordRangeFallback`，将该模块接入语句生命周期。
- `pkg/expression/exprstatic/exprctx.rs` 和 `pkg/expression/sessionexpr/sessionctx.rs` 持有或暴露 `Arc<PlanCacheTracker>`，并将表达式构建阶段的 `SetSkipPlanCache` 转发给 tracker。
- `pkg/util/ranger/context/context.rs` 把两个对象作为 `Option<&...>` 携带，供 ranger 在参数改写或 range 超限时调用。
- `pkg/session/runtime/planning.rs`、`pkg/planner/core/optimizer_runtime.rs` 及多个 expression/ranger 模块通过上述上下文设置类型、启用缓存、检查 `UseCache` 或记录跳过原因。

下游只有两类行为依赖：`WarnAppender::AppendWarning` 接收由 `errors::NewNoStackError` 构造的错误，以及标准库 `Mutex`/`Once` 提供互斥与单次执行语义。本文件不访问计划缓存容器，也不决定缓存 key 或计划命中。

## 错误处理与边界

公开方法不返回 `Result`。业务上的“不可缓存”不是 Rust 错误，而是状态转换和 SQL warning。普通跳过仅在 `useCache == true` 时生效；这使首个有效跳过原因保持稳定。强制缓存时即使有风险也保留缓存资格，调用方必须意识到此时 `PlanCacheUnqualified()` 不会反映新风险原因。

`warnSkipPlanCache` 内仍有 `DefaultNoCache -> "unknown cache type"` 分支，但公开 `WarnSkipPlanCache` 会在该类型上跳过，而 `SetSkipPlanCache` 只有在未设置类型却已启用缓存时才可到达该告警分支。non-prepared 默认仅记录原因而不告警。`rangeMaxSize` 是 `i64`，本文件不校验数值的正负或合理性，只将它插入告警文案。

所有 `Mutex::lock` 都使用 `expect("plan cache tracker mutex poisoned")`；若持锁代码曾 panic 导致锁中毒，后续访问会 panic，而不是恢复中毒状态。另外，警告在 tracker 锁持有期间追加，自定义 `WarnAppender` 不应重入调用同一 tracker，否则可能自锁。

## 并发与资源生命周期

`PlanCacheTracker` 的五个状态字段在同一把 `Mutex` 下读写，因此单个方法内的检查与更新是不可分割的；`Save` 看到一致五元组，`Restore` 也不暴露中间状态。warning handler 由 `Arc` 共享且 trait object 要求 `Send + Sync`，使 tracker 可在线程间安全共享。文件本身不创建线程、任务、通道或事务。

`RangeFallbackHandler<'a>` 通过借用绑定资源生命周期，销毁 handler 不会销毁 tracker 或 warning sink。`Once` 保证多线程并发调用同一 handler 时最多一次 fallback 容量告警，但每次调用仍会尝试 `SetSkipPlanCache`。新建 handler 会带来新的 `Once`，因此“只告警一次”不是 tracker 全局或会话全局属性。`RangerContext::Detach` 保留两个 handler 的指针身份，且其注释明确要求并行执行前为 session 创建新 `StatementContext`，不应把借用延伸解读为可并行共享原 statement context。

## 与 Go 版本的对应关系

Go 对照文件为 `pkg/util/context/plancache.go`。Rust 保留了同名枚举值、tracker 五个可变字段、所有公开操作、告警文案、强制缓存分支、non-prepared 告警开关以及 range fallback 的 `sync.Once` 语义。Go 的零值 `PlanCacheTracker` 与 Rust `NewPlanCacheTracker` 初始状态等价；Go `sync.Mutex` 映射为 `Mutex<PlanCacheState>`，Go 的嵌入字段则集中到私有 state struct。

主要语言差异是：Rust 构造器要求拥有 `Arc<dyn WarnAppender + Send + Sync>`，而 Go 保存接口值；Rust fallback handler 用生命期检查两个借用，Go 用指针/接口；Rust 读取原因和快照时需 clone 拥有的 `String`；Rust 锁中毒会在 `expect` 处 panic，Go 锁没有对应的 poison 状态。这些是表示和故障模型差异，不改变正常路径的分支语义。

Go 相关测试 `pkg/sessionctx/stmtctx/stmtctx_test.go` 覆盖 statement context 快照恢复后的缓存状态，`pkg/util/ranger/context/context_test.go` 覆盖 Detach 保留 handler 身份。Rust 直接行为对照测试位于独立文件 `pkg/util/context/migration_aster_unit_test.rs`，没有把测试嵌入生产源文件。

## 扩展指南

- 增加缓存类型时，必须同时扩展 `PlanCacheType` 和 `warnSkipPlanCache` 的穷尽匹配，核对 Go 同路径常量数值及告警策略，并检查 `Save`/`Restore` 的快照消费者是否需新字段。
- 增加 tracker 状态时，需要修改 `PlanCacheState`、`NewPlanCacheTracker`、`Save`、`Restore`，以及 `pkg/sessionctx/stmtctx/stmtctx.rs` 中的 `LogicalPlanBuildState` 和两个快照方法；否则嵌套规划会泄漏候选分支状态。
- 改变跳过顺序或告警规则时，要保留“已关闭则不覆盖首因”、“强制模式不关闭且不写不合格原因”和“non-prepared 默认静默”等现有契约，或者明确同步 Go 实现与上游使用者。
- 改变 range fallback 时，不要把 tracker 禁用放进 `Once` 闭包；只有容量告警应去重。还应评估 handler 粒度变化是否导致每条语句出现多次告警。
- 回归测试应放在独立 Rust 测试文件，首选扩展 `pkg/util/context/migration_aster_unit_test.rs`；若改动 statement 快照或 ranger 借用语义，同步扩展 `pkg/sessionctx/stmtctx/stmtctx_test.rs` 或 `pkg/util/ranger/context/context_test.rs`，并核对对应 Go 测试意图。
- 性能风险主要在高频调用的互斥锁竞争、原因字符串 clone 和持锁追加 warning；兼容风险则包括告警文案、枚举数值及首因保留规则的变化。

## 验证依据

- RustCodeGraph `status` 确认索引含 7,032 个 Rust 文件；`files --filter pkg/util/context` 确认 `plancache.rs`、Go 对照和独立迁移测试均在索引中。
- RustCodeGraph `explore "pkg/util/context/plancache.rs PlanCacheTracker PlanCacheType EnablePlanCache"` 显示表达式静态/会话上下文、statement context 等使用者；`node --file pkg/util/context/plancache.rs` 核对了全部 242 行生产源码。精确 `callers/callees` 对 Rust 同名方法未返回边，因此用限定 `*.rs` 的 `rg` 补齐上述直接调用点。
- 已读生产/配置证据：`pkg/util/context/plancache.rs`、`pkg/util/context/lib.rs`、`pkg/util/context/Cargo.toml`、`pkg/util/context/warn.rs`、`pkg/sessionctx/stmtctx/stmtctx.rs`、`pkg/util/ranger/context/context.rs` 和 `pkg/util/context/plancache.go`。
- 已读独立测试证据：`pkg/util/context/migration_aster_unit_test.rs` 的 `migration_plan_cache_tracker_matches_go_branches` 与 `migration_range_fallback_warns_once_but_always_disables_cache`，`pkg/sessionctx/stmtctx/stmtctx_test.rs` 的 `test_logical_plan_build_state_restore`，以及 `pkg/util/ranger/context/context_test.rs` 的 `test_context_detach`。还用 `rg` 确认了对应 Go 测试路径。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验证应确认本文件存在且恰有十一个规定的二级标题。
