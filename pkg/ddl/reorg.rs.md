# `pkg/ddl/reorg.rs`

## 文件定位

`reorg.rs` 属于 `astersql-ddl` crate，并由 `pkg/ddl/lib.rs` 以 `pub mod reorg` 公开。它不是 DDL 状态机入口，而是在线 DDL 进入数据重组阶段后使用的状态与工具层：一部分保存并发回填的运行时计数、告警和进度，另一部分通过 `mysql.tidb_ddl_reorg` 保存可恢复的扫描游标。其生产调用目前集中在 `pkg/ddl/job_worker.rs` 的事务型索引回填，以及 `pkg/ddl/partition.rs` 的分区索引回填参数传递。

本文件同时保留若干与 Go `pkg/ddl/reorg.go` 对齐的辅助 API。仓库内精确调用搜索表明，`ReorgHandler`、`table_max_handle`、`table_range`、`encode_temporary_index_range`、`split_keys_for_temporary_index_ranges` 和 `update_backfill_progress` 尚无非测试 Rust 调用者；它们不能被视为完整 DDL reorg 主链已经接线的证据。`pkg/ddl/job_worker.rs` 还定义了另一个 `ReorgContext`，用于简化的 job 状态机，两者是不同类型，不应混用。

## 核心职责

1. `ReorgContext` 以原子变量和互斥锁汇总多个 backfill worker 的行数、最大进度和 warning，并携带资源组名。
2. `ReorgExpressionContext`、`ReorgRowEncodingConfig` 与 `ReorgTableMutateContext` 提供回填求值和行编码所需的精简上下文；构造时从 `astersql-sessionctx-vardef` 读取 DDL reorg 行格式。
3. `ReorgElement` 与 `ReorgInfo` 描述 job、物理表、元素以及半开扫描区间的当前检查点。
4. `is_reorg_runnable`、范围编码和进度函数实现可独立验证的策略工具。
5. `PersistentReorgHandler` 在独立 SQL 事务中初始化、更新和清理 `mysql.tidb_ddl_reorg`，`restore_reorg` 从该表和 `Job` 共同恢复 owner 接管后的状态。

文件不负责选择 add-index、modify-column 或 partition reorg 的执行后端，也不负责 DDL job 的 schema 状态迁移；这些职责在 `job_worker.rs` 及具体 action 模块中。当前生产接线明确拒绝把 ingest/DXF job 静默降级为普通事务回填，见 `JobWorker::run_transactional_index_backfill`。

## 主要符号

- `ReorgContext`：内部包含 `AtomicI64 row_count`、以 `AtomicU64` 保存的 `f64` 最大进度位模式、`Mutex<BTreeMap<错误码, (文案, 次数)>>` 和公开的 `resource_group_name`。`set_row_count`/`increase_row_count`/`row_count` 负责计数；`merge_warnings`/`take_warnings` 负责告警；`set_max_progress` 用 CAS 保证常规数值下只升不降。
- `SqlMode`、`ReorgExpressionContext`、`new_reorg_expression_context`：严格模式关闭三类 warning 降级，非严格模式开启截断、非法 NULL、除零 warning；时区仅保存 UTC 秒偏移。
- `ReorgRowEncodingConfig`、`ReorgTableMutateContext`、`new_reorg_table_mutate_context`：保存表达式策略、编码开关、可复用缓冲和连接/事务断言/分片分配等属性。`refresh_row_encoding_config` 令新版行格式同时开启 row encoder 与行级 checksum。
- `ReorgElement`：以 `id` 和字节型 `element_type` 标识索引或列；当前索引类型按字面值 `b"_idx_"` 过滤。
- `ReorgInfo`：保存 `job_id`、`physical_table_id`、`[start_key, end_key)`、当前元素、完整元素列表和临时索引合并标志。`element_ids` 只从 `elements` 取索引 ID，不回退到单个 `element`；`update_reorg_meta` 仅推进内存起点。
- `ReorgRunnableError` 与 `is_reorg_runnable`：按 cancelled、paused、非 owner、server shutting down 的固定优先级返回原因。
- `update_backfill_progress`：估计数非正时返回 0，否则将非负行数比例限制到 `[0, 1]`；`merging_temporary_index` 参数目前不改变比率。
- `table_max_handle`、`table_range`：分别计算最大整数 handle，以及从最小 key 到 record-prefix-end 的范围；它们是 Go 存储扫描逻辑的简化纯函数，并不访问 storage。
- `encode_temporary_index_range`、`split_keys_for_temporary_index_ranges`：按 TiDB table/index key 的有符号整数翻转编码生成临时索引区间及 split key，仅处理 `_idx_` 元素。
- `ReorgHandler`：以内存 `BTreeMap<(job_id, element_type, element_id), ReorgInfo>` 增删查句柄；当前没有生产调用者，也不能替代持久化 handler。
- `adjust_end_key_across_version`：当 reorg meta version 为 0 时向 end key 追加零字节，以兼容旧版闭区间语义。
- `PersistentReorgContext`：将 `snapshot_ver`、持久化恢复的 `ReorgInfo` 和新的 `ReorgContext` 聚合给 worker。
- `PersistentReorgHandler::{initialize, restore, stage_update, update, cleanup}`：封装持久化记录生命周期；其中生产回填直接调用 `stage_update`，事务边界由调用者持有。
- `key_hex`、`decode_key_hex`、`reorg_transaction`、`restore_reorg`：负责二进制 SQL 字段的十六进制往返、事务提交/回滚和恢复校验。

