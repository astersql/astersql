# `pkg/executor/replace.rs`

源码：[replace.rs](replace.rs)；独立 Rust 测试：[replace_test.rs](replace_test.rs)；Go 对照：[replace.go](replace.go)。

## 文件定位

本文件属于 `astersql-executor` crate 的 REPLACE 数据修改执行器实现。`pkg/executor/lib.rs` 通过 `pub mod replace` 公开该模块，crate 根由 `pkg/executor/Cargo.toml` 的 `[lib] path = "lib.rs"` 声明。它把 Go 版 `pkg/executor/replace.go` 中依赖会话、事务、表、索引和外键的操作抽象为 `ReplaceRuntime`，并由泛型 `ReplaceExec<R>` 保存运行时。

当前接线状态需要特别区分：模块已经公开，算法主体和独立 Rust 单元测试已经存在；但仓库搜索只发现 `pkg/executor/replace_test.rs` 为 `ReplaceRuntime` 提供实现，没有发现生产实现或构造 `ReplaceExec` 的 Rust 调用者。因此，本文件目前是可测试的移植边界，不能据此宣称 Rust SQL 主链已经实际使用它。RustCodeGraph 将文件列为被 `pkg/session/nontransactional.rs` 使用，但源码级 `rg` 没有找到该文件对 `replace`、`ReplaceExec` 或 `ReplaceRuntime` 的直接引用，故这条图关系不作为已接线证据。

## 核心职责

- `ReplaceRuntime`（`replace.rs:29`）定义 REPLACE 所需的能力端口：打开/关闭 SELECT 子执行器、事务和重复键探测、旧行删除、新行插入、缓存预取、运行时统计、输出消息以及外键触发器访问。
- `ReplaceExec<R>`（`replace.rs:115`）编排 REPLACE 生命周期和“冲突行先删、目标行后插”的算法，而不直接依赖具体会话或 KV 类型。
- `Open`、`Next`、`Close`（`replace.rs:122,136,248`）提供执行器式生命周期；`Next` 在 REPLACE ... SELECT 与常量 VALUES 两条输入路径间分派。
- `exec`、`replaceRow`、`removeIndexRow`（`replace.rs:204,148,184`）完成批量准备、主键/唯一键冲突清理、插入和事务刷新。
- `GetFKChecks`、`GetFKCascades`、`HasFKCascades`（`replace.rs:272-283`）暴露运行时持有的外键检查与级联信息，语义对应 Go 的 `WithForeignKeyTrigger` 方法集。

`priority` 字段（`replace.rs:117`）对应 Go `ReplaceExec.Priority`，但本文件内没有读取它；优先级如何进入事务或调度尚未在 Rust 生产接线中体现。

## 主要符号

`ReplaceRuntime` 是公开 trait，具有 `Context`、`Row`、`CheckedRow`、`Handle`、`Transaction`、`DuplicateKeyCheckMode`、`ForeignKeyCheck`、`ForeignKeyCascade` 和 `Error` 九个关联类型。它将能力分成以下几组：

- 生命周期与输入：`attach_memory_tracker`、`open_select`、`close_select`、`has_select_executor`、`has_child_executor`、`initialize_evaluation_buffer`、`insert_rows_from_select`、`insert_rows`。
- 冲突检测与写入：`handle_key`、`unique_keys`、`decode_row_key`、`transaction_get`、`error_is_not_found`、`fetch_duplicated_handle`、`remove_row`、`add_record`、`keys_need_check`、`optimize_duplicate_key_check`、`may_flush`。
- 事务优化和统计：`transaction`、`set_top_sql_option`、`prefetch_data_cache`、`set_prefetch_duration`、`reset_write_runtime_stats`、`record_write_cpu_work`、快照统计 begin/end 以及记录行数的方法。
- 结果与外键：输出 chunk 重置、RUv2 指标开关、语句消息计数与设置、外键检查/级联切片访问。

`ReplaceExec<R: ReplaceRuntime>` 只保存公开字段 `runtime` 和 `priority`。其公开方法保持 Go 风格命名（文件级 `#![allow(non_snake_case)]`）：`Open`、`Next`、`Close`、`GetFKChecks`、`GetFKCascades`、`HasFKCascades`；算法辅助方法 `exec`、`replaceRow`、`removeIndexRow` 和 `setMessage` 也为 `pub`，但仍受模块路径和泛型运行时约束。

文件没有模块级常量、枚举、条件编译项、`unsafe` 代码或自行启动的异步任务。

## 执行流程

