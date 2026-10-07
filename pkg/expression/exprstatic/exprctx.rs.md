# `pkg/expression/exprstatic/exprctx.rs`

## 文件定位

本文件实现 `astersql-expression-exprstatic` crate 的静态表达式构建上下文。crate 入口 `pkg/expression/exprstatic/lib.rs` 以 `exprctx_impl` 模块加载并公开再导出这里的符号；`Cargo.toml` 则声明它依赖表达式上下文接口 crate、静态求值所需的字符集/MySQL 常量、会话变量默认值、校对、随机数与计划缓存工具。

这里的“静态”表示上下文不借用完整会话对象，而是持有表达式构建期间所需的一组快照或共享资源。生产代码可直接构造它，例如 `pkg/session/runtime/planning.rs::plan_context_with_params_and_explain` 为带参数的规划流程创建 `EvalContext`、`ExprContext` 和共享的会话列 ID 分配器；`pkg/expression/sessionexpr/sessionctx.rs::ExprContext::IntoStatic` 则通过 `MakeExprContextStatic` 把会话绑定上下文物化为该静态类型。DDL、分区表达式和无会话表达式构建也直接调用 `NewExprContext`。

## 核心职责

`ExprContext` 在 `EvalContext` 之上聚合表达式“构建期”配置：连接字符集/校对规则、utf8mb4 默认校对、是否启用新校对框架、块加密模式、`SYSDATE` 语义、noop 函数策略、MySQL 随机数状态、计划缓存可用性、计划列 ID、连接 ID、窗口高精度选项和 `GROUP_CONCAT` 长度上限。

它同时承担四类职责：

- 用 `NewExprContext` 建立完整且可用的服务器默认状态，再用 `ExprCtxOption` 覆盖指定字段。
- 用 `Apply` 从现有上下文派生浅拷贝，保留未覆盖字段并共享带内部可变性的资源。
- 实现 `exprctx::BuildContext`、`exprctx::ExprContext` 和 `exprctx::StaticConvertibleExprContext`，让规划器和表达式代码只依赖接口。
- 用 `LoadSystemVars` 与 `MakeExprContextStatic` 分别完成系统变量加载和会话上下文静态化。

## 主要符号

- `ExprCtxState`：私有字段集合。普通标量和 `String` 在克隆时复制；`eval_ctx`、`rng`、`plan_cache_tracker` 与 `column_id_allocator` 使用 `Arc`。
- `ExprCtxOption = Box<dyn FnOnce(&mut ExprCtxState)>`：一次性构造/更新闭包。`WithEvalCtx`、`WithCharset`、`WithRng`、`WithPlanCacheTracker` 等选项只改动对应字段。
- `WithNoopFuncsMode`：唯一在创建选项时主动断言取值范围的选项，只接受 `variable::{OnInt, OffInt, WarnInt}`。
- `ExprContext`：公开静态上下文，内部仅含一个 `ExprCtxState`，不暴露字段。
- `NewExprContext`：按服务器默认值构造上下文、应用选项，并保证默认计划缓存跟踪器绑定最终的 `EvalContext` 告警接收器。
- `ExprContext::Apply`：克隆状态后依次执行选项，返回新上下文而不替换原对象。
- `ExprContext::LoadSystemVars` / `load_system_vars_internal`：统一解析变量，再分别更新内嵌 `EvalContext` 和本层字段。
- `MakeExprContextStatic`：从任意 `StaticConvertibleExprContext` 建立静态对象，复制求值快照，延续列 ID，并优先复用 RNG 与计划缓存跟踪器的 `Arc`。
- 三个 trait 实现：`BuildContext` 暴露构建参数和可变决策入口，`exprctx::ExprContext` 补充窗口/聚合配置，`StaticConvertibleExprContext` 暴露静态化所需状态。

## 执行流程

