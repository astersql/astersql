# `pkg/expression/exprctx/context.rs`

## 文件定位

本文件是 `astersql-expression-exprctx` crate 的核心上下文契约，源码由 [`lib.rs`](./lib.rs) 的 `mod context; pub use context::*;` 对外重导出。它不执行某一种具体表达式，而是规定表达式求值、表达式构建和聚合/窗口构建所能读取或修改的会话快照，并提供若干只改变单一策略的包装器。Cargo 清单 [`Cargo.toml`](./Cargo.toml) 表明该 crate 位于 `pkg/expression/exprctx`，直接依赖类型/错误上下文、parser AST、排序规则、会话变量和 MySQL 随机数实现。

在应用链路中，上游的静态上下文与会话上下文实现这些 trait（例如 `pkg/expression/exprstatic/{evalctx.rs,exprctx.rs}` 和 `pkg/expression/sessionexpr/sessionctx.rs`），下游表达式、DDL 与 planner 代码通过 `dyn EvalContext`、`dyn BuildContext` 或 `dyn ExprContext` 消费稳定接口。RustCodeGraph 将本文件关联到 49 个 Rust 文件；这表示接口及重导出被广泛使用，不等同于每个包装函数都有 49 个直接调用者。

## 核心职责

- `PlanColumnIDAllocator` / `SimplePlanColumnIDAllocator` 为计划内派生列提供线程安全、单调（发生整数溢出前）递增的 ID，并保留 Go `atomic.Int64.Add` 的二进制补码回绕语义。
- `EvalContext` 汇总一次表达式求值所需的 SQL mode、类型与错误策略、时区/稳定当前时间、当前库、系统变量、用户变量、参数、告警处理和可选属性。
- `BuildContext` 在求值上下文之上增加字符集、parser 覆盖、排序规则、随机数、计划缓存决策、列 ID、优化检查标志和连接信息；`ExprContext` 再增加窗口精度与 `GROUP_CONCAT` 上限。
- `NullRejectCheckExprContext` 与 `ConstantPropagateCheckContext` 采用装饰器方式仅把一个优化标志强制为 `true`，其余能力委托给原 `ExprContext`。
- `InnerOverrideEvalContext`、`InnerOverrideBuildContext` 和 `CtxWithTruncateResult` 支撑 `CtxWithHandleTruncateErrLevel`：只替换截断相关的类型 flags 与错误组级别，且不原地修改调用方上下文。
- `StaticConvertibleExprContext` / `StaticConvertibleEvalContext` 定义把会话态上下文物化为静态快照所需的额外读取面；`SessionVarsLocation` / `AssertLocationWithSessionVars` 仅承担时区一致性测试断言。

## 主要符号

- `PlanColumnIDAllocator::{AllocPlanColumnID, GetLastPlanColumnID}`：分配新 ID 与读取最近 ID。`SimplePlanColumnIDAllocator` 内含私有 `AtomicI64`；`NewSimplePlanColumnIDAllocator(offset)` 以偏移量初始化，首次分配返回 `offset + 1`。
- `EvalContext: contextutil::WarnHandler + ParamValues`：要求实现者同时提供告警容器和参数读取。`CurrentTime` 返回 `Result<DateTime<Tz>, SharedError>`，并约定同一 `CtxID` 下重复调用得到同一时刻。`GetOptionalPropProviderUnwrapped` 默认等同于公开 provider 查询，允许装饰器显式绕过另一层访问限制。
- `BuildContext`：`NewCollationEnabled` 默认读取 collate crate；`ParseSQL` 默认返回 `None`，表示使用默认解析路径，实现者可返回自定义 AST/警告/错误结果。其余方法均为必须实现的会话构建决策。
- `ExprContext: BuildContext`：补充 `GetWindowingUseHighPrecision` 和 `GetGroupConcatMaxLen`。
- `UserVarsReader: Send + Sync`：提供用户变量值、类型与 trait-object 克隆，避免核心接口耦合可变 `SessionVars`。
- `WithNullRejectCheck` / `WithConstantPropagateCheck`：创建借用原上下文的轻量包装器；包装器分别覆盖 `IsInNullRejectCheck` 或 `IsConstantPropagateCheck`，其他 `BuildContext` / `ExprContext` 方法逐项转发。
- `CtxWithHandleTruncateErrLevel(ctx, level)`：返回 `CtxWithTruncateResult::{Original, Overridden}`。`WasOverridden` 暴露是否创建包装；枚举自身实现 `BuildContext`，调用方无需为两个分支写不同调用代码。
- `StaticConvertibleExprContext`：除 `ExprContext` 外读取静态可转换 eval context、plan-cache tracker 和最后列 ID；可选的 `GetRngArc` / `GetPlanCacheTrackerArc` 默认返回 `None`。
- `StaticConvertibleEvalContext`：补充全部参数、告警处理器，以及默认返回 `None` 的共享告警处理器 `Arc` 获取接口。

