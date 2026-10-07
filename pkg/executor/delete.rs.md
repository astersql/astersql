# `pkg/executor/delete.rs`

## 文件定位

[源文件](./delete.rs)位于 `astersql-executor` crate，crate 根为 `pkg/executor/lib.rs`，并由其中的 `pub mod delete;` 公开为 DELETE 执行模块。`pkg/executor/Cargo.toml` 的 `[lib] path = "lib.rs"` 和 `[package.metadata.porting] go-package = "pkg/executor"` 表明它属于 Go `pkg/executor` 的 Rust 移植范围。

当前实现是一个以 `DeleteRuntime` trait 隔离具体执行环境的泛型 DELETE 算法内核：它描述如何从子执行器按 chunk 拉取候选行、选择单表或多表路径、删除记录、处理外键以及维护统计和内存账本。仓库搜索只发现 `pkg/executor/delete_test.rs` 中的 `TestRuntime` 实现 `DeleteRuntime`，没有生产运行时实现或生产代码构造 `DeleteExec<R>`；因此本文件目前可由单元测试验证，但尚无证据表明它已接入 Rust SQL 请求主链。Go 生产路径仍由 `pkg/executor/delete.go` 的具体 `DeleteExec` 承担。

## 核心职责

- `DeleteExec::Next` 重置输出请求，并依据 `DeleteRuntime::is_multi_table` 在单表和多表 DELETE 算法间分派。
- `deleteSingleTableByChunk` 流式消费子执行器 chunk，记录写入 CPU 工作量和 chunk 内存，可按配置分批提交事务；每行经过单表投影、`DELETE IGNORE` 外键预检、handle 构建后进入统一删除路径。
- `deleteMultiTablesByChunk` 先将连接结果按“表 ID + handle”收集到 `TableRowMap`，跳过外连接未匹配行并去重，再由 `removeRowsInTblRowMap` 删除。
- `removeRow` 规定单行副作用顺序：先删存储记录，再执行外键检查/级联，成功后增加 affected rows。
- `Open`、`Close` 和运行时 trait 共同管理子执行器、写入统计、事务刷新和内存账本的生命周期。
- `GetFKChecks`、`GetFKCascades`、`HasFKCascades` 暴露外键触发器信息，供未来的具体运行时或上层执行框架使用。

## 主要符号

- `DeleteChunk<D> { rows, memory_usage }`：一次子执行器拉取的行集合及其内存估算。算法只依赖这两个字段，不绑定具体 chunk 库。
- `TableColumnPosition { table_id, start, end }`：多表连接行中某个目标表的半开列区间 `[start, end)`；同时用于定位 handle 和提取待删行数据。
- `HandleInfoPair<D> { handle_values, position_index }`：保存去重后某个 handle 对应的行值，以及其 `TableColumnPosition` 下标。
- `TableRowMap<H, D>`：两层 `HashMap`，外层键为表 ID，内层键为 handle。它是多表 DELETE 的去重边界。
- `DeleteRuntime`：包含上下文、请求、datum、handle、外键对象和错误六类关联类型，并定义子树拉取、handle 构建、事务切换、内存/统计、记录删除、FK 与开关子执行器等运行时操作。`Datum: Clone` 用于保存多表行切片，`Handle: Clone + Eq + Hash` 用于哈希去重。
- `DeleteExec<R: DeleteRuntime>`：只持有 `runtime: R` 的公开泛型执行器；算法方法均在该类型的 impl 中。
- `onRemoveRowForFK`：公开的 FK 收尾函数；非 IGNORE 模式先执行 delete checks，随后无条件执行 delete cascades。
- 文件没有模块级常量、条件编译项或异步函数；`#![allow(non_snake_case)]` 用于保留 `Next`、`Open`、`Close` 等 Go 风格方法名。

## 执行流程

1. 上层应先调用 `DeleteExec::Open`。它先通过 `reset_write_runtime_stats` 开启新的写入统计生命周期，再调用 `open_child`。
2. `Next` 先调用 `reset_request`，确保 DELETE 不向请求结果中遗留数据；之后按 `is_multi_table` 分流。
3. 单表路径 `deleteSingleTableByChunk`：
   1. 固定目标 `table_id`、extra handle 状态和 batch 配置。
   2. 每次拉取前释放上一 chunk 的内存记账；`next_child_chunk` 返回 `None` 时结束，因此最后一个 chunk 的记账也会在终止拉取前扣回。
   3. 对整个 chunk 调用 `record_write_cpu_work(table_id, rows.len())`，所以被 IGNORE 跳过的候选行也算已处理工作量。
   4. 对每个连接行执行 `filter_single_table_row`。若开启 IGNORE 且 `check_fk_ignore_error` 返回 `true`，直接跳过该行。
   5. 当已成功删除的 `row_count` 达到 batch 大小时，`doBatchDelete` 先 `commit_statement`，再 `new_transaction_in_statement`；新事务错误由 `batch_delete_error` 包装。
   6. `deleteOneRow` 构造 handle；若存在 extra handle，传给存储删除的数据切片排除最后一列。成功删除后才增加 `row_count`。
   7. 每个非空 chunk 处理完调用 `may_flush_transaction`。