## 执行流程

持久化初始化流程由 `PersistentReorgHandler::initialize` 驱动：先生成 version 1 的空 checkpoint JSON，再通过 `reorg_transaction` 开启独立事务；事务中按 job ID 删除旧行，然后插入 job、元素、起止 key、物理表 ID 与 reorg meta。操作成功才提交，任一 begin/SQL/commit 错误都触发 rollback。

owner 恢复流程由 `PersistentReorgHandler::restore` 转入 `restore_reorg`：按 job ID 查询一行，并用 SQL `HEX` 保全任意二进制 key；校验列数和数值字段，解码元素与范围，根据 `job.reorg_meta.Version` 修正旧 end key；随后从 job 恢复 snapshot version、row count 和 resource group。若记录不存在，会先把 `job.snapshot_ver` 置零，再返回 `DDL reorg element does not exist`，让上层走重新初始化/重试边界。

事务型 add-index 回填位于 `JobWorker::run_transactional_index_backfill`。它验证 owner lease、job 状态、`WriteReorganization` schema 状态、action 类型、reorg 元素和事务后端后，循环处理 `[start_key, end_key)`。每批提交索引写入后累加 `reorg.runtime` 的行数与告警；随后另开检查点事务，调用 `PersistentReorgHandler::stage_update` 写 `next_key`，只有提交成功才更新内存 `reorg.info.start_key`。因此数据批次已提交而检查点失败时，下一次会幂等重放该范围，而不会提前发布游标。

`PersistentReorgHandler::update` 是完整的独立事务包装：空起止 key 时直接返回；否则写检查点并提交后才推进内存起点。`cleanup` 只在 `Done`、`Synced`、`Cancelled`、`RollbackDone` 删除记录；暂停、取消中和回滚中的 job 保留恢复点。

## 数据与状态

核心持久状态位于 `mysql.tidb_ddl_reorg`：本文件读写 `job_id`、`ele_id`、`ele_type`、`start_key`、`end_key`、`physical_id` 和 `reorg_meta`。`start_key` 是下一批要处理的位置；`end_key` 与当前版本约定组成半开区间。二进制字段写入时用 `X'…'`，读取时用 `HEX(...)`，避免 session 的 UTF-8 行 ABI 损坏 key。

运行时状态不落盘：行数和最大进度是原子值，warning map 由 mutex 保护；`take_warnings` 会原子地取走并清空整张表。合并 warning 时两个输入 map 任一为空就不处理；同一错误码第一次出现的文案被保留，后续只累加次数，缺失的 count 按零处理。

`ReorgInfo.physical_table_id` 在普通表通常是表 ID，在分区场景是当前 physical partition ID。`elements` 是多元素列表，`element` 是当前/兼容单元素位置。持久化 SQL 当前只保存 `element`，恢复出的 `elements` 为空，这是调用者必须理解的边界。

## 依赖与调用关系

上游生产关系：

- `pkg/ddl/lib.rs` 公开模块，并在测试配置中装入独立的 `reorg_test.rs`。
- `pkg/ddl/job_worker.rs::JobExecutionContext::restore_reorg` 调用 `restore_reorg`；`JobWorker::run_transactional_index_backfill` 持有 `PersistentReorgContext`，合并运行时统计并调用 `PersistentReorgHandler::stage_update`。
- `pkg/ddl/partition.rs::backfill_non_touched_partition_indexes` 接收可变 `ReorgInfo` 并在分区回填间推进物理表和检查点。

