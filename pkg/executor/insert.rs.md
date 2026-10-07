# `pkg/executor/insert.rs`

## 文件定位

本文件属于 `astersql-executor` crate；crate 根由 `pkg/executor/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，`pkg/executor/lib.rs:120` 通过 `pub mod insert;` 公开本模块，独立测试则在同一 crate 的 `pkg/executor/insert_test.rs` 中由 `#[path = "insert_test.rs"] mod insert_test;` 装配。

它是 Go `pkg/executor/insert.go` 的 Rust 控制流移植层：定义 INSERT 所需的运行时能力接口，并编排普通 INSERT、`INSERT IGNORE`、`INSERT ... ON DUPLICATE KEY UPDATE`、`INSERT ... SELECT` 以及执行器 `Open`/`Next`/`Close` 生命周期。当前仓库中没有生产代码实现 `InsertRuntime`；代码搜索只找到 `pkg/executor/insert_test.rs:70` 的 `TestRuntime` 实现。因此，本文件目前提供可测试的行为契约和算法骨架，但不能据此认定 Rust SQL 主链已经实例化并使用了该执行器。

## 核心职责

- 用 `InsertRuntime` 把事务、表写入、KV 批量读取、冲突行更新、统计、内存和外键对象等环境能力从算法中抽离（`insert.rs:46-162`）。
- 用 `InsertExec::exec`/`exec_inner` 在 ON DUPLICATE、IGNORE、普通插入三条写路径之间分派，并保证批次结束后的缓冲清理（`insert.rs:170-223`）。
- 为 ON DUPLICATE 路径预取待检查键和冲突旧行，按行 handle、唯一索引、最终插入的顺序解决冲突（`insert.rs:225-350`）。
- 实现执行器生命周期和错误归类：选择 VALUES 或 SELECT 输入路径，特殊处理自增 ID 读取错误，登记运行时统计并转交子 SELECT 的开关（`insert.rs:352-405`）。
- 暴露外键检查/级联列表，以及普通 INSERT 和悲观事务下的重复键检查模式决策（`insert.rs:407-452`）。

## 主要符号

- `DupKeyCheckMode::{Lazy, InPlace}`：普通插入约束检查时机。`Lazy` 把检查推迟到后续事务阶段，`InPlace` 在写入路径立即检查（`insert.rs:28-35`）。
- `PessimisticLazyDupKeyCheckMode::{InPrewrite, InAcquireLock}`：悲观事务已经采用 Lazy 时，进一步指定在预写还是加锁阶段检查（`insert.rs:37-44`）。
- `InsertRuntime`：拥有 13 个关联类型以及写入、预取、错误分类、生命周期、统计和外键相关方法的能力边界。算法只要求 `Key: Clone + Eq + Hash`，错误要求实现 `std::error::Error + 'static`（`insert.rs:46-162`）。
- `InsertExec<R>`：只保存一个 `runtime: R`；所有表、事务、会话和缓存状态均由运行时提供（`insert.rs:164-167`）。
- `exec`/`exec_inner`：批量写入总入口及其内部策略分派（`insert.rs:170-223`）。
- `prefetchUniqueIndices`、`prefetchConflictedOldRows`、`prefetchDataCache`：两阶段预取辅助函数，先读 handle/唯一索引键，再根据索引值解码 handle 并读旧记录键（`insert.rs:225-280`）。
- `batchUpdateDupRows`：ON DUPLICATE 的逐行冲突处理主循环（`insert.rs:282-350`）。
- `Open`、`Next`、`Close`：保持 Go 命名的执行器生命周期方法；文件启用 `#![allow(non_snake_case)]` 以容纳这些移植名称（`insert.rs:23,352-405`）。
- `GetFKChecks`、`GetFKCascades`、`HasFKCascades`：直接投影运行时保存的外键触发器集合（`insert.rs:407-420`）。
- `optimizeDupKeyCheckForNormalInsert`、`getPessimisticLazyCheckMode`：纯函数形式的事务模式决策（`insert.rs:423-452`）。

## 执行流程

