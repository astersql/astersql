# `pkg/meta/model/flags.rs`

## 文件定位

`flags.rs` 定义 SQL 执行请求使用的 13 个 `u64` 位标志（位 0 至位 12），对应 Go 文件 [`pkg/meta/model/flags.go`](flags.go) 中供 `tipb.SelectRequest.Flags` 使用的协议常量。该文件本身没有函数、类型、trait、`impl` 或条件编译项，只保存跨模块共享的位编号与 Rust 命名兼容别名。

它由 [`internal/group1/lib.rs`](internal/group1/lib.rs) 通过 `#[path = "../../flags.rs"] mod flags; pub use flags::*;` 编入 `astersql-meta-model-group1`，再由包根 [`lib.rs`](lib.rs) 的 `pub use ::group_1::*` 经 `astersql-meta-model` 门面导出。当前可见的主要 Rust 消费者是 [`pkg/sessionctx/stmtctx/stmtctx.rs`](../../sessionctx/stmtctx/stmtctx.rs)：`StatementContext::PushDownFlags` 编码这些位，`StatementContext::InitFromPBFlagAndTz` 解码其中一部分。

## 核心职责

- 固定执行模式与错误处理策略的二进制协议位：截断、溢出、零日期、除零，以及 INSERT、UPDATE/DELETE、SELECT、集合运算、LOAD DATA、受限 SQL 和 TiKV 短路求值上下文。
- 保证 Go/Rust 对同一个 `u64` 字段采用相同位号；这些值是协议身份，不应重排或复用。
- 同时暴露 Go 风格的 `FlagXxx` 名称和 Rust 常见的 `FLAG_XXX` 别名。两组常量值完全相同，别名不会建立第二份状态。

该文件只负责“位代表什么”，不负责判断 SQL 模式、选择告警级别或发送 protobuf 请求；这些决策由语句上下文等调用方完成。

## 主要符号

| 位 | Go 风格常量 | Rust 别名 | 语义 |
| --- | --- | --- | --- |
| 0 | `FlagIgnoreTruncate` | `FLAG_IGNORE_TRUNCATE` | 忽略截断错误；优先级由消费方处理。 |
| 1 | `FlagTruncateAsWarning` | `FLAG_TRUNCATE_AS_WARNING` | 将截断降为 warning。 |
| 2 | `FlagPadCharToFullLength` | `FLAG_PAD_CHAR_TO_FULL_LENGTH` | 启用 CHAR 补齐到完整长度语义。 |
| 3 | `FlagInInsertStmt` | `FLAG_IN_INSERT_STMT` | INSERT 语句上下文。 |
| 4 | `FlagInUpdateOrDeleteStmt` | `FLAG_IN_UPDATE_OR_DELETE_STMT` | UPDATE 或 DELETE 共用的语句类型位。 |
| 5 | `FlagInSelectStmt` | `FLAG_IN_SELECT_STMT` | SELECT 语句上下文。 |
| 6 | `FlagOverflowAsWarning` | `FLAG_OVERFLOW_AS_WARNING` | 将数值溢出降为 warning。 |
| 7 | `FlagIgnoreZeroInDate` | `FLAG_IGNORE_ZERO_IN_DATE` | 忽略日期中的零值错误。 |
| 8 | `FlagDividedByZeroAsWarning` | `FLAG_DIVIDED_BY_ZERO_AS_WARNING` | 将除零降为 warning。 |
| 9 | `FlagInSetOprStmt` | `FLAG_IN_SET_OPR_STMT` | UNION、EXCEPT 或 INTERSECT 等集合运算上下文。 |
| 10 | `FlagInLoadDataStmt` | `FLAG_IN_LOAD_DATA_STMT` | LOAD DATA 上下文。 |
| 11 | `FlagInRestrictedSQL` | `FLAG_IN_RESTRICTED_SQL` | 内部受限 SQL；Go 注释以 Auto Analyze 为例。 |
| 12 | `FlagEnableTiKVShortCircuitExpression` | `FLAG_ENABLE_TIKV_SHORT_CIRCUIT_EXPRESSION` | 允许 TiKV 短路求值表达式。 |

所有公开符号均为编译期 `pub const u64`。`FLAG_*` 常量直接引用对应的 `Flag*` 常量；例如 `FLAG_IGNORE_TRUNCATE = FlagIgnoreTruncate`，因此两套名称不存在数值漂移空间。

## 执行流程

典型编码流程位于 `StatementContext::PushDownFlags`：

1. `PushDownFlagsWithTypeFlagsAndErrLevels` 先把类型标志和错误级别转成位掩码。`IgnoreTruncateErr` 为真时设置位 0；否则 `TruncateAsWarning` 为真时同时设置位 1 和位 6。这个 `if/else if` 保证忽略截断优先于“截断告警”。
2. 同一辅助函数根据 `IgnoreZeroInDate` 设置位 7，并在除零错误级别不是 `LevelError` 时设置位 8。
3. `PushDownFlags` 再设置短路求值、语句类型、LOAD DATA 和受限 SQL 位。INSERT、UPDATE/DELETE、SELECT 使用 `if/else if`，因此编码时最多选择其中一个主语句类型位；LOAD DATA 与受限 SQL 可与它叠加。
4. 形成的 `u64` 由上层作为下推请求标志使用；`flags.rs` 不执行 I/O。

