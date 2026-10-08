# `pkg/util/sqlexec/restricted_sql_executor.rs`

## 文件定位

本文件属于 Cargo crate `astersql-util-sqlexec`，crate 根在 `pkg/util/sqlexec/lib.rs`。根模块以私有模块 `restricted_sql_executor` 挂载本文件，再通过 `pub use restricted_sql_executor::*` 将这里的接口、类型和辅助函数作为 crate 公共 API 导出。`pkg/util/sqlexec/Cargo.toml` 将默认 feature 设为 `formal-crate`；本文件在该 feature 下从 crate 根引入 AST、chunk、执行上下文、日志、解析器、结果字段、系统过程跟踪、错误和会话变量类型。

它不是具体 SQL 引擎，而是位于会话、执行器和需要执行内部 SQL 的上层模块之间的抽象边界。Go 对照文件 `pkg/util/sqlexec/restricted_sql_executor.go` 明确说明：`RestrictedSQLExecutor` 用受限制的内部 SQL 操作系统表并打断包依赖环，`SQLExecutor` 则为普通/内部 SQL 提供更通用的依赖倒置接口。Rust 调用证据包括 `pkg/dxf/framework/storage/*.rs` 大量通过 `sqlexec::ExecSQL` 操作任务元数据，以及 `pkg/extension/extensionimpl/bootstrap.rs` 使用 `DrainRecordSet` 汇聚结果。

## 核心职责

1. 定义 SQL 执行边界：`RestrictedSQLExecutor`、`SQLExecutor`、`SQLParser` 和并发安全的 `Statement` trait 只约定行为，不在这里实现会话、解析器或执行计划。
2. 定义受限执行选项：`ExecOption` 保存快照、统计版本、分区裁剪、会话来源、会话变量临时设置和系统过程跟踪等配置；一组 `ExecOption*`/`Get*Option` 函数构造或应用一次性闭包。
3. 定义结果集协议：`RecordSet` 约定字段、分块读取、分配和关闭，并提供 `Finish`、`TryDetach`、`OnFetchReturned` 的兼容默认行为；`DetachableRecordSet` 与 `MultiQueryNoDelayResult` 描述附加能力。
4. 桥接 chunk 的两种 Rust 所有权：`RecordChunk` 同时容纳直接拥有的 `Box<chunk::Chunk>` 和分配器返回的 `chunk::ChunkRef`，向 `RecordSet` 暴露统一访问方式。
5. 提供消费辅助：`DrainRecordSet` 循环读取所有行，`DrainRecordSetAndClose` 保证尝试关闭，`ExecSQL` 串联 `ExecuteInternal`、Drain 和 Close。

## 主要符号

- `GoError = Box<dyn Error + Send + Sync>`：跨 trait 边界的统一动态错误；`Send + Sync` 允许错误随并发安全接口跨线程传递。
- `RestrictedSQLExecutor`：包含 `ParseWithParams`、`ExecRestrictedStmt`、`ExecRestrictedSQL`。前者返回 AST，后两者返回行和 `resolve::ResultField`；限制策略由实际会话实现负责，本 trait 本身不检查“仅系统表”或递归调用。
- `TrackSysProcFn` / `UnTrackSysProcFn`：开始跟踪可失败，结束跟踪无返回值；二者与 `TrackSysProcID` 一起写入 `ExecOption`。
- `SessionVarsSetup`：先接收可变 `SessionVars` 并返回一个 `FnOnce` 恢复回调。Rust 版恢复回调再次接收同一类会话变量，避免设置闭包长期持有可变借用；`go_merge_34_test.rs` 验证设置和恢复 `SelectLimit`。
- `ExecOption` / `OptionFuncAlias`：`OptionFuncAlias` 是 `FnOnce(&mut ExecOption)`，因此每个选项只能消费一次；`GetExecOption` 从 `Default` 开始按输入顺序调用，后写值覆盖前写值。
- 选项函数：`ExecOptionIgnoreWarning`、`ExecOptionEnableDDLAnalyze`、`ExecOptionAnalyzeVer2`、`GetPartitionPruneModeOption`、`GetAnalyzeSnapshotOption`、`ExecOptionUseCurSession`、`ExecOptionUseSessionPool`、`ExecOptionWithSessionVarsSetup`、`ExecOptionWithSnapshot`、`ExecOptionWithSysProcTrack` 各自只修改对应字段。
- `SQLExecutor`：`Execute` 可返回多个结果集；`ExecuteInternal` 和 `ExecuteStmt` 返回 `Option<Box<dyn RecordSet>>`，其中 `None` 表示语句没有结果集。
- `SQLParser`：复用执行上下文关联的解析能力，返回 AST 列表以及解析警告/错误列表。
- `Statement: Send + Sync`：提供原始/当前/日志文本、执行、prepared/read-only 判断、重建计划及取得 AST；`Send + Sync` 是实现者必须满足的并发契约。
- `RecordChunk`：`Owned` 与 `Allocated` 两个变体；`with_chunk`/`with_chunk_mut` 统一只读和可变访问，`NumRows`、`copy_rows`、`renew` 服务于 Drain。
- `RecordSet`：核心方法为 `Fields`、`Next`、`NewChunk`、`Close`；默认 `Finish` 成功，默认 `TryDetach` 返回 `(None, false)`，默认 `OnFetchReturned` 不做事。
- `DrainError`：同时保存失败前已经复制出的 `rows` 和底层 `source`；其 `Display` 透传源错误，`Error::source` 保留错误链。
- `DrainRecordSet`、`DrainRecordSetAndClose`、`ExecSQL`：本文件仅有的完整控制流函数。

