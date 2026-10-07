# `pkg/executor/adapter_slow_log.rs`

## 文件定位

该文件属于 `astersql-executor` crate 的语句适配层，由 [`pkg/executor/lib.rs`](lib.rs) 以公开模块 `adapter_slow_log` 装配。它承接 [`pkg/sessionctx/variable/slow_log.rs`](../sessionctx/variable/slow_log.rs) 定义的慢日志规则、字段访问器和日志条目类型，负责规则字段预采集、匹配、条目补全、对象复用，以及 `WRITE_SLOW_LOG` hint 的强制输出。

当前 Rust 接线并不完整。`WriteForcedSlowLog`/`WriteForcedSlowLogTo` 已由 [`pkg/session/runtime/dispatch.rs`](../session/runtime/dispatch.rs) 和 [`pkg/session/runtime/control.rs`](../session/runtime/control.rs) 的真实语句结束路径调用；RustCodeGraph 与全仓库检索没有发现 `PrepareSlowLogItemsForRules`、`ShouldWriteSlowLog`、`SetSlowLogItems`、`putSlowLogItems` 的生产调用者。因此规则引擎主体目前是已实现、可测试的迁移接口，不能描述成已经替代 Go 的完整慢查询主链。

## 核心职责

1. `SessionSlowLogRules` 保存会话规则、合并后的字段集合、全局规则哈希快照和失效标志。
2. `updateAllRuleFields` 将会话规则、本连接的全局规则和 `UnsetConnID` 全局默认规则所引用的字段求并集，并用哈希与显式标志避免无变化时重复计算。
3. `PrepareSlowLogItemsForRules` 只预采集规则真正引用且具有 setter 的字段，延迟创建 `SlowQueryLogItems`；`CompleteSlowLogItemsForRules` 在决定记录日志后补齐其余已注册字段。
4. `Match` 和 `ShouldWriteSlowLog` 实现“规则之间 OR、单条规则的条件之间 AND”，并按会话、本连接全局、未绑定连接全局的顺序短路。
5. `getSlowLogItems`/`putSlowLogItems` 通过进程级池复用日志条目；归还前恢复为默认值，避免跨语句残留。
6. `plan_digest_accessor` 与 `session_plan_digest` 注册并计算 `plan_digest` 规则字段；`WriteForcedSlowLogTo` 则把已经构造好的条目格式化后写到指定 logger。

## 主要符号

- `SessionSlowLogRules`：执行器侧暂存的会话规则状态。`rules` 为可选会话规则；`effective_fields` 是三类适用规则字段的并集；`global_raw_rules_hash` 和 `need_update_effective_fields` 控制缓存失效。源码注释明确说明它是 `SessionVars` 尚未暴露全部对应字段时的过渡边界。
- `SlowLogRuleContext`：规则引擎对会话的抽象，提供连接 ID、规则状态、字段注册表、setter 和 matcher。其动态实现决定字段如何从真实会话写入条目以及如何解释阈值。
- `SlowLogStatement`：`SetSlowLogItems` 对执行语句的抽象。它要求先取得会话规则上下文，再由 `fill_slow_log_items` 填入计划、重试、RU/CPU、keyspace 等执行侧字段。当前仓库未发现生产 `impl SlowLogStatement`。
- `mergeConditionFields`、`updateAllRuleFields`：字段集合合并和缓存更新辅助函数。
- `getSlowLogItems`、`putSlowLogItems`：进程级 `Mutex<Vec<Box<SlowQueryLogItems>>>` 对象池入口。
- `PrepareSlowLogItemsForRules`、`CompleteSlowLogItemsForRules`、`SetSlowLogItems`：分别承担规则前置采集、剩余规则字段补全、执行侧字段补全。
- `Match`：经 `SlowLogRuleContext::match_rule_field` 匹配；未知字段如何处理由上下文实现决定。`MatchSessionVars`：直接查 `SlowLogRuleFieldAccessors`，未知字段返回 `false`，供现有 `SessionVars` 规则测试使用。
- `ShouldWriteSlowLog`：依次匹配会话规则、`connection_id` 对应全局规则、`UnsetConnID` 规则，任一命中即返回 `true`。
- `WriteForcedSlowLogTo`、`WriteForcedSlowLog`：只有 `force_by_hint` 为真才以 warn 级别输出 `SessionVars::SlowLogFormat(items)`；后者选择全局慢查询 logger。
- `plan_digest_accessor`、`session_plan_digest`、`SLOW_LOG_PACKAGE_INIT`：构造 plan-digest accessor，并在 Unix/macOS/Windows 的加载期注册。digest 先复用 `StatementContext` 缓存，否则扁平化物理计划、规范化、缓存后返回。