直接下游依赖：`crate::backfilling::Key` 定义 key 表示；`astersql-sessionctx-vardef` 提供默认 row format 与 shard allocate step；`astersql-meta-model::group_3::{Job, JobState}` 提供 job 元数据和终态；`serde_json` 构造 checkpoint JSON；`crate::job_worker::DurableJobSession` 提供 begin/query/commit/rollback。上述依赖均由 `pkg/ddl/Cargo.toml` 的 `astersql-ddl` crate 声明或属于 crate 内模块。

精确 Rust 调用搜索还显示，表达式/变更上下文和 runnable 策略主要由 `backfilling_test.rs`、`column_type_change_test.rs` 验证，范围编码与进度工具主要由 `reorg_test.rs` 验证；不能据此推断它们已被生产 action 使用。

## 错误处理与边界

- `PersistentReorgHandler` 和 `restore_reorg` 统一返回 `Result<_, String>`，保留 SQL/session 错误文本，但没有结构化错误类型。
- `reorg_transaction` 在 begin 失败时也调用 rollback；操作或 commit 失败同样 rollback。它不能撤销已经在前一个独立事务提交的数据批次。
- `restore_reorg` 对缺行、列数不是 5、奇数长度/非十六进制 key、非法元素 ID 或 physical ID 明确报错；只读取查询结果第一行，数据库唯一性由表约束/写入协议负责。
- `decode_key_hex` 接受大小写十六进制；`key_hex` 输出小写。SQL 是用格式化数值与本地生成的 hex 构造，不接受任意 SQL 文本输入。
- `Mutex::lock().expect(...)` 在锁中毒时 panic，而非返回业务错误。
- `set_max_progress` 不把值限制在 `[0, 1]`，测试明确允许 `1.25`；需要百分比限制的调用者应先使用其他策略计算。`update_backfill_progress` 才会 clamp。
- `table_range` 对空 handles 返回 `None`；非空时复制并排序，只采用最小 handle，结束位置固定为调用者给出的 prefix end。
- `PersistentReorgHandler::update` 在 start 和 end 同为空时不写 SQL；只有一个为空时仍会写入。

## 并发与资源生命周期

`ReorgContext` 可跨 worker 共享：行数使用 Acquire/Release 或 AcqRel；最大进度通过 `compare_exchange_weak` 循环解决竞争；warning 合并和 drain 在同一 mutex 下串行化。`reorg_test.rs` 用 100 个线程验证最大进度最终为 `0.99`。

持久化游标遵循“提交后发布”不变量。`stage_update` 本身不 begin/commit，这是为了让 `job_worker.rs` 能在同一事务中加入 owner epoch 检查和冲突注入；`update` 才提供独立事务便利接口。worker 每次提交前后校验 owner lease/epoch，防止失去 owner 后的旧任务继续发布检查点。

记录生命周期从 `initialize` 开始，经 `restore`/`stage_update` 多次推进，最终由 `cleanup` 在终态删除。paused、cancelling、rolling-back 中间状态保留记录以支持恢复。内存 `ReorgHandler` 没有持久化、锁或生产接线，只适合单线程拥有或测试用途。

## 与 Go 版本的对应关系

Rust `ReorgContext` 对应 Go `reorgCtx` 的 row count、warning merge 和 max progress 子集；Go 版本还包含 snapshot version、RU、完成通道等更完整状态。Rust 用 `AtomicU64` 保存 `f64` 位模式，Go 使用原子浮点封装，两者的最大进度 CAS 意图一致。

Rust `new_reorg_expression_context` 与 `new_reorg_table_mutate_context` 是 Go 同名逻辑的精简移植。Go 会构造完整 expr context、错误级别、SQL mode flag、时区、随机 RowID shard generator 和 mutate buffers；Rust 只保存本文件消费/测试的字段，因此不是完整 `table.MutateContext` 替代品。

Rust `ReorgInfo` 对应 Go `reorgInfo` 的 job/range/physical table/element 子集。Go 还持有完整 `Job`、job context、DB info、first 标记与动态系统表配置。Rust 的范围/最大 handle 函数也只是纯函数近似，未移植 Go 中通过 storage、snapshot 和 table scan 获取边界的完整行为。