`NewExprContext` 首先查询 `mysql::DefaultCharset` 的字符集信息；该默认字符集不存在会直接 `expect` 失败。随后创建默认 `EvalContext`、按时间播种的 `MysqlRng`、从 0 起步的简单列 ID 分配器，以及绑定默认求值告警接收器且已启用缓存的 `PlanCacheTracker`。它填入其余 `vardef`/`variable`/`collate_crate` 默认值，再按传入顺序消费所有选项，因此同一字段多次设置时最后一个选项生效。若选项替换了 `EvalContext` 却没有替换默认 tracker，函数会重建并启用 tracker，使其告警接收器指向新的求值上下文；若调用者显式传入 tracker，则保留调用者选择。

`Apply` 对现有 `ExprCtxState` 做 `Clone`，然后应用选项。字符串与标量修改只影响新对象；`Arc` 字段仍与原对象指向同一资源，除非对应选项显式替换。这一行为使规划流程能从一个基础上下文派生不同接口视图，同时共享计划缓存决策和列 ID 序列。

`LoadSystemVars` 先调用 `parse_system_vars`。解析成功后，`load_system_vars_internal` 先让内嵌 `EvalContext` 根据同一份 `ParsedSystemVars` 和原始变量表派生新值，再遍历传入键（按 ASCII 小写匹配）为表达式层变量组装选项，最后调用 `Apply`。字符集或校对任一键都会同时写入解析后的字符集/校对配对；未知变量或非法值在解析阶段返回错误，而不是被本层静默接受。

`MakeExprContextStatic` 先通过 `MakeEvalContextStatic` 创建独立求值快照。若源上下文可提供 RNG/tracker 的 `Arc`，则直接复用；否则分别从源 RNG 种子重建、用 tracker 的 `Save`/`Restore` 重建状态。新列 ID 分配器从源的最后 ID 起步，下一次分配会继续递增；其余字段逐项写入 `NewExprContext`。

## 数据与状态

`ExprCtxState` 的数据可分三组：

- 不可变快照字段：`charset`、`collation`、`default_collation_for_utf8mb4`、`new_collation_enabled`、`block_encryption_mode`、`sysdate_is_now`、`noop_funcs_mode`、`connection_id`、`windowing_use_high_precision`、`group_concat_max_len`。
- 求值快照：`Arc<EvalContext>`。`GetEvalCtx`、`GetStaticEvalCtx` 和 `GetStaticConvertibleEvalContext` 都从这一字段提供不同接口视图。
- 共享可变资源：`Arc<MysqlRng>`、`Arc<PlanCacheTracker>` 和 `Arc<dyn PlanColumnIDAllocator>`。因此 `Apply(Vec::new())` 并不是深复制：原对象与派生对象会观察到同一随机数序列推进、同一“跳过计划缓存”状态和同一列 ID 计数。

静态实现固定返回 `IsInNullRejectCheck == false`、`IsConstantPropagateCheck == false` 和 `IsReadonlyUserVar == false`。需要这些特殊语义时，应使用 `pkg/expression/exprctx/context.rs` 中的上下文装饰器或会话实现，而不是改变静态默认值。

## 依赖与调用关系

上游生产入口包括：

- `pkg/session/runtime/planning.rs::plan_context_with_params_and_explain`：创建规划、range 构建和下推构建所用的静态表达式上下文，并通过 `SessionPlanColumnIDAllocator` 接入会话列 ID。
- `pkg/expression/sessionexpr/sessionctx.rs::ExprContext::IntoStatic`：调用 `MakeExprContextStatic(self)`，把会话实现转换为可脱离会话持有的对象。
- `pkg/session/runtime/{mview_ddl,modify_column_backfill,ddl}.rs`、`pkg/ddl/storage_class.rs`、`pkg/table/tables/{partition_expr,canonical_partition_expr,index}.rs`：在 DDL、回填、存储属性或表表达式处理中直接构造静态上下文。
- 表达式与规划器代码通过 `exprctx::BuildContext`/`ExprContext` 消费接口；例如 `pkg/expression/expression.rs` 和多个 planner operator 调用 `AllocPlanColumnID`。