## 执行流程

受限 SQL 的配置流程从调用方构造 `Vec<OptionFuncAlias>` 开始。`GetExecOption` 创建默认 `ExecOption`，依次消费闭包；因此调用顺序具有语义，例如先 `ExecOptionUseCurSession` 后 `ExecOptionUseSessionPool` 的最终值是 `false`。聚合后的配置由 `RestrictedSQLExecutor` 的外部实现解释，本文件不主动执行跟踪、快照切换或会话变量恢复。

`DrainRecordSet` 的流程是：先调用 `RecordSet::NewChunk(None)` 获得首个请求块；循环调用 `Next`；若 `Next` 失败，立即返回含已有行的 `DrainError`；若块的行数为零，视为 EOF 并返回全部行；否则逐行调用 `CopyConstruct` 复制出独立行，再以 `chunk::Chunk::Renew` 和 `maxChunkSize` 创建下一只 owned chunk。复制是必要的，因为下一轮会替换/复用请求块，返回行不能继续借用旧 chunk。

`DrainRecordSetAndClose` 先保存 `DrainRecordSet` 的成功或失败结果，再无条件调用一次 `Close`。关闭失败仅写入后台日志，不替换已经得到的 Drain 结果。

`ExecSQL` 先调用 `SQLExecutor::ExecuteInternal(ctx, sql, args)`。执行错误通过 `?` 原样返回；`None` 直接映射为 `Ok(None)`；有结果集时使用固定最大块大小 1024 Drain。随后无论 Drain 成败都通过 `terror::Call` 尝试 `Close`，关闭错误不覆盖 Drain 结果，最终把成功行包装为 `Some(rows)`。本函数没有实现 Go 注释中提到的重试。

## 数据与状态

`ExecOption` 是一次执行的值对象。派生的 `Default` 使布尔值为 `false`、数值为 0、字符串为空、可选回调/快照为 `None`；这与 Go 零值初始化对应。`AnalyzeSnapshot: Option<bool>` 特意区分“未设置”和“明确 false”。闭包字段没有派生 `Clone`，整个选项集合按所有权传递。

`RecordChunk::Owned` 独占 chunk；`Allocated` 保存 `chunk::ChunkRef`，访问时锁定其互斥量。`copy_rows` 把每行复制到独立 `chunk::Row`，使返回集合不依赖请求块后续的 Renew 或分配器复用。`renew` 无论输入变体为何都生成 `Owned`，所以 allocator 只影响首个 `NewChunk`，Drain 后续批次走 owned 路径。

`RecordSet` 的游标位置、底层执行器和关闭状态都由实现类型维护；本文件只通过 trait 调用，不保存全局状态。`DrainError.rows` 是错误发生前完整读出的批次，不包括失败的 `Next` 可能在请求块中留下的内容。

## 依赖与调用关系

下游依赖由 `pkg/util/sqlexec/Cargo.toml` 和 `lib.rs` 确认：`astersql-parser-ast` 提供 AST，`astersql-parser` 提供 `ParseParam`，`astersql-parser-terror` 提供 `terror::Call`，`astersql-util-chunk` 提供列式数据块/行/分配器，`astersql-planner-core-resolve` 提供字段解析结果，`astersql-sessionctx-variable` 与 `astersql-sessionctx-sysproctrack` 提供会话变量和系统过程跟踪，`astersql-kv` 提供上下文，`astersql-util-logutil` 提供后台日志。