## 执行流程

规则路径的设计流程如下，但目前尚未由 Rust 生产语句主链完整串接：

1. `PrepareSlowLogItemsForRules` 调用 `updateAllRuleFields`。仅当会话显式标脏或 `GlobalSlowLogRules::raw_rules_hash` 变化时，重新合并会话、本连接和全局默认字段。
2. 若合并集合为空，直接返回 `None`。否则逐字段检查 `rule_field_has_setter`；找到第一个可采集字段时才从池中取条目，再调用 `set_rule_field`。只有 `Conn_ID` 等无 setter 的字段不会制造空条目。
3. 执行完成后，调用方可用 `ShouldWriteSlowLog` 做规则判断。它对每一组适用规则调用 `Match`，每条规则需所有条件满足，多条规则只需一条满足。
4. 决定输出后，`SetSlowLogItems` 先用 `CompleteSlowLogItemsForRules` 补齐未参与预采集的注册字段，再调用语句实现的 `fill_slow_log_items`，保持与 Go 相同的顺序。
5. 条目不再需要时应调用 `putSlowLogItems` 清零并归还池；当前 Rust 生产链没有该调用边，因此这一步仍是待接线契约。

已接线的 hint 路径从 [`pkg/session/runtime/dispatch.rs`](../session/runtime/dispatch.rs) 构造最小 `SlowQueryLogItems` 开始。若协议响应尚未结束，条目进入 `pending_protocol_slow_logs`；`FinishProtocolResponse` 写入 `WriteSQLRespTotal` 后调用 `WriteForcedSlowLog`。否则 dispatch 立即调用 `WriteForcedSlowLogTo` 或 `WriteForcedSlowLog`。两者都在 `force_by_hint == false` 时无副作用。

plan digest 路径由 `SLOW_LOG_PACKAGE_INIT` 在包加载期调用 `RegisterPlanDigestAccessor`。setter 调用 `session_plan_digest`：先读 `StatementContext::GetPlanDigest` 缓存；缓存不完整时复用或构造 `FlatPhysicalPlan`，仅对首个 select 操作为物理计划的情况调用 `NormalizeFlatPlan`，最后无论成功与否都通过 `SetPlanDigest` 缓存结果。

## 数据与状态

- 规则状态分成两层：`GlobalSlowLogRules` 按 `i64` 连接 ID 保存规则；`SessionSlowLogRules` 保存单会话规则及其派生字段集合。`UnsetConnID` 表示没有绑定特定连接的全局规则。
- `effective_fields` 使用 `BTreeSet<String>`，集合语义消除重复，稳定排序便于确定性处理；字段匹配前会把条件名转成 ASCII 小写。
- `SlowQueryLogItems` 是预采集、匹配、补全和格式化之间共享的可变载体。对象池条目必须只由一个调用流程拥有；`putSlowLogItems` 会用 `Default` 覆盖全部旧值。
- 对象池是 `OnceLock<Mutex<Vec<Box<_>>>>`，首次访问惰性初始化、生命周期覆盖整个进程，没有容量上限或淘汰策略。
- plan digest 同时在 `StatementContext` 内缓存规范化文本与 digest；`session_plan_digest` 还可能缓存扁平计划，避免后续重复扁平化。

## 依赖与调用关系

crate 边界由 [`pkg/executor/Cargo.toml`](Cargo.toml) 确认：该文件直接依赖 `astersql-sessionctx-variable`（规则、会话、日志格式）、`astersql-sessionctx-stmtctx`（语句上下文）、`astersql-parser`（digest）、`astersql-planner-core`（计划扁平化/规范化）和 `astersql-util-logutil`（logger）；这些均是 workspace 路径依赖，`adapter_slow_log` 本身不受 `nextgen` feature 条件控制。