1. `Open` 先调用 `reset_write_runtime_stats`，再挂接内存追踪器。若存在 SELECT 执行器，则把上下文交给 `open_select`；否则初始化 VALUES 表达式求值缓冲（`replace.rs:136-145`）。
2. 执行框架调用 `Next` 时，先清空请求 chunk 并开启 RUv2 行/列度量。存在子执行器时走 `insert_rows_from_select`，否则走 `insert_rows`（`replace.rs:248-256`）。这两个运行时方法应在适当批次上回调或等价执行 `exec`，但当前 trait 没有强制该调用关系，生产实现也尚未出现。
3. `exec` 保存原输入行数，调用 `keys_need_check` 生成带 handle/唯一键信息的 `CheckedRow` 列表，然后取得一个事务（`replace.rs:204-210`）。
4. 若运行时统计开启，`exec` 开始快照统计；随后设置 Top SQL 事务选项，对全部待检查行执行缓存预取，并无论预取成功与否记录预取耗时（`replace.rs:210-227`）。预取失败时会先结束已开启的快照统计，再返回错误。
5. 预取成功后，登记输入记录数和写 CPU 工作量，计算一次重复键检查模式，然后逐行调用 `replaceRow`（`replace.rs:229-239`）。任一行失败都会结束已开启的快照统计并立即返回。
6. `replaceRow` 若有 handle 键，先解码 handle，再查询事务：存在旧行时调用 `remove_row`；返回“行未变化”则整行短路成功，不再插入。not-found 被视为无冲突，其他查询错误直接传播（`replace.rs:155-168`）。
7. 主键路径未短路时，`replaceRow` 循环调用 `removeIndexRow`。后者按 `unique_keys` 顺序查找第一个重复 handle，只删除一次并返回；外层循环因此能逐个清理可能指向不同旧行的多个唯一键冲突。若删除判定新旧行未变化则短路；没有重复项后调用 `add_record`（`replace.rs:170-200`）。
8. 所有行成功后调用 `may_flush`；无论 flush 成功还是失败，都结束已开启的快照统计，并把 flush 结果作为 `exec` 结果（`replace.rs:240-245`）。
9. `Close` 先用 `setMessage` 生成客户端统计消息，再关闭可选 SELECT 执行器，最后登记运行时统计。登记发生在关闭之后，即使关闭返回错误也照常执行（`replace.rs:122-133`）。

## 数据与状态

执行器自身只有 `runtime` 与 `priority`。实际可变状态均由 `ReplaceRuntime` 拥有，包括事务句柄、待检查行、记录/影响/警告计数、内存追踪器、SELECT 子执行器、运行时统计和外键触发器集合。这样的设计方便用 `pkg/executor/replace_test.rs::TestRuntime` 记录事件并替代真实 KV 操作，但也意味着正确性依赖运行时实现遵守隐含协议。

重要状态不变量如下：

- `record_rows` 和 `record_write_cpu_work` 使用的是原始 `rows.len()`，不是 `keys_need_check` 返回的行数（`exec`）。Rust 测试 `exec_counts_input_rows_and_only_closes_enabled_snapshot_stats` 用 3 个输入和 1 个 checked row 固定了这一点。
- 重复键检查模式在一次 `exec` 中只计算一次，并复用于全部 `CheckedRow`。
- `removeIndexRow` 每次最多删除一个冲突行；重复清理由 `replaceRow` 的循环承担，避免在删除后继续使用可能已失效的冲突视图。
- `setMessage` 仅在 SELECT 路径或记录数大于 1 时写入消息；重复数使用 `affected_rows.saturating_sub(records)`，避免 Rust 无符号整数下溢（`replace.rs:259-268`）。Go 版直接执行减法，正常运行时依赖影响行数不小于记录数。
- `priority` 当前仅存储不消费；外键访问器只借用运行时切片，不复制或改变集合。

## 依赖与调用关系

直接 Rust 依赖只有标准库的 `Duration`/`Instant` 和 `astersql_util_chunk::Chunk`。后者由 `pkg/executor/Cargo.toml:146` 以本地路径 `../util/chunk` 声明；本文件没有使用 `nextgen` feature，也没有条件编译分支。

模块装配关系为 `pkg/executor/Cargo.toml` → `pkg/executor/lib.rs` → `pub mod replace`。测试装配由紧邻的 `#[cfg(test)] mod replace_test` 完成，因此测试逻辑保持在独立文件 `pkg/executor/replace_test.rs`，没有内嵌进生产源文件。

