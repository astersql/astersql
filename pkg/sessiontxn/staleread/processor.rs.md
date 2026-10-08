# `pkg/sessiontxn/staleread/processor.rs`

## 文件定位

本文件是 `astersql-sessiontxn-staleread` crate 的语句级过期读（stale read）判定器。它把一条 SELECT 或 EXECUTE 可见的 `AS OF TIMESTAMP`、事务级读时间戳、`tidb_read_staleness`、外部时间戳和既有过期读事务上下文，归并为唯一的读时间戳与对应 `InfoSchema`。模块入口 `pkg/sessiontxn/staleread/lib.rs` 公开重导出本文件的 API；crate 边界及 Go 包映射由 `pkg/sessiontxn/staleread/Cargo.toml` 声明。

当前 Rust 迁移状态需要与 Go 主链区分：RustCodeGraph 与仓库搜索只找到 `processor_test.rs`、`externalts_test.rs` 对 `StaleReadProcessor` 的 Rust 调用，未找到生产 Rust 调用者。生产 SQL 预处理接线仍可在 Go 的 `pkg/planner/core/preprocess.go` 中验证：`Preprocess` 创建处理器，访问表名时调用 `OnSelectTable`，执行预编译语句时调用 `OnExecutePreparedStmt`，随后把读 ts、evaluator 和 `InfoSchema` 写入预处理结果。因此 Rust 文件是已实现并有独立测试的移植边界，但尚不能据现有证据称其已接入 Rust SQL 主链。

## 核心职责

- `Processor` 定义查询判定结果及两个入口：普通 SELECT 的逐表处理和预编译语句执行。
- `BaseProcessor` 保存“本语句是否已求值”、最终 ts、快照元数据和可供 PREPARE 缓存的 evaluator，并保证结果只固化一次。
- `StaleReadProcessor` 实现来源优先级和互斥规则：活跃事务上下文优先走事务路径；事务外依次考虑语句级 AS OF、`txn_read_ts`、`tidb_read_staleness`、external ts，最后落到普通读。
- `parse_and_validate_as_of` 将可选表达式转换为 TSO，并要求后端校验快照读时间戳。
- 每次 `StaleReadProcessor::new` 调用 `Session::begin_statement`，清除语句级 stale/external-ts 缓存与 `statement_is_staleness`，使缓存生命周期不跨语句。

## 主要符号

- `StalenessTsEvaluator = Arc<dyn Fn(&Context, &SessionRef) -> Result<u64, Error> + Send + Sync>`：可在线程间共享的 ts 求值闭包。它让 PREPARE 保存求值逻辑而非只保存一次求值结果。
- `TableName { as_of: Option<Expression> }`：Rust 端针对本处理器所需信息裁剪的表引用，不是完整 parser AST。
- `Processor`：公开 trait。只读方法暴露是否过期读、快照 `InfoSchema`、读 ts 和可缓存 evaluator；变更方法是 `on_select_table` 与 `on_execute_prepared_stmt`。
- `BaseProcessor`：私有公共状态容器。`evaluated` 是一次性提交闩锁，`ts == 0` 表示普通读，`info_schema` 与 `evaluator` 是否存在取决于来源。
- `BaseProcessor::{set_non_stale_read,set_evaluated_ts,set_evaluated_ts_without_evaluator,set_evaluated_evaluator,set_evaluated_values}`：把不同来源归一到一次性写入。固定 ts 通常生成常量 evaluator；external ts 和事务上下文路径刻意不保存 evaluator。
- `StaleReadProcessor { base, statement_ts }`：`statement_ts` 专门用于多表/UNION 场景比较各表 AS OF 是否一致，不能用最终 `base.ts` 替代，因为最终 ts 也可能来自会话变量。
- `evaluate_from_transaction`：活跃事务分支。过期读事务复用 `start_ts`/`info_schema` 并标记已挂接本地临时表；普通事务直接确定为非过期读。
- `evaluate_from_statement_or_variables`：事务外来源选择器，实现互斥检查与固定优先级。
- `parse_and_validate_as_of`：无表达式返回 0；有表达式时调用 `calculate_as_of_ts_expr`，再经 `SessionBackend::validate_snapshot_read_ts` 校验。