## 执行流程

1. 正常构建/求值时，具体实现（静态上下文或会话上下文）实现本文件 trait；表达式代码仅通过 trait 方法读取会话快照。`BuildContext::GetEvalCtx` 是构建态到求值态的主要桥梁。
2. 创建派生计划列时，调用 `AllocPlanColumnID`。`SimplePlanColumnIDAllocator` 用 `fetch_add(1, SeqCst)` 取得旧值，再 `wrapping_add(1)` 返回新值，使存储值和返回值在 `i64::MAX` 边界同步回绕。
3. 空拒绝或常量传播检查需要临时策略时，构造对应借用包装器；包装器仅对目标查询返回 `true`，将 parser、eval context、字符集、缓存决策和其他标志全部委托给原上下文。Rust 规划器当前另有 `pkg/planner/plannersession/context.rs` 的拥有型空拒绝包装，常量传播也有 `pkg/expression/constant_propagation.rs` 的适配器，因此不能把这两个借用构造器描述为所有生产路径的唯一入口。
4. 调用 `CtxWithHandleTruncateErrLevel` 时，先从 `GetEvalCtx` 复制轻量的 `TypeCtx` 与 `ErrCtx`，根据 `LevelWarn` / `LevelIgnore` / 其他级别计算 `TruncateAsWarning`、`IgnoreTruncateErr` 两个 flag，再核对截断错误组级别。
5. 若 flags 与截断组级别都已匹配，返回 `Original(ctx)`；否则构造两层包装：内层 eval 包装仅覆盖 `TypeCtx` / `ErrCtx`，外层 build 包装仅覆盖 `GetEvalCtx`。`CtxWithTruncateResult` 将两种结果统一为 `BuildContext`。
6. 静态化流程由 `pkg/expression/exprstatic/{evalctx.rs,exprctx.rs}` 消费 `StaticConvertible*` trait：能取得 `Arc` 时共享 RNG、plan-cache tracker 或告警处理器，不能取得时由该实现的静态化代码走复制状态的回退，而不是由本文件执行克隆。

## 数据与状态

`SimplePlanColumnIDAllocator.id` 是本文件唯一直接持有的可变状态，采用 `AtomicI64`，没有外部锁。两个优化包装器只保存 `&dyn ExprContext`，不拥有也不复制会话状态。`InnerOverrideEvalContext` 保存原 eval 引用和新的 `types::Context`、`errctx::Context` 值；这些值通过 `WithFlags` / `WithErrGroupLevel` 派生，因此原上下文保持不变。`InnerOverrideBuildContext` 保存原 build 引用与上述 eval 包装器。

上下文中的 `DateTime<Tz>`、`Datum`、`FieldType`、参数值、用户变量与可选属性均由实现者拥有；本文件只规定访问协议。`GetRngArc`、`GetPlanCacheTrackerArc`、`GetWarnHandlerArc` 的 `Option<Arc<_>>` 明确区分“可共享同一对象”和“只能走复制回退”。所有借用包装均受生命周期参数约束，不能比原上下文活得更久。

## 依赖与调用关系