4. 多表路径 `deleteMultiTablesByChunk`：
   1. 取得所有 `multi_table_positions`，逐 chunk 维护同样的 chunk 内存借记/归还与事务 `may_flush_transaction`。
   2. `composeTblRowMap` 遍历每个表位置。`unmatched_outer_row` 为真时跳过；否则构造 handle，并把 `[start, end)` 行切片写入该表的 handle 映射。
   3. 同一表、同一 handle 再次出现会覆盖旧的 `handle_values`，但不会再次增加行和 handle 的估算内存；因此保留最新连接行的列值，同时保证一行只删除一次。
   4. 收集结束后，`removeRowsInTblRowMap` 遍历去重结果。它先记录每个候选 handle 的写入 CPU 工作量，再执行 IGNORE 预检，最后调用 `removeRow`。
5. `removeRow` 调用 `remove_record`，随后调用 `onRemoveRowForFK`，两者均成功才 `add_affected_rows(1)`。
6. 上层调用 `Close` 时先保存 `close_child` 的结果，再无条件 `reset_memory_usage`，最后返回子执行器关闭结果；即使关闭失败，内存账本仍被复位。

`pkg/executor/delete_test.rs` 直接验证了上述关键序列：单表 batch/IGNORE/统计/内存流，多表去重、外连接跳过和最新值覆盖，以及 Open/Close/FK 生命周期。

## 数据与状态

`DeleteExec` 自身不缓存行、计数器或事务对象，全部长期状态均由 `runtime` 持有。单次调用中的局部状态包括：单表路径的 `row_count` 与 `previous_chunk_memory`，以及多表路径的 `positions`、`table_rows` 和 `previous_chunk_memory`。

单表处理是 chunk 流式的，除当前 chunk 和单行投影外不累积所有结果。多表处理必须先收集全部唯一目标行再删除，空间复杂度随不同 `(table_id, handle)` 数量增长。`TableRowMap` 基于 `HashMap`，遍历/删除顺序没有稳定保证；正确性不能依赖表或 handle 的处理顺序。

内存账本分为两类：chunk 内存通过“下一轮开始先扣上一块、再加当前块”的方式跟踪；多表去重映射仅在新 handle 首次出现时累计 `estimated_row_memory(joined_row) + handle_extra_memory(handle)`。映射内存不会在局部流程中逐项扣除，而由 `Close::reset_memory_usage` 清零整个执行器账本。测试中的期望序列 `vec![0, 10, -10, 20, -20]` 和多表内存增量验证了这一约定。

affected rows 只在存储删除和 FK 收尾都成功后增加。写入 CPU 统计则按“尝试处理的候选行”记账，包含随后被 IGNORE 跳过的行；这是与 affected rows 不同的指标语义。

## 依赖与调用关系

本文件源码只直接依赖 Rust 标准库的 `HashMap` 和 `Hash`；数据库设施均通过 `DeleteRuntime` 注入。虽然 `astersql-executor` 的 Cargo manifest 声明了 KV、table、session、planner、expression 等大量 crate 依赖，本文件没有直接引用它们，因而不能仅凭 Cargo 依赖推断某个具体适配已经存在。

文件内已经由 RustCodeGraph 源码节点和直接调用确认的主调用边为：

- `Next -> deleteSingleTableByChunk` 或 `Next -> deleteMultiTablesByChunk`；
- `deleteSingleTableByChunk -> doBatchDelete`、`deleteOneRow`；
- `deleteOneRow -> removeRow`；
- `deleteMultiTablesByChunk -> composeTblRowMap -> removeRowsInTblRowMap`；
- `removeRowsInTblRowMap -> removeRow -> onRemoveRowForFK`；
- `onRemoveRowForFK -> foreign_key_delete_checks / foreign_key_delete_cascades`（运行时边界）。

模块装配边为 `pkg/executor/lib.rs -> pub mod delete`；测试装配边为同文件的 `#[path = "delete_test.rs"] mod delete_test`。仓库级 Rust 搜索未发现测试之外的 `DeleteRuntime` 实现或 `DeleteExec` 构造，因此外部生产调用者当前为“未验证/未接线”，不能把 Go 的调用者直接视为 Rust 调用者。