## 执行流程

1. 构造处理器时，`StaleReadProcessor::new` 尝试锁定会话并调用 `begin_statement`，随后创建尚未求值的 `BaseProcessor`。这里锁失败不会使构造失败；后续实际访问会话时会返回 backend 错误。
2. SELECT 路径进入 `on_select_table`。若 `Session::in_txn`：表上存在 AS OF 立即报错；否则首次访问调用 `evaluate_from_transaction`，后续表访问幂等返回。
3. 事务外 SELECT 为表上的表达式建立 evaluator（无 AS OF 时 evaluator 返回 0）并立即求值。处理器尚未固化时进入统一来源选择；已经固化时只允许新的 `statement_ts` 与首个表一致，否则拒绝混用不同 AS OF。
4. EXECUTE 路径进入 `on_execute_prepared_stmt`。重复执行同一处理器先报 `AlreadyEvaluated`；事务内禁止携带 evaluator，事务外则对传入 evaluator 每次执行只求值一次，并把结果交给统一来源选择器。传入的 prepared evaluator 不会覆盖计划缓存中的 evaluator。
5. `evaluate_from_statement_or_variables` 首先调用 `Session::use_txn_read_ts`（同时留下“已消费”标记），然后按 `statement_ts > txn_read_ts > read_staleness > external_ts > normal read` 决策。语句 AS OF 与非零事务级 ts 同时出现时直接报冲突。
6. 确定非零 ts 后，除复用事务上下文外都通过 `get_session_snapshot_info_schema` 获取相同 ts 的元数据视图；最后 `set_evaluated_values` 原子地固化处理器字段并把 `Session::statement_is_staleness` 设为 `ts != 0`。

## 数据与状态

核心不变量是“一次处理器实例对应一条语句的一次最终判定”。`BaseProcessor::evaluated` 从 `false` 单向变为 `true`；`set_evaluated_values` 拒绝第二次写入。最终状态组合如下：普通读为 `ts=0`、无 `InfoSchema`、无 evaluator；语句 AS OF 和 `txn_read_ts` 为非零 ts、快照 `InfoSchema`、常量或原表达式 evaluator；`tidb_read_staleness` 保存可重新按相对时间计算的 evaluator；external ts 和事务上下文不保存 evaluator。

`statement_ts` 只记录明确的语句 AS OF。首次表没有 AS OF 而因其他来源完成判定后，再出现 AS OF 会与 0 比较并报错；同一个 AS OF 跨多个表可重复求值并通过一致性检查。`InfoSchema.snapshot_ts` 必须对应最终读 ts；事务上下文复用时还将 `local_temporary_tables_attached` 置为 `true`。

会话侧的 `txn_read_ts_used`、`statement_is_staleness` 和语句缓存也会被修改。`use_txn_read_ts` 即便返回 0 也记录消费动作；`begin_statement` 清理缓存；最终固化才更新 `statement_is_staleness`。这些副作用是扩展或重排分支时必须保持的协议。

## 依赖与调用关系

上游方面，Rust 当前可验证调用者是 `pkg/sessiontxn/staleread/processor_test.rs` 和 `externalts_test.rs`；`lib.rs` 将符号公开到 crate 外。Go 对照主链位于 `pkg/planner/core/preprocess.go`：构造 `NewStaleReadProcessor`，AST 遍历表名时调用 `OnSelectTable`，处理 EXECUTE 时调用 `OnExecutePreparedStmt`，再由 `updateStateFromStaleReadProcessor` 把状态传给计划阶段。

下游均通过同 crate 的公开项调用：