内部调用边为：`GetExecOption -> OptionFuncAlias`；`RecordChunk::NumRows/copy_rows/renew -> with_chunk`；`RecordChunk::with_chunk*` 的 allocated 分支调用互斥锁；`DrainRecordSet -> RecordSet::{NewChunk,Next}`、`RecordChunk::{NumRows,copy_rows,renew}`；`DrainRecordSetAndClose -> DrainRecordSet + RecordSet::Close + logutil::BgLogger`；`ExecSQL -> SQLExecutor::ExecuteInternal + DrainRecordSet + terror::Call + RecordSet::Close`。

RustCodeGraph 将目标文件标记为被 17 个文件使用，并列出 `pkg/executor/adapter.rs`、`pkg/executor/internal/exec/executor.rs` 及相关测试等使用者。文本引用进一步确认：`pkg/dxf/framework/storage/nodes.rs`、`task_state.rs`、`subtask_state.rs`、`history.rs`、`task_table.rs` 和 `pkg/dxf/importinto/scheduler.rs` 调用 `sqlexec::ExecSQL`；`pkg/extension/extensionimpl/bootstrap.rs` 调用 `DrainRecordSet`；`pkg/server/internal/resultset/{cursor.rs,resultset.rs}` 使用 `RecordChunk` 并转发 `TryDetach`/`OnFetchReturned`。

## 错误处理与边界

`RestrictedSQLExecutor` 只是约束接口。Go 注释要求参数占位符是独立实体，不能把 `%?` 与其他字符拼接来期待防注入；Rust trait 的短注释没有复述全部警告，因此调用方仍应遵循 `pkg/util/sqlexec/restricted_sql_executor.go:43-51` 的约束。系统表限制、禁止递归和参数替换安全性必须由实现与调用方保证，不能从 trait 声明推断已强制执行。

`RecordChunk` 的 allocated 访问使用 `expect("chunk allocator mutex poisoned")`；互斥锁中毒会 panic，而不是返回 `GoError`。`maxChunkSize` 不在本文件校验，合法范围由 chunk 实现和调用方承担。

Drain 的主要错误优先级是明确的：`Next` 错误成为 `DrainError.source`，同时保留之前的行；`Close` 错误在 `DrainRecordSetAndClose` 中只记日志，在 `ExecSQL` 中交给 `terror::Call`，都不会替换 Drain 成功或失败。`TryDetach` 的默认 `(None, false)` 表示“不支持/不适合分离”；真正实现若返回错误，Go 对照说明原结果集及会话可能处于未知状态，调用方不应继续使用。

## 并发与资源生命周期

只有 `Statement` 显式要求 `Send + Sync`，即同一语句对象可被多个线程并发使用，执行域本地状态不应藏在共享实现实例中。`RecordSet`、`SQLExecutor` 和 `RestrictedSQLExecutor` 没有 `Send`/`Sync` 超 trait，通常由调用方以可变借用串行驱动；不能据此宣称结果集可并发 `Next`。

`RecordChunk::Allocated` 通过互斥锁保护共享 chunk，锁 guard 只存在于 `with_chunk`/`with_chunk_mut` 回调期间。回调返回后 guard 释放，API 不允许返回对 chunk 内部的借用，从类型上限制了锁外引用逸出。

结果集的正常生命周期是 `NewChunk -> 多次 Next -> 空块 -> Close`。`DrainRecordSet` 本身不关闭资源，适合需要由调用方控制关闭时机的场景；`DrainRecordSetAndClose` 与 `ExecSQL` 才提供“无论 Drain 结果如何都尝试 Close”的保证。`SessionVarsSetup` 返回的恢复闭包必须由受限执行器实现调用，本文件只传递它，不自动恢复。

## 与 Go 版本的对应关系

Rust 的 trait 和字段逐项对应 `pkg/util/sqlexec/restricted_sql_executor.go` 中的 `RestrictedSQLExecutor`、`ExecOption`、`SQLExecutor`、`SQLParser`、`Statement`、`RecordSet`、`DetachableRecordSet`、`MultiQueryNoDelayResult` 及三个辅助函数。主要语义保持一致：选项按序覆盖、空 chunk 表示结束、Drain 后 Renew、关闭错误不覆盖主结果、`ExecSQL` 对无结果语句返回空值。