## 错误处理与边界

所有可能失败的运行时动作均使用 `R::Error` 向上传播。子树拉取、handle 构造、事务刷新、FK IGNORE 检查、记录删除、FK 检查/级联和子执行器打开/关闭的首个错误会终止当前流程。`doBatchDelete` 是例外：`new_transaction_in_statement` 的原始错误会经 `batch_delete_error` 转成 batch DELETE 专用错误；`commit_statement` 接口本身不返回错误。

重要副作用边界如下：

- `removeRow` 已调用 `remove_record` 后，FK 收尾仍可能失败；文件依赖外部语句事务在错误时回滚已产生的写入，不在此处自行补偿。
- 非 IGNORE 模式下，FK delete check 失败会阻止 cascade 和 affected-row 增量。IGNORE 模式的候选行通常在调用 `removeRow` 前由 `check_fk_ignore_error` 跳过；若直接调用 `onRemoveRowForFK` 且 runtime 报 IGNORE，则它跳过 checks 但仍运行 cascades，这是该公开函数的精确定义。
- `Close` 无论 `close_child` 成功与否都会清零内存，但保留并返回关闭错误。
- `deleteOneRow` 在 `extra_handle == true` 时执行 `row.len() - 1`，要求行非空；多表路径要求 `start <= end <= joined_row.len()`，`unmatched_outer_row` 和 `build_handle` 也必须能安全访问对应位置；`position_index` 必须仍指向传入的同一份 positions。当前类型未编码这些不变量，违反时可能 panic。
- batch DELETE 只出现在单表路径；多表路径没有分批提交逻辑。batch 大小为零时即使开关开启也禁用分批提交。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁或通道，所有方法都通过 `&mut self` 串行修改同一个 runtime；`DeleteExec<R>` 是否可跨线程取决于具体 `R`，本文件没有声明或保证 `Send`/`Sync`。多表使用普通 `HashMap`，也不是并发容器。

事务生命周期由运行时负责。单表 batch 模式可以在一条语句执行期间多次“提交当前语句事务并开启新事务”；每个 chunk 后还允许 runtime 刷新事务缓冲。多表路径只调用刷新，不切换事务。发生中途错误时，剩余行不再处理，已产生写入如何回滚由外层事务系统决定。

资源生命周期的正常顺序是 `Open -> Next -> Close`。`Open` 重置写入运行时统计并打开子执行器；执行中记账当前 chunk 和多表映射；`Close` 关闭子执行器并无条件复位内存。`delete_open_resets_write_runtime_stats` 与 `foreign_key_and_lifecycle_hooks_match_delete_contract` 分别验证统计重置和“关闭失败仍清内存”。

Go 测试 `pkg/executor/delete_test.go::TestDeleteLockKey` 还验证了真实悲观事务中的锁行为，但 Rust 泛型单元测试没有并发存储适配，因此该并发/锁语义不能视为 Rust 当前实现已经端到端验证。

## 与 Go 版本的对应关系

Rust 的方法布局和主要分支对应 `pkg/executor/delete.go`：`Next`、`deleteOneRow`、单/多表 chunk 路径、`doBatchDelete`、映射收集/删除、`removeRow`、`onRemoveRowForFK`、Open/Close 及三个 FK 查询方法均有直接同名来源。核心语义也保持一致：单表可 batch、多表按 handle 去重、外连接未匹配行跳过、IGNORE 在删除前做 FK 预检、删除后处理 FK 并增加 affected rows。

两者的实现层级不同：

- Go `DeleteExec` 直接嵌入 `BaseExecutor`，持有表对象、列位置信息、内存 tracker、FK maps 和写入统计，并调用具体的 `exec.Next`、事务、`table.RemoveRecord` 等设施；Rust 将这些能力全部抽象为 `DeleteRuntime`，当前只有测试 runtime。
- Go 单表路径显式校验 schema columns 与返回字段数量；Rust 把单表投影交给 `filter_single_table_row`，本文件没有对应的显式长度校验。
- Go 的多表映射使用 `kv.MemAwareHandleMap` 并计入 `Set` 返回的映射内存变化；Rust 使用标准 `HashMap`，只显式累计估算行内存和 handle 额外内存。两者的内存统计数值模型不完全等价。
- Go Open 创建并挂载 `memory.Tracker`，按需创建 `WriteRuntimeStats`；Rust Open 只调用运行时 hooks。Go Close 注册统计并 defer 清空 tracker；Rust 对应行为必须由 runtime hooks 实现。
- Go 的 handle、datum、FK 和错误都是具体 TiDB 类型；Rust 通过关联类型表达，算法覆盖得到验证，但真实类型集成尚未验证。