- `calculate_as_of_ts_expr`：表达式求值、日期时间/原始 TSO 转换及基础合法性检查；
- `calculate_ts_with_read_staleness`：根据相对偏移计算读 ts；
- `get_external_timestamp`：读取语句缓存后的 external ts；
- `get_session_snapshot_info_schema`：取快照元数据并挂接本地临时表语义；
- `SessionRef = Arc<Mutex<Session>>` 与 `SessionBackend`：承载会话状态及真实存储能力的抽象边界。

`Cargo.toml` 将 crate 的库入口指向 `lib.rs`，并记录 Go 包为 `pkg/sessiontxn/staleread`。其中历史上游依赖均位于 `target.'cfg(any())'.dependencies`，该条件恒假；当前可编译实现实际依赖同 crate 的 `errors.rs`、`util.rs` 等轻量移植类型，而不是直接链接完整 sessionctx/domain/parser crate。这也解释了 Rust `TableName`、`Expression`、`InfoSchema` 是收窄模型，尚未等同于生产 Go 类型。

## 错误处理与边界

- 所有会话锁中毒在实际读写路径映射为 `Error::backend("session lock poisoned")`。构造函数是例外：初始化锁失败被忽略，因此错误延迟到后续入口。
- `set_evaluated_values` 和重复的 prepared 执行返回 `ErrorKind::AlreadyEvaluated`；SELECT 的重复表访问则有专门的幂等/时间一致性分支。
- 活跃事务内设置 AS OF（包括 prepared evaluator）返回 AS OF 错误；事务内新增的 external ts 或会话变量被明确忽略，以免事务读时间倒退。
- 语句 AS OF 与 `txn_read_ts` 不得并存；多个表不得指定不同 AS OF。
- `parse_and_validate_as_of` 传播表达式求值、时间格式、过早 TSO、未来快照和后端错误。注意 `calculate_as_of_ts_expr` 的某些输入路径已经校验过 ts，本函数仍执行最终后端校验，这是现有契约而不是冗余假设。
- external ts 读取错误被转换为 AS OF 错误且保留 message；external ts 为 0 表示未启用有效快照并继续成为普通读。受限 SQL 根本不调用 external-ts 后端。
- 获取快照 `InfoSchema` 或 evaluator 失败时不会调用最终固化函数，因此处理器仍保持未完成状态；相关测试验证 evaluator 错误后 `is_staleness == false`。

## 并发与资源生命周期

`SessionRef` 使用 `Arc<Mutex<Session>>` 共享会话，evaluator 使用 `Arc` 且要求 `Send + Sync`，因此闭包可以被计划缓存或跨线程持有。实现只在读取或更新少量字段时持锁；调用后端、表达式求值及快照元数据获取主要由 `util.rs` 封装，避免本文件在持有会话锁时形成显式长临界区。

处理器自身不实现 `Send` 同步协议，也没有后台任务、通道或显式事务资源；它应按语句创建并可变地顺序调用。`new` 是语句缓存边界，`evaluated` 是实例生命周期边界。持久化 evaluator 时要区分语义：相对 staleness evaluator 每次执行应重新计算，固定 AS OF/事务级 ts 可返回常量，external ts 不得进入 PREPARE evaluator，否则会把一次语句值错误地跨执行复用。

## 与 Go 版本的对应关系

Rust 的 `Processor`、`BaseProcessor`、`StaleReadProcessor`、`parse_and_validate_as_of` 分别对应 `processor.go` 的 `Processor`、`baseProcessor`、`staleReadProcessor`、`parseAndValidateAsOf`。主要优先级、错误消息、多表 AS OF 一致性、事务内限制、prepared evaluator 每次执行求值以及 external ts 不缓存 evaluator均与 Go 实现一致。

明确差异如下：