`PersistentReorgHandler` 对应 Go `reorgHandler` 与其 `InitDDLReorgHandle`、`GetDDLReorgHandle`、更新和清理函数。旧版范围兼容规则一致：Go 对 version 0 使用 `endKey.Next()`，Rust 对字节 key 追加 `0`。Rust 初始化的 reorg checkpoint JSON、缺行时清零 snapshot、独立检查点事务和终态清理，均围绕同一 `mysql.tidb_ddl_reorg` 协议。

Go `isReorgRunnable` 还检查 worker/context cancellation，并允许 distributed reorg 跳过 owner 检查；Rust `is_reorg_runnable` 是四个布尔输入的纯策略函数，当前只在测试使用。Go `updateBackfillProgress` 读取统计估值、维持最大值并写 metrics；Rust 函数只计算比例，`merging_temporary_index` 暂未影响标签或指标。

## 扩展指南

- 扩展真实持久化字段时，应同时修改 `PersistentReorgHandler::initialize`、`stage_update`、`restore_reorg` 的 SQL/解码和 Go `pkg/ddl/reorg.go` 协议，并在独立 Rust 测试文件验证旧行兼容；不要只扩展内存 `ReorgHandler`。
- 改变检查点推进时，必须保持“持久化事务 commit 成功后才改 `ReorgInfo.start_key`”，并保留 owner lease/epoch 检查。重点同步 `pkg/ddl/job_worker.rs` 的事务型回填测试和 `pkg/ddl/tests/partition/reorg_partition_test.rs`。
- 新增元素类型时，审查所有硬编码 `b"_idx_"` 的过滤点和临时索引 key 编码，并扩展 `pkg/ddl/reorg_test.rs`；测试逻辑继续放在独立测试文件，不内嵌到源码。
- 扩展 SQL mode、时区或 mutate context 时，应以 Go 的完整 flags/err-level 语义为基准，先补 `backfilling_test.rs` 和 `column_type_change_test.rs`，避免把当前精简结构误当完整会话上下文。
- 若要把 `table_range`、`table_max_handle`、进度或 runnable helper 接入生产，先核对 Go 中 storage scan、pseudo statistics、dist-reorg owner 例外和 metrics 标签，不能直接以当前纯函数替换完整逻辑。
- 并发字段新增时优先明确原子顺序或锁的所有权；warning 错误类型若结构化，应同步修改 merge key、持久/Job warning 转换和 poison/error 策略。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/ddl/reorg.rs` 确认目标文件已索引；`node --file pkg/ddl/reorg.rs --offset 1 --limit 420` 与 `--offset 421 --limit 220` 读取完整 586 行及 63 个符号。限定符 `callers/callees` 未产生可用边，因此用精确 `rg` 补足调用证据。
- 已读源码/装配：`pkg/ddl/reorg.rs`、`pkg/ddl/lib.rs`、`pkg/ddl/job_worker.rs`、`pkg/ddl/partition.rs`、`pkg/ddl/Cargo.toml`。
- 已读契约：`pkg/ddl/doc.go` 与 `docs/agents/ddl/README.md`；后者仅作为导航，本文结论均回查源码和测试。
- Go 对照：`pkg/ddl/reorg.go` 的 `reorgCtx`、mutate context、`updateBackfillProgress`、`isReorgRunnable`、`reorgInfo`、`reorgHandler` 和 `adjustEndKeyAcrossVersion`；`pkg/ddl/reorg_test.go` 验证最大进度的串行与并发行为。
- Rust 测试：`pkg/ddl/reorg_test.rs` 验证进度单调性、warning 合并、元素过滤、临时索引编码和合并阶段比例；`pkg/ddl/ddl_test.rs`、`pkg/ddl/tests/partition/reorg_partition_test.rs` 验证检查点错误、缺失持久句柄、恢复与分区场景；`backfilling_test.rs`、`column_type_change_test.rs` 覆盖表达式、mutate context 和 runnable 分支。
- 人工复核结论：该文件存在的原因是统一 reorg 的运行时统计、范围/元素描述和可恢复 SQL 检查点；真实运行路径由 worker/action 驱动；安全扩展必须维持 Go 协议、半开区间、owner 生命周期和提交后发布检查点的不变量。