反向流程位于 `StatementContext::InitFromPBFlagAndTz`：它用按位与恢复 INSERT、SELECT、DELETE 和短路求值状态，恢复除零错误级别、截断/零日期类型标志及时区。当前实现把共享的 UPDATE/DELETE 位解码为 `InDeleteStmt`，与 Go 实现一致；它不恢复 `InUpdateStmt`、`InLoadDataStmt`、`InRestrictedSQL`、位 2、位 6 或位 9。因此不能把所有 13 位描述为在该函数中完全往返。

## 数据与状态

本文件没有可变全局量、堆分配或运行期缓存。每个常量都是单一位的 `u64` 值，组合方式是按位或，检测方式是按位与非零。

关键不变量如下：

- 已分配位连续覆盖 `1 << 0` 到 `1 << 12`，各常量互不重叠。
- `FlagIgnoreTruncate` 显式标注为 `u64`，其余表达式通过常量上下文推断并以公开签名固定为 `u64`。
- `FlagTruncateAsWarning` 与 `FlagOverflowAsWarning` 是两个独立位，但当前编码逻辑在非严格截断告警分支中同时设置它们，以保持 TiKV 兼容行为。
- `FlagPadCharToFullLength` 与 `FlagInSetOprStmt` 在当前仓库 Rust 生产代码中未发现直接消费；它们仍与 Go 协议位对齐，属于已定义但当前接线证据不足的兼容位。

## 依赖与调用关系

编译边界为：

`flags.rs` → `astersql-meta-model-group1`（路径模块并公开再导出）→ `astersql-meta-model`（根门面公开再导出）→ `astersql-sessionctx-stmtctx`（其 `Cargo.toml` 以 `task-model = { package = "astersql-meta-model", ... }` 引用）。

本文件没有 `use`、函数调用或外部 crate 依赖；其依赖是 Rust 内建整数常量表达式。主要下游关系由 [`stmtctx.rs`](../../sessionctx/stmtctx/stmtctx.rs) 建立：

- `PushDownFlagsWithTypeFlagsAndErrLevels` 消费位 0、1、6、7、8。
- `StatementContext::PushDownFlags` 消费位 3、4、5、10、11、12，并调用上述辅助函数。
- `StatementContext::InitFromPBFlagAndTz` 消费位 0、1、3、4、5、7、8、12。

RustCodeGraph 已索引 `flags.rs` 的 27 个常量符号，并能定位 `FlagIgnoreTruncate`、`FlagEnableTiKVShortCircuitExpression` 及 group1 的模块装配；但常量跨 `pub use` 的 caller 边不完整。因此上述消费关系同时以精确符号搜索和调用方源码复核，而不是仅凭图的缺边作“未使用”判断。

## 错误处理与边界

常量声明自身不会返回错误或 panic。真正的边界来自协议解释：

- 未知的更高位会被当前解码逻辑自然忽略；本文件没有合法掩码或拒绝未知位的 API。
- 冲突位也不会在此校验。编码器通过控制流避免同时设置多个主语句类型位，但外部传入的掩码可以同时包含它们；解码器会分别读取 INSERT、SELECT 和 UPDATE/DELETE 位。
- 位 4 合并 UPDATE 与 DELETE，信息在协议层本来就不可区分；当前反向初始化选择 DELETE 状态，扩展时不能假设能从该位恢复 UPDATE。
- 位 0 与位 1 同时存在时，实际优先级取决于消费方。当前编码器不会同时生成二者，当前解码器则把两个布尔属性都交给 `DefaultStmtFlags` 构造链。
- 改变现有位号会破坏 Go、Rust、TiKV/Tipb 请求间兼容；新增语义应分配新位并验证所有生产者、消费者与测试。

## 并发与资源生命周期

所有值都是不可变编译期常量，可在任意线程无锁读取，不涉及原子变量、互斥锁、通道、任务、事务或析构顺序。位掩码的生命周期由调用方拥有；`flags.rs` 不持有 `StatementContext`，也不管理下推请求资源。

并发安全风险只可能来自调用方在并发修改其自身语句上下文时的同步策略，而不是这些常量。新增常量不会引入运行期内存或性能开销；真正需要评估的是协议双方版本不一致时对新位的忽略行为。

## 与 Go 版本的对应关系

[`flags.go`](flags.go) 同样声明 13 个常量，顺序和值与 Rust 的 `Flag*` 一一对应：从 `FlagIgnoreTruncate uint64 = 1` 到 `FlagEnableTiKVShortCircuitExpression = 1 << 12`。Go 注释提供了更完整的策略背景，例如只读语句忽略截断/零日期错误、严格模式决定错误或 warning、受限 SQL可用于 Auto Analyze；这些是协议意图，具体决策仍由调用方实现。