1. `Open` 首先重置写入统计；有 ON DUPLICATE 赋值时初始化冲突求值缓冲。有子 SELECT 时打开子执行器，否则仅在赋值不全为常量时初始化普通求值缓冲（`insert.rs:392-405`）。
2. `Next` 清空输出请求并启用行列指标；存在 SELECT 子执行器时调用 `insert_rows_from_select`，否则调用 `insert_rows`（`insert.rs:353-376`）。普通 VALUES 路径发生错误时，终止型 auto-id 错误由 `insert_common::is_terminal_auto_id_error` 原样返回；非终止的自增读取失败且没有 ON DUPLICATE 赋值时，交给运行时补充语句错误上下文。
3. 当运行时把一批行交给 `exec`，`exec_inner` 取得事务、设置 Top SQL 选项、按需开启快照统计，随后累计记录行数和写 CPU 工作量（`insert.rs:177-186`）。
4. 若存在 ON DUPLICATE 赋值，进入 `batchUpdateDupRows`；否则若启用 IGNORE，调用运行时的 `batch_check_and_insert`；其余情况选择普通插入检查模式，并按 `shard_allocate_step().max(1)` 分段，在每段第一行携带剩余范围内的 auto-id size hint，其余行直接 `add_record`（`insert.rs:187-216`）。
5. 分支成功后才调用 `may_flush`；无论分支结果或 flush 是否失败，只要已经进入这段统计生命周期，都会调用 `end_snapshot_stats`。外层 `exec` 随后无条件 `clean_buffers`，并返回原结果（`insert.rs:217-223,170-175`）。事务获取失败发生在快照统计开始之前，但仍会经过外层缓冲清理。
6. ON DUPLICATE 路径先将输入转换为 `CheckedRow`，对非临时表执行两级预取，然后分别取得“更新已有行”和“插入新行”的重复键模式及自增列位置（`insert.rs:283-295`）。
7. 每个待检行优先按行 handle 更新；找不到记录时继续扫描唯一索引。唯一索引给出 handle 后更新旧行；若该 handle 指向的记录不存在，记录索引不一致日志并返回错误。所有冲突键均未命中时才把 `CheckedRow` 转回 `Row` 并插入（`insert.rs:296-347`）。
8. `Close` 登记统计、清零内存记账、设置 INSERT 消息；存在 SELECT 子执行器时关闭它（`insert.rs:379-389`）。

## 数据与状态

`InsertExec` 本身不保存批次缓存，只持有 `R`。`InsertRuntime` 的关联类型把上下文、请求、输入行、带键元数据的待检行、KV 键值、handle、事务、赋值表达式及外键动作参数化；这让本文件可以验证控制流，却把具体 TiKV key 编码、表元数据和会话状态留给运行时（`insert.rs:47-59`）。

预取的第一阶段返回 `HashMap<R::Key, R::Value>`。被标记 ignored 的行不贡献任何键；其他行最多贡献一个 handle 键和多个唯一索引键（`insert.rs:225-241`）。第二阶段跳过临时索引键，避免把不能按普通唯一索引值解码的值传给 `decode_handle_in_index_value`；得到的记录值本身被丢弃，`batch_get` 的作用是让事务/运行时填充读缓存（`insert.rs:243-266`，并与 `insert.go:158-182` 一致）。临时表的数据在内存中，`prefetchDataCache` 直接返回（`insert.rs:269-280`）。

统计状态也由运行时拥有。本文件明确重置写统计、按输入行数计费、包围快照统计、记录检查/插入耗时，并在关闭时登记统计（`insert.rs:61-70,92-94,152-154,179-221,380-383,393`）。独立测试 `insert_resets_write_stats_on_open_and_counts_all_input_rows` 验证一次 Open 重置和三行批次计数，`insert_ends_snapshot_stats_when_flush_fails` 验证 flush 失败仍结束快照统计（`insert_test.rs:298-323`）。

## 依赖与调用关系

模块直接使用标准库 `HashMap`、`Hash` 和计时类型，并调用同 crate 的 `crate::insert_common::is_terminal_auto_id_error`；具体存储、会话、表和表达式依赖都反转到 `InsertRuntime`。`pkg/executor/Cargo.toml` 表明文件归属 `astersql-executor`，crate 具有 `astersql-kv`、`astersql-table`、`astersql-tablecodec`、`astersql-meta-autoid` 等执行器级依赖，但本文件没有直接导入这些 crate，不能把 Cargo 中的全 crate 依赖都视为该文件的实际调用边。

RustCodeGraph 对 `prefetchDataCache` 给出的内部下游边为 `table_is_temporary`、`prefetchUniqueIndices`、`prefetchConflictedOldRows`；后两者继续落到键枚举、handle 解码、record key 构造和 `batch_get`。`exec_inner` 内部直接调用 `batchUpdateDupRows`、`batch_check_and_insert`、`add_record_with_auto_id_hint`/`add_record` 和 `may_flush`。由于泛型 trait 调用及同名 Go/Rust 符号的消歧限制，图的 callers 结果为空或过宽；仓库级补充搜索确认 Rust 侧只有 `insert_test.rs` 构造 `InsertExec` 并实现 `InsertRuntime`，尚无生产调用者。