上游实现包括 `pkg/expression/exprstatic/evalctx.rs`、`pkg/expression/exprstatic/exprctx.rs`、`pkg/expression/sessionexpr/sessionctx.rs`；后两类实现把真正的会话或静态状态适配成本文件 trait。RustCodeGraph/代码检索显示，`NewSimplePlanColumnIDAllocator` 在 `exprstatic/exprctx.rs` 的默认构造和静态化路径中使用，planner 测试也显式创建分配器；`StaticConvertible*` 则由 `MakeEvalContextStatic` / `MakeExprContextStatic` 消费。

直接生产调用证据包括：`pkg/expression/expression.rs:983` 用 `CtxWithHandleTruncateErrLevel(..., LevelIgnore)` 构建忽略截断的列转换上下文；`pkg/planner/util/null_misc.rs:438` 在 planner 辅助流程中按策略覆盖截断处理；`pkg/planner/core/operator/physicalop/base_physical_agg.rs` 通过列 ID 接口为 MPP AVG 转换分配列。Go 图中同名截断函数还被 DDL 默认值与列修改路径调用，但这些是 Go 侧证据，不能视作 Rust 路径已经逐项接线。

下游 crate 依赖来自 [`Cargo.toml`](./Cargo.toml)：`astersql-types` 提供 `Datum`、`FieldType`、类型 flags；`astersql-errctx` 提供错误组及级别；`astersql-util-context` 提供告警与 plan-cache tracker；parser/AST 支撑可选 SQL 解析；collate 与 mathutil 提供排序模式和 MySQL RNG；chrono/chrono-tz 表示稳定时间与时区。

## 错误处理与边界

本文件不吞并求值错误：`CurrentTime` 保留共享错误返回，`ParseSQL` 同时区分“未覆盖解析器”（外层 `None`）、“解析调用失败”（`Some(Err)`）以及“成功但带 parser 警告”（`Some(Ok((nodes, warnings)))`）。参数错误由 `ParamValues` 的 `ParamError` 传播，告警/注记由 `WarnHandler` 转发。

截断策略必须同时更新类型 flags 和 `ErrGroupTruncate` 的错误级别；只改一侧会让 datum 转换和错误分组处理不一致。未知于 Warn/Ignore 的级别走 `(false, false)` flags，并把错误组设置为传入级别。配置完全相同时不得叠加包装。`AssertLocationWithSessionVars` 在三方时区字符串不相等时直接 panic，源码明确标注其为测试辅助，不应放入普通请求错误处理路径。

列 ID 在 `i64::MAX` 后按 Go 原子整数语义回绕至 `i64::MIN`；因此“单调递增”只适用于不跨越整数边界的正常范围。`GetOptionalPropProviderUnwrapped` 是默认逃生口，新增访问控制装饰器时必须明确公开查询和解包查询的区别，避免意外绕过限制。

## 并发与资源生命周期

分配器使用 `Ordering::SeqCst`，并发调用获得全局可排序且不重复的递增结果；`migration_aster_unit_test.rs` 用 8 个线程各分配 500 次验证完整的 `1..=4000` 集合。`UserVarsReader` 强制 `Send + Sync`，可在线程间只读共享；共享型 RNG、tracker、warn handler 用 `Arc` 表达所有权。

其他包装器不创建线程、任务、通道或锁，也没有显式清理步骤。它们以引用借用原上下文，并由 Rust 生命周期保证包装器先于原对象销毁。截断覆盖只创建值/引用包装，不修改原上下文；重复请求同一策略会返回 `Original`，避免无界增长的动态包装链。警告处理继续转发到原 handler，所以覆盖上下文与原上下文共享告警生命周期和累计状态。

## 与 Go 版本的对应关系

Go 对照文件是 [`context.go`](./context.go)，主要接口、分配器、两个优化包装器、截断覆盖和静态转换接口逐项对应。Rust 的 `AtomicI64::fetch_add(...).wrapping_add(1)` 对齐 Go `atomic.Int64.Add(1)` 返回新值以及溢出回绕；独立 Rust 边界测试补充了 Go 源文件中未显式写出的该行为。