Rust 额外提供 13 个 `SCREAMING_SNAKE_CASE` 别名，Go 没有对应的第二套名称。除此之外，当前 `stmtctx.rs` 的 `PushDownFlags`、`PushDownFlagsWithTypeFlagsAndErrLevels` 和 `InitFromPBFlagAndTz` 分支结构与 [`stmtctx.go`](../../sessionctx/stmtctx/stmtctx.go) 对齐，包括位 1 与位 6 联动、主语句类型互斥、位 4 解码为 DELETE，以及短路位往返。

迁移状态不是“每个位均有完整 Rust 运行路径”：精确搜索只在 `flags.rs` 自身发现位 2 和位 9，且当前反向初始化不处理若干已定义位。它们应记录为协议常量已移植、当前仓库 Rust 消费接线未验证，而不是宣称完整支持。

## 扩展指南

新增协议标志时应：

1. 在 `flags.rs` 的最高已用位之后增加新的 `FlagXxx: u64`，并增加值引用式 `FLAG_XXX` 别名；同步修改 `flags.go` 或确认对应上游 Go 变更，绝不能重排旧位。
2. 根据方向在 `StatementContext::PushDownFlags`、`PushDownFlagsWithTypeFlagsAndErrLevels` 和/或 `InitFromPBFlagAndTz` 接线。若要求往返，显式检查编码与解码两侧，不以常量存在代替行为实现。
3. 优先扩展独立测试 [`pkg/sessionctx/stmtctx/stmtctx_test.rs`](../../sessionctx/stmtctx/stmtctx_test.rs)，覆盖单个位、组合、冲突优先级和编码/解码往返；模型固定值可扩展 [`bdr_1_aster_unit_test.rs`](bdr_1_aster_unit_test.rs) 或更聚焦的新独立测试文件。遵守仓库规则，不把测试内嵌进 `flags.rs`。
4. 同步 Go 对照测试 [`pkg/sessionctx/stmtctx/stmtctx_test.go`](../../sessionctx/stmtctx/stmtctx_test.go)，并核对接收该请求的 TiKV/Tipb 端是否认识新位。

兼容风险高于性能风险：位号复用或双方语义不一致会静默改变远端执行方式；单次按位运算的性能成本可忽略。若删除历史位，优先保留占位避免后续位号漂移。

## 验证依据

- 目标定义：[`pkg/meta/model/flags.rs`](flags.rs)，人工核对 13 个原始常量、13 个别名以及位 0—12。
- Go 对照：[`pkg/meta/model/flags.go`](flags.go)，核对常量值、协议用途和注释语义。
- 模块与 Cargo 边界：[`internal/group1/lib.rs`](internal/group1/lib.rs)、[`internal/group1/Cargo.toml`](internal/group1/Cargo.toml)、[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs) 以及 [`pkg/sessionctx/stmtctx/Cargo.toml`](../../sessionctx/stmtctx/Cargo.toml)。
- Rust 生产调用方：[`pkg/sessionctx/stmtctx/stmtctx.rs`](../../sessionctx/stmtctx/stmtctx.rs) 的 `StatementContext::PushDownFlags`、`PushDownFlagsWithTypeFlagsAndErrLevels`、`StatementContext::InitFromPBFlagAndTz`。
- Rust 独立测试：[`pkg/sessionctx/stmtctx/stmtctx_test.rs`](../../sessionctx/stmtctx/stmtctx_test.rs) 覆盖主要单个位、组合和短路位往返；[`pkg/sessionctx/stmtctx/stmtctx_1_aster_unit_test.rs`](../../sessionctx/stmtctx/stmtctx_1_aster_unit_test.rs) 覆盖编码/解码边界组合；[`bdr_1_aster_unit_test.rs`](bdr_1_aster_unit_test.rs) 与 [`go_merge_15_test.rs`](go_merge_15_test.rs) 固定关键位值。
- Go 测试：[`pkg/sessionctx/stmtctx/stmtctx_test.go`](../../sessionctx/stmtctx/stmtctx_test.go) 的 `TestStatementContextPushDownFLags` 覆盖与 Rust 对应的编码组合和短路位恢复。
- RustCodeGraph：`status` 显示索引含 `pkg/meta/model/flags.rs`（27 symbols）；`query`/`node` 定位 `flags.rs::FlagIgnoreTruncate`、`flags.rs::FlagEnableTiKVShortCircuitExpression`；`node --file pkg/meta/model/internal/group1/lib.rs --offset 450 --limit 30` 验证路径模块和公开再导出。图对跨再导出常量的 caller 边不完整，故调用边由精确 `rg` 与上述源码交叉验证。
- 本任务是纯文档分析，按计划不运行 Cargo；结构检查要求目标文件存在且恰有 11 个规定二级标题。