下游依赖由 `pkg/expression/exprstatic/Cargo.toml` 和源码导入共同界定：`exprctx-crate` 提供三个 trait 与列 ID 分配器；`contextutil-crate` 提供告警和计划缓存 tracker；`mathutil-crate` 提供 MySQL RNG；`charset-crate`、`mysql-crate`、`collate-crate` 提供字符集/校对默认值；`vardef-crate`、`variable-crate` 提供系统变量名、默认值与 ON/OFF/WARN 映射。`parse_system_vars`、`ParsedSystemVars` 和 `MakeEvalContextStatic` 来自同 crate 的 `evalctx.rs`。

RustCodeGraph 的文件节点报告 `exprctx.rs` 被 27 个索引文件使用；由于精确 `callers/callees` 查询未在本次限定时间内返回结果，以上具体调用边均由符号搜索后读取对应调用点确认，不以未返回的图结果作推断。

## 错误处理与边界

`LoadSystemVars` 是本文件主要的可恢复错误入口，返回 `Result<ExprContext, contextutil::errors::SharedError>`，并用 `?` 原样传播 `parse_system_vars` 的共享错误。Rust 测试 `TestExprSystemVarNormalizationMatchesGo` 验证非法块加密模式返回错误，`migration_aster_unit_test.rs` 验证未知变量返回错误；合法数值还会按 Go SysVar 规则规范化，例如 `group_concat_max_len=1` 被裁剪为 4、块加密模式被转为小写、noop 枚举索引 `2` 变为 `WarnInt`。

构造阶段有两个不可恢复约束：服务器默认字符集必须存在，否则 `NewExprContext` 的 `expect` 会 panic；`WithNoopFuncsMode` 的入参不属于 ON/OFF/WARN 时会由 `assert!` panic。其他 `Arc` 选项的类型本身不能表示空指针，但 trait 对象的具体实现仍必须满足各 trait 契约。

系统变量更新只处理调用者实际提供的键：未出现的字段继承旧上下文。键比较大小写不敏感；字符集和校对是联动解析的配对。`MakeExprContextStatic` 只保证 `EvalContext` 独立，RNG 与 tracker 在源实现提供 `Arc` 时有意共享；不能把该函数理解为所有内部对象的深复制。

## 并发与资源生命周期

本文件自身不启动线程、任务、通道或事务，生命周期由值和 `Arc` 管理。`ExprContext` 没有实现自定义 `Drop`；最后一个强引用释放时，内嵌资源随之释放。

`Apply` 及静态化后的共享语义是并发分析的重点：计划缓存 tracker 的一次 `SetSkipPlanCache` 会对所有共享该 `Arc` 的上下文可见；列 ID 分配器也由所有派生对象共享。默认实现 `SimplePlanColumnIDAllocator` 在 `pkg/expression/exprctx/context.rs` 中使用 `AtomicI64` 和 `SeqCst`，支持并发单调分配；自定义 `PlanColumnIDAllocator` 则必须自行提供调用场景所需的并发保证。RNG 是否可并发使用取决于 `MysqlRng` 的内部实现和调用约束，本文件只共享引用，不额外串行化访问。