Go 侧的真实上游是执行器框架对 `Open`/`Next`/`Close` 的调用；`Next` 再进入 `insertRows` 或 `insertRowsFromSelect`，批次最终进入 `InsertExec.exec`。Rust 当前保留了同样的生命周期形状，但没有实现生产 `Executor` trait或运行时适配器，所以这条上游链仅是 Go 对照，不是当前 Rust 接线事实。

## 错误处理与边界

所有可失败操作统一返回 `R::Error`，大多使用 `?` 立即传播。普通插入遇到首个 `add_record` 错误即停止后续行，但仍记录该普通分支耗时、结束快照统计并由外层清理缓冲（`insert.rs:192-223`）。分支失败不会调用 `may_flush`。

ON DUPLICATE 的“记录不存在”是有意区分的控制信号：handle 主键路径的 not-found 表示继续尝试唯一索引；唯一索引已经解析到 handle 后再 not-found 则表示索引与数据不一致，必须记日志并返回错误，而不是静默插入（`insert.rs:299-340`）。`insert_logs_unique_index_pointing_to_missing_row` 对该行为进行回归验证（`insert_test.rs:325-336`）。解码行键、索引值或任何批量读取失败均直接中止。

`Next` 对 auto-id 错误有两层边界：终止型错误不能被语句错误上下文改写；普通 `AutoIncrementReadFailed` 仅在无 ON DUPLICATE 时交给运行时处理。对应测试是 `terminal_auto_id_rebase_error_bypasses_statement_error_context` 和 `ordinary_auto_id_error_still_uses_statement_error_context`（`insert_test.rs:338-366`）。

模式函数的边界是：只要关闭原地检查、事务为悲观或启用流水线，普通插入就选择 `Lazy`；三者都不成立才选择 `InPlace`（`insert.rs:424-434`）。悲观 Lazy 只有在“关闭悲观原地检查 + 显式事务 + 非受限 SQL + 用户连接 ID 大于零”全部满足时才推迟到 Prewrite，否则在 AcquireLock 检查（`insert.rs:437-452`）。

## 并发与资源生命周期

本文件没有线程、异步任务、锁或通道；`&mut self`、`&mut Context` 和 `&mut Transaction` 使单次调用中的运行时及事务访问保持独占。它不声明 `Send`/`Sync`，并发保证取决于未来的生产 `InsertRuntime` 适配器。

事务对象在一次 `exec_inner` 中取得并借给预取、冲突更新和 flush。快照统计在事务建立后开始，在写分支和可选 flush 完成后结束；`insert_ends_snapshot_stats_when_flush_fails` 证明 flush 错误不会漏掉结束动作。需要注意：如果未来在 `begin_snapshot_stats` 之后、汇总为局部 `result` 之前新增带 `?` 的操作，可能绕过 `end_snapshot_stats`，扩展时应维持当前显式收尾结构。

缓冲清理由外层 `exec` 在 `exec_inner` 返回后无条件执行。执行器级资源由 `Open` 初始化/重置、`Close` 登记统计和清零内存，并对 SELECT 子执行器执行成对的 open/close；但 `Open` 中子执行器打开失败时，本文件不自行回滚先前初始化，清理由框架或运行时契约承担（`insert.rs:379-405`）。

## 与 Go 版本的对应关系

Rust `InsertExec<R>` 对应 Go `InsertExec`，但 Rust 把 Go 结构体内嵌的 `InsertValues`、赋值缓冲、优先级、统计及表/会话访问收敛为 `InsertRuntime` 方法。Go `exec` 的三分支、Top SQL 设置、快照统计、行数/CPU 记账、分片 auto-id hint 和 `txn.MayFlush()` 在 Rust `exec_inner` 中都有对应控制流（`insert.go:59-127` 对 `insert.rs:177-223`）。Rust 额外用外层 `exec` 显式保证 `clean_buffers`，对应 Go 的 `defer sessVars.CleanBuffers()`。

预取和冲突更新的顺序与 Go `prefetchUniqueIndices`、`prefetchConflictedOldRows`、`prefetchDataCache`、`batchUpdateDupRows` 一致（`insert.go:129-327`）。Rust 保留了临时表/临时索引跳过、行 handle 优先、唯一索引回退及索引不一致日志语义；具体 `doDupRowUpdate` 中的表达式求值、类型转换、生成列、警告重写、`mysql_insert_id()` 更新等复杂逻辑没有在本文件展开，而是通过 `update_duplicate_row` 能力委托给未来运行时。Go 测试 `TestMySQLInsertID`、`TestInsertNullInNonStrictMode` 和 `TestInsertDuplicateToGeneratedColumns` 展示了这些被委托语义的兼容边界（`insert_test.go:618-759`），Rust 独立测试尚未覆盖这些端到端行为。