Go 依靠匿名嵌入自动转发接口方法，Rust 必须为 `NullRejectCheckExprContext`、`ConstantPropagateCheckContext` 和 override 类型显式实现并逐项委托。Go 的截断函数可以统一返回接口值，并在无变化时返回同一指针；Rust 用 `CtxWithTruncateResult` 表达借用原对象或拥有包装器，`WasOverridden` 与指针比较测试保留“不重复包装”语义。

Rust 相比当前 Go 文件新增/显式化了 `ParseSQL` 默认覆盖点、`GetOptionalPropProviderUnwrapped`、`UserVarsReader` trait 和可选 `Arc` 获取方法，这些是 Rust 对象安全、所有权与当前移植接线所需的接口扩展，不应反推为 Go 同名接口已有完全相同签名。Go 的时区断言直接接收 `*variable.SessionVars`；Rust 用最小 `SessionVarsLocation` trait 解耦具体会话类型。

Go 截断回归位于 [`context_override_test.go`](./context_override_test.go)；Rust 对应测试位于 [`context_override_test.rs`](./context_override_test.rs)，并由 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 补充分配器并发、包装器委托、时区断言等移植契约。

## 扩展指南

新增求值数据时，优先判断它是否为可选属性：若不是所有上下文都需要，宜扩展 `optional.rs` 的 provider 协议，而不是让 `EvalContext` 再增加强制方法。确需扩展 trait 时，必须同步静态与会话实现、所有测试替身，以及本文件三个显式转发包装器；否则编译错误之外，还可能出现某层装饰器丢失新语义。新增装饰器应只覆盖目标方法，并保留 `ParseSQL`、可选 provider 的公开/解包访问和静态转换共享语义。

修改截断策略应集中在 `CtxWithHandleTruncateErrLevel`，同时核对 `types::Flags` 与 `errctx::ErrGroupTruncate`，并扩展 `context_override_test.rs` 和 `migration_aster_unit_test.rs` 的 Error/Warn/Ignore、原对象不变、重复调用复用测试。修改列 ID 规则要同步 `context_test.rs` 的溢出边界、迁移测试的并发唯一性，以及 `exprstatic/exprctx_test.rs` 的静态化/自定义分配器场景。

`AssertLocationWithSessionVars` 仍应保持测试辅助性质。新测试应继续放在独立 `*_test.rs` 文件，并在 `lib.rs` 以 `#[cfg(test)]` 模块接入，不要内嵌到生产源文件。性能上应避免无变化时创建 override，避免为每次 getter 做深拷贝，并优先通过已有 `Arc` 接口共享明确线程安全的资源。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/expression/exprctx` 确认目标 Rust、Go 对照及独立测试均被索引。
- RustCodeGraph `node --file`：完整读取 `pkg/expression/exprctx/context.rs`（625 行）、`context.go`（266 行）、`context_test.rs`、`context_override_test.rs`、`migration_aster_unit_test.rs`；符号查询确认 Go/Rust 同名函数，并识别 `CtxWithHandleTruncateErrLevel` 的 Rust 符号 `context.rs::CtxWithHandleTruncateErrLevel`。
- RustCodeGraph `explore` / 调用图证据：确认 `AllocPlanColumnID` 的 Rust 调用包含 `exprstatic/exprctx.rs` 与 MPP 聚合转换，`GetGroupConcatMaxLen` 下游包含聚合 descriptor/PB 转换；同时显示 Go 同名截断函数的 DDL 调用面，文中已与 Rust 直接接线分开陈述。
- 配置与模块证据：读取 `pkg/expression/exprctx/Cargo.toml` 和 `lib.rs`，核对 crate 名、直接依赖、Go package 元数据、重导出以及测试模块装配。
- Go 与测试证据：`context.go`、`context_override_test.go`；Rust 独立测试 `context_test.rs`、`context_override_test.rs`、`migration_aster_unit_test.rs`，以及直接使用检索涉及的 `exprstatic/exprctx.rs`、`expression.rs`、`planner/util/null_misc.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前运行任务指定的 11 章节结构检查，并人工复核只新增本说明、未修改 Rust/Go/Cargo/只读总计划。