已验证的内部调用边包括：`Close → setMessage`；`exec → replaceRow`；`replaceRow → removeIndexRow`；以及上述方法到相应 `ReplaceRuntime` 能力方法的调用。RustCodeGraph 的 `query` 能定位 `ReplaceRuntime`、`ReplaceExec`、`replaceRow`、`removeIndexRow` 等符号，但对这些泛型方法执行 `callers`/`callees` 未输出精确边；源码调用点补充了这部分证据。

仓库级搜索未发现测试之外的 `ReplaceRuntime` 实现和 `ReplaceExec` 构造。因此，上游生产调用者、具体事务类型、表实现和外键执行类型当前均为“未接线/未验证”，不应从 Go 主链反推为 Rust 现状。

## 错误处理与边界

所有可失败操作统一返回 `R::Error`，本文件不包装错误，也不重试。主要边界为：

- handle 解码失败、非 not-found 的事务读取失败、重复 handle 获取失败、删除/插入失败均原样向上传播。
- `transaction_get` 的 `Ok(false)` 与被 `error_is_not_found` 识别的错误都表示没有主键冲突；运行时必须保证该分类准确，否则可能吞掉真实存储错误或错误地中止 REPLACE。
- `fetch_duplicated_handle` 用 `Option<Handle>` 区分“无重复”和查找失败；`remove_row` 的布尔值表示新旧行是否相同，而不是删除是否成功。
- 预取只是优化，但当前实现把预取错误视为整个 `exec` 的错误，与 Go 版一致；不能在扩展时擅自忽略。
- `exec` 对预取、逐行替换和 flush 三条离开路径都配对结束已开启的快照统计；`keys_need_check` 或 `transaction` 在 begin 之前失败，无需结束。若未来在 begin 与现有清理点之间增加新的 `?`，必须同步补清理或引入作用域守卫。
- `Close` 保留子执行器关闭错误，但仍登记统计；`setMessage` 和 `register_runtime_stats` 在 trait 中不可失败。`Open` 在 `open_select` 失败后没有本地回滚，清理由外层框架/运行时负责。
- 空输入仍会取得事务、执行预取、计数、计算重复键模式并 `may_flush`；本文件没有专门的空批短路。

## 并发与资源生命周期

本文件是同步、逐行编排器，没有锁、通道、线程、future 或后台任务。`&mut self`、`&mut Context` 和 `&mut Transaction` 使一次调用中的状态修改串行化；trait 未要求 `Send`/`Sync`，因此不能假设执行器可跨线程共享。

资源生命周期由 `Open → Next（一次或多次，由外部框架决定）→ Close` 表达。内存追踪在 `Open` 挂接，但本 trait 没有显式 detach；SELECT 子执行器仅在存在时打开/关闭。事务局部变量只活到一次 `exec` 结束，`may_flush` 是该批次最后的事务动作，但提交/回滚不属于本文件职责。

快照统计以显式 begin/end 管理，覆盖预取、冲突处理、插入和 flush；它不是 RAII guard，因此新增提前返回点是资源清理风险。预取耗时总会记录；写统计生命周期在每次 `Open` 通过 `reset_write_runtime_stats` 重新开始。`Close` 的“先关闭子执行器、后登记统计”顺序由 `close_registers_stats_after_child_close_even_when_close_fails` 固定。

## 与 Go 版本的对应关系

Rust `ReplaceExec` 对照 `pkg/executor/replace.go::ReplaceExec`，核心流程逐项保持：Open 时初始化写统计和内存追踪；主键冲突先删；唯一键冲突循环删除；无冲突后插入；批量预取后逐行替换并 `MayFlush`；Next 在 SELECT 与 VALUES 间分派；Close 设置消息、关闭 SELECT 并登记统计；外键方法返回检查和级联集合。

主要结构差异来自解耦方式：Go `ReplaceExec` 内嵌 `*InsertValues` 并直接操作 `sessionctx`、`kv.Transaction`、`table`、`chunk` 与外键执行器；Rust 将这些全部放入 `ReplaceRuntime` 关联类型和方法。因此 Go 中 `replaceRow` 每行重新通过 `Ctx().Txn(true)` 取事务，而 Rust `exec` 获取一次事务并把可变引用传给所有 `replaceRow`。在通常由会话返回同一语句事务的前提下意图相同，但生产运行时尚未接线，等价性仍需集成验证。