RustCodeGraph 确认的主要内部边包括：`PrepareSlowLogItemsForRules -> updateAllRuleFields/getSlowLogItems`、`updateAllRuleFields -> mergeConditionFields`、`ShouldWriteSlowLog -> Match`、`SetSlowLogItems -> CompleteSlowLogItemsForRules/SlowLogStatement`、`WriteForcedSlowLog -> WriteForcedSlowLogTo`、`plan_digest_accessor -> session_plan_digest`、`session_plan_digest -> NormalizeFlatPlan/NewDigest`。

生产上游目前只确认强制输出边：`session::runtime::dispatch` 调用 `WriteForcedSlowLogTo`/`WriteForcedSlowLog`，`Session::FinishProtocolResponse` 调用 `WriteForcedSlowLog`。规则主体只有本文件内部边和测试调用；这与 Go 的 `ExecStmt.LogSlowQuery -> PrepareSlowLogItemsForRules -> ShouldWriteSlowLog -> SetSlowLogItems` 生产链不同。

## 错误处理与边界

- 这些 API 不返回 `Result`。无规则、无生效字段、无 setter 或未强制输出分别通过 `false`/`None`/跳过表达，不属于错误。
- `MatchSessionVars` 对注册表中不存在的字段返回 `false`；`Match` 则委托上下文处理未知字段。扩展上下文时必须让两条路径对未知字段保持兼容语义。
- 对象池锁若中毒，`getSlowLogItems` 和 `putSlowLogItems` 都会因 `expect("slow-query-log item pool poisoned")` panic；该文件没有降级或恢复策略。
- `session_plan_digest` 对缓存的 flat plan 和物理 plan 使用 `downcast(...).expect(...)`；类型不满足约定会 panic。没有计划、不能扁平化或首节点不是物理计划时，则缓存并返回空 digest，而非报错。
- `WriteForcedSlowLogTo` 不负责构造、补全、限流或判断慢阈值，只格式化传入条目并写日志。调用方若传入最小条目，输出内容也只反映已有字段。

## 并发与资源生命周期

全局对象池通过 `Mutex` 串行化访问，`OnceLock` 保证只初始化一次；条目离开池后以独占 `Box` 使用，归还时先完全清零。池没有线程本地分片和大小限制，高并发下可能产生锁竞争，突发峰值后也可能长期保留较多条目。

`SessionSlowLogRules` 本身没有内部同步，接口通过 `&mut impl SlowLogRuleContext` 更新，预期由单个会话执行流串行拥有。全局规则以不可变引用传入；该文件只比较其哈希和读取 map，不负责其发布同步。

hint 延迟输出时，日志条目由 session 状态的队列持有，直到协议响应结束；`FinishProtocolResponse` 取出一个条目、补写响应耗时后输出。logger 的并发安全性由 `astersql-util-logutil` 提供，本文件不持有后台任务、通道或事务资源。

## 与 Go 版本的对应关系

Rust 的 `mergeConditionFields`、`updateAllRuleFields`、对象池、`PrepareSlowLogItemsForRules`、`Match`、`ShouldWriteSlowLog`、`CompleteSlowLogItemsForRules` 和 `SetSlowLogItems` 分别对应 [`pkg/executor/adapter_slow_log.go`](adapter_slow_log.go) 的同名实现。核心语义保持一致：字段并集缓存、惰性分配、规则 OR/条件 AND、会话/连接/全局默认三级匹配，以及归还前清零。

存在以下重要差异：

- Go 把规则状态直接放在 `SessionVars.SlowLogRules`，Rust 暂以 `SessionSlowLogRules` 和 `SlowLogRuleContext` 抽象隔离；当前没有真实生产上下文实现。
- Go 使用 `sync.Pool`，Rust 使用显式的全局 `Mutex<Vec<Box<_>>>`；Rust 池不会自动丢弃缓存对象。
- Go `SetSlowLogItems` 直接读取 `ExecStmt` 并填充所有执行字段；Rust 将细节下放给 `SlowLogStatement::fill_slow_log_items`，但当前没有生产实现。
- Go 的 `ExecStmt.LogSlowQuery` 已完整处理开关、阈值/规则判断、限流、条目补全、输出、指标和 domain 慢查询记录；Rust 当前只把 hint 强制输出接入 session runtime，规则 API 尚未形成同等生产链。
- Go 包初始化注册 `plan_digest` accessor；Rust 用平台 link-section 初始化函数实现等价注册。Rust 独立测试验证注册发生在规则解析之前。
- Rust 额外提供 `MatchSessionVars`，用于绕过尚未落地的 `SlowLogRuleContext` 生产实现，直接复用 sessionctx 的 accessor 注册表。