测试对应关系也不同。`pkg/executor/delete_test.rs` 是算法级独立单元测试，覆盖 batch、IGNORE、统计、内存、去重、外连接和生命周期；`pkg/executor/delete_test.go` 是 SQL/事务级测试，覆盖悲观锁键和单表/多表/batch DELETE IGNORE 的真实 FK 告警。Rust 测试没有复现 Go 的端到端锁等待、SQL warning 文本或真实事务回滚。

## 扩展指南

新增或修改 DELETE 行为时，应首先判断责任位于算法还是环境适配：

- 修改单表逐行策略、batch 时机或 IGNORE 分支，接入 `deleteSingleTableByChunk`，并在 `pkg/executor/delete_test.rs` 增加独立测试，至少断言删除集合、提交/新事务次数、CPU 统计、affected rows 和内存增减序列。
- 修改多表去重、外连接处理或行布局，接入 `composeTblRowMap` / `removeRowsInTblRowMap`。必须保持 `(table_id, handle)` 唯一性，明确重复 handle 是保留最新值还是首值，并测试多个表、重复行、未匹配外连接及非法位置边界。
- 新增存储删除或 FK 副作用，应接入 `removeRow` / `onRemoveRowForFK`，保持“删除记录 -> FK -> affected rows”的成功顺序；同时覆盖 IGNORE 与非 IGNORE、check/cascade 失败及事务回滚责任。
- 增加运行时能力时扩展 `DeleteRuntime`，并同步所有实现。当前仓库只有 `TestRuntime`，未来生产适配应放在生产源文件，不要把实现塞入 `delete.rs` 的内嵌测试；测试继续放在同目录独立的 `delete_test.rs`。
- 若接入真实 Rust executor 主链，需要新增生产 `DeleteRuntime` 适配和构造点，并用调用图确认上游 builder/Executor 接线；还应补真实 session/KV 集成测试，尤其覆盖 Go `TestDeleteLockKey` 和 `TestDeleteIgnoreWithFK` 的锁、告警、回滚与多表行为。

兼容性风险集中在 handle 构造与行切片、IGNORE/FK 副作用顺序、batch 的事务边界和多表无序遍历；性能风险集中在多表 DELETE 全量物化、重复 clone、HashMap 容量及内存估算偏差。任何优化都不应以零测试或仅编译成功替代上述行为证据。

## 验证依据

- RustCodeGraph：`status` 显示本仓库已索引 11,467 个文件；`node --file pkg/executor/delete.rs --offset 1 --limit 500` 返回完整 348 行源码及“used by 1 file: `pkg/executor/delete_test.rs`”；`query` 确认 `DeleteExec`、`DeleteRuntime`、`DeleteChunk`、`TableColumnPosition`、`HandleInfoPair`、`TableRowMap`、`deleteSingleTableByChunk`、`deleteMultiTablesByChunk`、`composeTblRowMap`、`removeRowsInTblRowMap`、`removeRow`、`doBatchDelete` 和 `onRemoveRowForFK`。`callers/callees` 查询在当前索引上超时，故外部接线结论改由模块与仓库引用搜索核对，并明确标为未生产接线。
- Rust 源与装配：`pkg/executor/delete.rs`、`pkg/executor/lib.rs`、`pkg/executor/Cargo.toml`。`lib.rs` 公开 `delete` 模块并以 `#[path = "delete_test.rs"]` 注册独立测试；Cargo 确认 crate 名、根文件、Go 包映射与 crate 边界。
- Rust 测试：`pkg/executor/delete_test.rs`。四个测试分别覆盖单表 batch/IGNORE/统计/内存、Open 统计重置、多表去重/外连接/最新值、FK 与 Close 错误清理。
- Go 对照：`pkg/executor/delete.go`、`pkg/executor/delete_test.go`。源码核对实际生产 executor 语义；`TestDeleteLockKey` 与 `TestDeleteIgnoreWithFK` 提供悲观锁和真实 FK/告警/batch 行为证据。
- 仓库引用搜索：`rg` 只在 `pkg/executor/delete_test.rs` 找到 `impl DeleteRuntime` 和 Rust `DeleteExec` 构造，因此文档没有声称存在未找到的生产适配。
- 本任务为纯文档分析，按任务约束未运行 Cargo 或代码测试；交付验证仅执行固定 11 章节结构检查和文档差异自审。