- Go 直接操作 `context.Context`、`sessionctx.Context`、完整 `ast.TableName`、真实 `infoschema.InfoSchema` 和 `TxnManager`；Rust 通过 `Context`、裁剪的 `TableName/Expression/InfoSchema` 与 `SessionBackend` 桩隔离这些子系统。
- Go 的 `baseProcessor` 提供默认“不支持”的入口方法并通过嵌入复用 getter；Rust 用 trait 实现与私有组合表达同一职责。
- Rust `set_evaluated_values` 额外维护轻量会话的 `statement_is_staleness`，且 `new` 显式重置语句缓存；Go 的语句上下文标记和缓存生命周期由外围 session/planner 设施维护。
- Go 生产接线已由 `preprocess.go` 证明；Rust 仓库搜索没有生产调用边，因此本文不宣称端到端 SQL 已切换到 Rust。

Rust 独立测试 `processor_test.rs` 覆盖普通读、日期时间和原始 TSO、NULL/非法/未来 ts、事务、UNION 多表一致性、各来源优先级、restricted SQL、prepared 执行及缓存边界。Go 的 `processor_test.go` 通过 testkit 与真实 SQL/parser/session 路径覆盖对应行为，尤其提供 Rust 轻量模型尚不能证明的端到端语义。

## 扩展指南

新增一种读 ts 来源时，优先在 `evaluate_from_statement_or_variables` 明确插入优先级，并回答它是否在事务内生效、是否与 AS OF/`txn_read_ts` 冲突、是否应保存 evaluator、是否需要快照 `InfoSchema`。不要绕过 `set_evaluated_values`，否则会破坏一次性求值和 `statement_is_staleness` 同步。

扩展表级语法时修改 `TableName`/`on_select_table`，并保持多表一致性规则；扩展 PREPARE 时修改 `StalenessTsEvaluator`/`on_execute_prepared_stmt`，重点防止把一次执行的 ts 写回可跨执行复用的缓存。若要接入完整 Rust SQL 主链，应在 planner/session 的真实 AST 与会话类型处增加适配，而不是继续扩张本文件的轻量桩模型，并用调用边证明接线。

行为变更必须同步独立的 `pkg/sessiontxn/staleread/processor_test.rs`，external-ts 专项还应同步 `externalts_test.rs`；为保持移植一致性，还应对照 `processor.go` 与 `processor_test.go`。重点回归风险是优先级倒置、事务内时间倒退、相对时间 evaluator 被固化、external ts 被 PREPARE 复用、不同 ts 与 `InfoSchema` 不匹配，以及会话锁临界区扩大造成的性能或死锁风险。Rust 测试逻辑继续放在独立测试文件，不应内嵌进 `processor.rs`。

## 验证依据

- RustCodeGraph：`status` 显示仓库索引包含 `processor.rs`；`node --file pkg/sessiontxn/staleread/processor.rs --offset 1 --limit 400` 返回完整 336 行、符号实现及“used by”摘要；精确查询找到 `parse_and_validate_as_of` 及其三个独立测试。自然语言 `explore` 与部分 callers/callees 命令未输出可用边，因此调用关系又用仓库精确搜索核验。
- 生产源码：`pkg/sessiontxn/staleread/processor.rs`（全部类型、trait、函数和实现）、`util.rs`（`SessionRef`、会话字段、后端抽象和时间戳工具）、`lib.rs`（模块与测试装配）。目标目录没有 `doc.go`。
- crate 声明：`pkg/sessiontxn/staleread/Cargo.toml`（crate 名、`lib.rs` 入口、Go 包映射、恒假条件下的历史依赖声明）。
- Go 对照：`pkg/sessiontxn/staleread/processor.go`（同名状态机和优先级）、`pkg/planner/core/preprocess.go`（生产构造、SELECT/EXECUTE 调用和结果回填）。
- 测试证据：`pkg/sessiontxn/staleread/processor_test.rs`、`externalts_test.rs`、`main_test.rs`，以及端到端 Go 测试 `processor_test.go`。Rust 测试验证状态机和 mock 后端边界；Go 测试验证真实 SQL、parser、session 与 InfoSchema 接线。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定的固定标题命令验证文档存在且恰有 11 个二级章节，并人工复核没有把未接线的 Rust 实现描述为生产主链。