## 扩展指南

接入完整 Rust 慢查询主链时，应优先为真实会话实现 `SlowLogRuleContext`、为真实执行语句实现 `SlowLogStatement`，然后在语句结束位置按“预采集 → 执行 → 匹配 → 补全 → 格式化/输出 → 归还”的顺序接线。不能只调用 `ShouldWriteSlowLog` 而传入空条目，也不能漏掉 `putSlowLogItems`；否则分别会改变规则结果或使池失去复用价值。

新增规则字段时，需要同步维护 sessionctx 的 `SlowLogRuleFieldAccessors`、解析阈值类型、setter/matcher、字段名小写规范，以及执行器上下文的注册/采集实现。若字段没有 setter，应确认它是否像 `Conn_ID` 一样由会话直接参与匹配，并添加“不会因此分配空条目”的独立测试。

修改 plan digest 行为时应保持缓存、扁平计划复用、规范化与大小写匹配约定一致，并同步 [`pkg/executor/adapter_slow_log_aster_unit_test.rs`](adapter_slow_log_aster_unit_test.rs) 及 [`pkg/sessionctx/variable/tests/slowlog/slow_log_test.rs`](../sessionctx/variable/tests/slowlog/slow_log_test.rs)。修改规则预采集/池行为时同步 [`pkg/executor/adapter_slow_log_test.rs`](adapter_slow_log_test.rs)；Rust 测试继续放在独立文件中，不嵌入生产源文件。

兼容风险主要是 Go/Rust 字段集合或匹配顺序漂移；正确性风险是未知字段、无 setter 字段或空规则的处理差异；性能风险是提前分配、重复填充、plan digest 重算以及全局池锁竞争。生产接线完成前应明确保留“尚未接线”标记，不能用单元测试覆盖替代运行时调用证据。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；目标文件 397 行被完整读取。
- RustCodeGraph 查询过 `updateAllRuleFields`、`PrepareSlowLogItemsForRules`、`ShouldWriteSlowLog`、`SetSlowLogItems`、`WriteForcedSlowLog`、`session_plan_digest` 的节点、callers/callees；关键边见“依赖与调用关系”。图查询显示规则主体没有 Rust 生产调用者，强制输出入口由 session runtime 调用。
- 读取的 Rust 生产证据：[`pkg/executor/adapter_slow_log.rs`](adapter_slow_log.rs)、[`pkg/executor/lib.rs`](lib.rs)、[`pkg/session/runtime/dispatch.rs`](../session/runtime/dispatch.rs)、[`pkg/session/runtime/control.rs`](../session/runtime/control.rs)、[`pkg/sessionctx/variable/slow_log.rs`](../sessionctx/variable/slow_log.rs)；`pkg/executor` 没有 `doc.go`。
- 读取的边界与对照证据：[`pkg/executor/Cargo.toml`](Cargo.toml)、[`pkg/executor/adapter_slow_log.go`](adapter_slow_log.go)、[`pkg/executor/adapter.go`](adapter.go)。Go 的 `LogSlowQuery` 明确串联规则预采集、判断、延迟补全、池回收、限流和输出。
- 读取的测试证据：[`pkg/executor/adapter_slow_log_test.rs`](adapter_slow_log_test.rs) 验证无 setter 字段不分配条目；[`pkg/executor/adapter_slow_log_aster_unit_test.rs`](adapter_slow_log_aster_unit_test.rs) 验证 hint 输出门控和 plan-digest 注册；[`pkg/sessionctx/variable/tests/slowlog/slow_log_test.rs`](../sessionctx/variable/tests/slowlog/slow_log_test.rs) 验证字段访问器、阈值和 AND/OR 语义；Go `adapter_test.go` 验证预采集/补全、三级规则匹配和 lazy/prealloc 基准场景。
- 本任务是纯文档分析，按任务约束未运行 Cargo；完成判据使用任务文件指定的 11 章节结构命令。