当 `MakeExprContextStatic` 无法复用 tracker `Arc` 时，新 tracker 绑定新 `EvalContext` 的告警接收器，并恢复源 tracker 的缓存开关、缓存类型、原因、强制缓存和始终告警状态。列 ID 仅复制最后值到新分配器，因此之后与源上下文分成两个独立序列。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/expression/exprstatic/exprctx.go`。Rust 保留了 Go 的 `exprCtxState`/`ExprCtxOption`、默认构造、浅复制 `Apply`、三个上下文接口、系统变量加载和静态化总体流程；字段集合也与 Go 对齐，包括后加入的 `newCollationEnabled`。

语言层差异主要是所有权表达：Go 使用指针和 variadic options，Rust 使用 `Arc`、trait object 和 `Vec<ExprCtxOption>`；Go 构造器先允许字段为空、应用选项后补默认对象，Rust 先建立完整默认对象再覆盖。为保持 Go 的关键语义，Rust 构造器额外检测“EvalCtx 已替换但 tracker 仍是默认实例”，并重新绑定 tracker 告警接收器。

静态化语义一致：求值上下文是独立快照，RNG 与 tracker 保持指针共享，列 ID 从最后值继续但使用新分配器。Rust 通过 `GetRngArc`/`GetPlanCacheTrackerArc` 允许已知实现零损复用；不提供 `Arc` 的其他 trait 实现走种子复制和 `Save`/`Restore` 回退。Rust 的 `LoadSystemVars` 使用共享的 `ParsedSystemVars`，替代 Go 临时构造 `SessionVars`，但测试覆盖的大小写、裁剪、枚举和字符集联动语义与 Go 一致。

## 扩展指南

新增表达式构建配置时，应同时检查并按需修改：`ExprCtxState` 字段、`NewExprContext` 默认值、对应 `With...` 选项和 getter、`exprctx` crate 中的 trait 契约、相关 trait 实现、`MakeExprContextStatic` 的复制列表，以及若由系统变量控制则修改 `ParsedSystemVars`/`parse_system_vars` 与 `load_system_vars_internal` 的匹配分支。Go 对照 `exprctx.go` 仍是行为基准，不应只为通过 Rust 测试而省略字段或分支。

测试优先扩展独立文件 `pkg/expression/exprstatic/exprctx_test.rs`：默认/全选项字段放入 `checkDefaultStaticExprCtx` 与 `checkOptionsStaticExprCtx`，共享或替换语义放入 `TestStaticExprCtxApplyOptions`，静态化字段放入 `TestMakeExprContextStatic`，系统变量放入 `TestExprCtxLoadSystemVars` 或规范化测试。跨层解析规则还应同步检查 `evalctx_test.rs` 和 `migration_aster_unit_test.rs`；Go 行为变化则对照 `exprctx_test.go`。

兼容风险集中在默认值、系统变量规范化和 trait 新方法；遗漏静态化字段会造成会话/静态上下文行为分叉。性能风险集中在无意把共享资源改成深复制、重复构造 tracker，以及在热路径增加字符串克隆。并发风险集中在更换列 ID 分配器或 tracker 时破坏现有共享语义。若增加新的资源型字段，应明确 `Apply` 是共享还是复制、`MakeExprContextStatic` 是否需要独立化，并以指针身份和状态传播测试固定契约。

## 验证依据

本说明读取并核对了以下直接证据：

- 生产实现与装配：`pkg/expression/exprstatic/exprctx.rs`、`lib.rs`、`Cargo.toml`、`evalctx.rs` 中被本文件调用的系统变量解析/静态化接口。
- 接口与并发基础：`pkg/expression/exprctx/context.rs` 中的 `BuildContext`、`ExprContext`、`StaticConvertibleExprContext`、`PlanColumnIDAllocator` 和原子简单分配器。
- Go 对照：`pkg/expression/exprstatic/exprctx.go`。
- 独立测试：`pkg/expression/exprstatic/exprctx_test.rs`、`exprctx_test.go`、`migration_aster_unit_test.rs`；覆盖默认值、完整选项、tracker 告警绑定、`Apply` 共享、列 ID 替换、静态化、系统变量加载及非法输入。
- 生产调用点：`pkg/session/runtime/planning.rs`、`pkg/expression/sessionexpr/sessionctx.rs`，并通过仓库符号搜索确认 DDL、表和表达式/规划器中的其他直接使用点。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/expression/exprstatic` 定位目标与相邻 Go/Rust 文件；`node --file pkg/expression/exprstatic/exprctx.rs --offset 1 --limit 500` 返回完整 469 行源码并报告 27 个使用文件；`query NewExprContext --json` 区分出 Go、静态 Rust 和会话 Rust 的同名符号。精确 `callers/callees` 查询未在限定时间内返回，故未据此声称具体边。

本任务为纯文档分析，按计划不运行 Cargo。人工复核确认本文分别回答了文件存在原因、默认构造/派生/系统变量加载/静态化的运行方式，以及扩展时必须同步的状态、接口和独立测试。