Rust 还显式化了若干 Go 细节：`Close` 用普通控制流确保关闭失败后仍登记统计；`exec` 在三个错误/返回分支手动结束快照统计；`setMessage` 用饱和减法；运行时增加 `reset_write_runtime_stats` 与 `record_write_cpu_work` 接口。Go 的 trace region、具体 MySQL `ErrInsertInfo` 格式表、allocator runtime-stats context 注入和两类独立 runtime stats 注册，在 Rust 中由抽象运行时合并或没有直接表现。

Rust 单元测试覆盖输入行计数、Open 重置写统计、Close 错误时的统计登记顺序，以及 handle 加多个唯一键冲突的连续删除。Go 测试还覆盖外键、REPLACE ... SELECT、临时/分区表、内存追踪、failpoint 事务限制和物化视图日志等系统行为；这些不能视为当前 Rust 泛型单测已覆盖。

## 扩展指南

- 接入生产主链时，应在独立生产文件实现 `ReplaceRuntime`，明确 `insert_rows[_from_select]` 如何驱动 `exec`，并在构造器/执行器 builder 中创建 `ReplaceExec`。同时增加独立测试文件中的真实事务或集成覆盖，不能只依赖 `TestRuntime`。
- 修改冲突算法优先落在 `replaceRow`/`removeIndexRow`，并同步核对 Go 同名方法。必须保留“每轮删除一个唯一键冲突再重新扫描”的语义、`remove_row == true` 的短路语义和 affected rows 计数约定。
- 新增可能失败的统计覆盖区操作时，应把 begin/end 改造成运行时作用域守卫，或逐条审计所有提前返回；相关回归测试应继续放在 `pkg/executor/replace_test.rs`，不要写入 `replace.rs`。
- 扩展消息或计数时修改 `setMessage` 及 `ReplaceRuntime` 的计数接口，并增加单行、多行、SELECT、warnings、affected < records 的独立测试；同时核对 Go `mysql.ErrInsertInfo` 的兼容格式。
- 扩展外键行为时，访问器仍应只暴露运行时拥有的集合；实际触发顺序应与 `insert_common`/Go `InsertValues` 语义共同验证，并补 `pkg/executor/test/fktest/foreign_key_test.go` 所代表场景的 Rust 测试。
- 性能变化重点审查 `keys_need_check` 的批量化、预取是否仍早于逐行探测、重复键模式是否每批只计算一次，以及唯一键冲突循环是否可能退化。兼容性风险集中在事务身份、错误分类、影响行数和新旧行相同短路。
- 若启用并发批处理，不能直接共享当前 `&mut ReplaceExec`/事务；需要先定义行间冲突、删除顺序、统计合并和事务隔离约束，并证明不改变 Go 的可观察结果。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引包含 11,467 个文件；`node --file pkg/executor/replace.rs --offset 1 --limit 500` 读取了完整 285 行源码；`query ReplaceRuntime`、`query ReplaceExec --kind struct`、`query replaceRow`、`query removeIndexRow` 定位了主要符号及 Go 对照。针对泛型方法的 `callers`/`callees` 查询没有返回精确调用边，因此调用关系又由源码调用点核实。
- Rust 源与装配：`pkg/executor/replace.rs`、`pkg/executor/lib.rs:184-186`、`pkg/executor/Cargo.toml`（crate 名、lib 路径、`astersql-util-chunk` 依赖）。
- Rust 独立测试：`pkg/executor/replace_test.rs`，其中 `TestRuntime` 是仓库内唯一检出的 `ReplaceRuntime` 实现；四个测试分别验证批量计数/统计、Open 重置、Close 错误清理顺序和多类冲突删除。
- Go 对照：`pkg/executor/replace.go` 的 `ReplaceExec` 及同名方法；相关系统行为证据包括 `pkg/executor/test/fktest/foreign_key_test.go::TestForeignKeyOnReplaceIntoChildTable`、`pkg/executor/test/oomtest/oom_test.go::TestMemTracker4InsertAndReplaceExec`、`pkg/executor/executor_failpoint_test.go` 的 REPLACE ... SELECT 事务场景，以及 `pkg/executor/test/writetest/mview_log_write_basic_test.go` 的主键/唯一键冲突场景。
- 接线限制：`rg` 在全部 Rust 文件中仅找到 `pkg/executor/replace_test.rs` 实现 `ReplaceRuntime`，仅找到 `pkg/executor/lib.rs` 和测试引用该模块；因此生产接线标记为未验证，而不是依据设计意图推断已支持。
- 本任务是纯文档分析，按总计划不运行 Cargo。交付结构以任务文件指定的 11 个固定二级标题命令验证，并人工复核“为何存在、如何运行、如何安全扩展”三项问题均可由以上路径和符号追溯。