Rust 为所有权和错误表达增加了几处适配：Go 的 `*bool` 对应 `Option<bool>`；Go 的 `*chunk.Chunk` 被 `RecordChunk::{Owned,Allocated}` 统一；Go 可以同时返回 `rows, err`，Rust 用 `DrainError { rows, source }` 保留部分结果；Go 的 nil `RecordSet` 对应 `Option<Box<dyn RecordSet>>`；Go 的 variadic 参数变为 `Vec<Box<dyn Any>>` 或切片。

存在值得维护者注意的接口差异：Go 基础 `RecordSet` 只有四个方法，Rust 为服务已迁移调用链把 `Finish`、`TryDetach`、`OnFetchReturned` 作为带默认实现的方法加入基础 trait；另有 `DetachableRecordSet` 重复声明分离能力。Go `SessionVarsSetup` 返回无参恢复函数，Rust 恢复函数重新接收 `&mut SessionVars`。Go Drain 直接追加引用当前 chunk 的行表示，Rust 显式 `CopyConstruct`，以满足块更新后的所有权安全。

## 扩展指南

新增受限执行选项时，应同步修改 `ExecOption`、增加只写该字段的 `OptionFuncAlias` 构造器，并在 `pkg/util/sqlexec/migration_aster_unit_test.rs` 扩展“按序应用/后写覆盖”测试；若选项临时修改会话变量，还应扩展 `go_merge_34_test.rs` 的设置与恢复断言，并核对 Go 同名 API。不要把选项效果写进 `GetExecOption`，实际执行/恢复属于 `RestrictedSQLExecutor` 实现。

扩展结果集能力时，先判断它是所有实现必须提供的核心方法，还是像 `Finish`、`TryDetach`、`OnFetchReturned` 一样的可选钩子。改变基础 trait 会影响 `pkg/util/sqlexec/simple_record_set.rs`、`pkg/executor/adapter.rs`、`pkg/server/internal/resultset/*.rs`、`pkg/server/protocol_result.rs` 以及测试 mock；可选能力应给出与 Go 类型断言失败一致的安全默认值。测试逻辑应继续放在独立的 `*_test.rs` 文件，不嵌入生产源文件。

修改 Drain 时必须保留三项不变量：空块才表示正常结束；返回行不能借用会被 Renew/复用的 chunk；Close 错误不能覆盖主要 Drain 结果。对应回归点位于 `pkg/util/sqlexec/migration_aster_unit_test.rs`。若改变 allocator 或锁策略，还需覆盖 `RecordChunk::Allocated` 及互斥锁生命周期，并评估逐行复制的性能成本。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/util/sqlexec` 收录目标 Rust/Go 文件及独立测试；`node --file pkg/util/sqlexec/restricted_sql_executor.rs` 读取 433 行完整源码，并报告 17 个使用文件；`query` 精确定位 Rust/Go 的 `DrainRecordSet`、`DrainRecordSetAndClose`、`GetExecOption` 等对应符号。图的 `callers`/`callees` 命令对这些节点未返回可展示边，因此调用点又用精确文本引用核验，未将空图结果推断为“无调用者”。
- 源码与 crate 边界：`pkg/util/sqlexec/restricted_sql_executor.rs`、`pkg/util/sqlexec/lib.rs`、`pkg/util/sqlexec/Cargo.toml`。
- Go 对照：`pkg/util/sqlexec/restricted_sql_executor.go`，尤其是接口动机、参数安全提示、可分离结果集错误语义和 Drain/Close 实现。
- Rust 独立测试：`pkg/util/sqlexec/migration_aster_unit_test.rs` 覆盖选项顺序、owned/allocated chunk、Drain 部分行、Close 优先级和 `ExecSQL`；`pkg/util/sqlexec/go_merge_34_test.rs` 覆盖会话变量设置/恢复；`pkg/server/internal/resultset/resultset_aster_unit_test.rs` 和 `pkg/server/tests/commontest/cursor_test.rs` 提供 `TryDetach`/`OnFetchReturned` 集成边界证据。
- 代表性上游：`pkg/dxf/framework/storage/*.rs`、`pkg/dxf/importinto/scheduler.rs`、`pkg/extension/extensionimpl/bootstrap.rs`、`pkg/server/internal/resultset/{cursor.rs,resultset.rs}`。
- 本任务是纯文档分析，按任务约束未运行 Cargo；最终只执行固定 11 章节的结构验证。