`Open`/`Next`/`Close` 和外键访问器与 Go `insert.go:362-420,587-599` 对齐。重复键模式两个 Rust 纯函数把 Go 的 `SessionVars`/`Transaction` 查询改为布尔参数，便于隔离测试；`insert_duplicate_check_modes_follow_transaction_and_pipeline_rules` 覆盖了关键组合（`insert_test.rs:274-296`）。

主要迁移差异是接线完整度：Go 类型直接实现生产 Executor 生命周期并操作真实 TiKV/表/会话对象；Rust 只有测试运行时。另一个可见差异是 Go 记录独立 `Prefetch` 耗时，而当前 Rust trait 只暴露总的 `record_check_insert_elapsed`，没有单独的 prefetch 计时接口。

## 扩展指南

- 接入生产执行链时，应在独立源文件实现 `InsertRuntime` 或建立适配器，并让 `InsertExec` 实现仓库实际使用的 Executor trait；不要把存储/会话细节重新硬编码进本文件。必须为适配器新增同目录独立 `*_test.rs`，不能把测试内嵌进生产源文件。
- 修改普通 INSERT 分派时优先检查 `exec_inner`、`normal_duplicate_key_mode`、`shard_allocate_step` 和 `may_flush`。需维持“首错停止、结束快照统计、外层清缓冲”的不变量，并同步 `insert_test.rs` 的 flush/统计测试。
- 修改 ON DUPLICATE 时以 `batchUpdateDupRows` 为主入口，保持 handle 冲突优先、唯一索引回退、索引悬挂时报错三项顺序；扩展 `update_duplicate_row` 时需对照 Go `updateDupRow`/`doDupRowUpdate`，补齐生成列、警告、auto-id 和同一批次内部重复键测试。
- 修改预取时应保持其只是缓存优化而非正确性前提；临时表和临时索引仍须跳过不适用的存储/解码路径。性能风险集中在键向量及 `HashMap` 内存、重复键未去重和两次 `batch_get` 的 RPC/缓存成本。
- 修改模式选择时同时更新 `optimizeDupKeyCheckForNormalInsert`、`getPessimisticLazyCheckMode` 及 `insert_duplicate_check_modes_follow_transaction_and_pipeline_rules`，并对照 Go 的 session/transaction 条件。错误选择可能把唯一约束错误从语句阶段推迟到锁或提交阶段，属于兼容性和事务语义风险。
- 新增统计或资源时，应在 `Open`/`exec_inner`/`Close` 三处检查生命周期对称性；尤其避免在快照统计开始后新增会提前 `return`/`?` 且绕过收尾的路径。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点和 1,848,419 条边；使用 `node --file pkg/executor/insert.rs` 阅读了全部 452 行，使用 `node --file pkg/executor/insert_test.rs` 阅读了全部 366 行。
- RustCodeGraph 查询：查询了 `InsertExec`、`batchUpdateDupRows`、`prefetchDataCache`、`optimizeDupKeyCheckForNormalInsert`、`getPessimisticLazyCheckMode`，并运行相应 callers/callees。图确认 `prefetchDataCache -> prefetchUniqueIndices/prefetchConflictedOldRows` 等内部边；泛型方法 callers 为空、部分带 ID/限定名的结果发生宽泛消歧，因此没有据此宣称不存在调用，而用仓库搜索补证。
- 已读源码与装配：`pkg/executor/insert.rs`、`pkg/executor/lib.rs`、`pkg/executor/Cargo.toml`。最近的 `pkg/executor/doc.go` 不存在。
- 已读对照与测试：Go 实现 `pkg/executor/insert.go`，Rust 独立测试 `pkg/executor/insert_test.rs`，以及 Go 测试 `pkg/executor/insert_test.go` 中运行时统计、MySQL insert ID、非严格 NULL、生成列重复键等相关用例。
- 补充接线搜索：`rg` 检索 `InsertRuntime for`、`InsertExec<`、`insert::InsertExec` 和两个模式函数，确认当前 Rust 唯一运行时实现及执行器构造均在 `insert_test.rs`；生产 Rust 接线未验证到。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构验证，并人工复核文档是否回答文件存在原因、执行方式和安全扩展位置。
