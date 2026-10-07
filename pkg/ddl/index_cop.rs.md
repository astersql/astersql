# `pkg/ddl/index_cop.rs`

## 文件定位

`index_cop.rs` 属于 `astersql-ddl` crate（`pkg/ddl/Cargo.toml` 的 `[lib] path = "lib.rs"`），并由 `pkg/ddl/lib.rs` 以 `pub mod index_cop` 暴露。它把 Go 文件 `pkg/ddl/index_cop.go` 中“ADD INDEX 回填时通过 Coprocessor 扫描行并转换索引输入”的若干概念，压缩成可独立验证的 Rust 数据模型与纯函数。

当前实现不是完整的 TiKV Coprocessor 客户端，也没有直接驱动 DDL job、schema state、checkpoint 或分布式回填。RustCodeGraph 对本文件公开函数的调用查询只找到测试/基准组合入口；生产 Rust 代码中主要是 `pkg/ddl/index_presplit.rs` 和 `pkg/ddl/index_auto_presplit.rs` 复用 `Datum`。因此它目前更准确的角色是“索引回填语义基础件与移植边界”，而不是已经接入 owner/worker 主链的扫描执行器。

## 核心职责

- 用 `Datum`、`ScanRow`、`Handle` 表达最小的列值、扫描行和行句柄模型。
- 用 `build_table_scan` 模拟左闭右开 key range 扫描，并可选执行 selection 谓词。
- 用 `fetch_table_scan_result` 将扫描结果按固定大小分批交给调用者，同时保持遇错即停。
- 用 `extract_datum_by_offsets` 从一行中抽取句柄列或索引列，并显式报告越界。
- 用 `build_handle` 构造整数句柄或简化的 common handle；用 `get_restore_data` 模拟 common handle 的恢复数据筛选。
- 用 `wrap_in_begin_rollback` 保存 Go `defer se.Rollback()` 的关键清理顺序；用 `complete_error` 给重复键错误补上索引名。

这些职责可由 `pkg/ddl/index_cop_test.rs` 的五组测试直接复核，但它们仅覆盖简化模型，不能替代 Go 文件中的 DAG 构建、TiKV 请求、虚拟列填充、collation 编码和真实事务会话。

## 主要符号

- `Datum::{Null, Int, UInt, Bytes, Text}`：本文件自有的简化值类型，不等同于完整的 `types.Datum`。它派生 `Clone/Eq/PartialEq`，供复制式抽取和测试断言使用。
- `ScanRow { key, columns }`：一条内存扫描记录；`key` 使用 `crate::backfilling::Key`，`columns` 保持表列顺序。
- `Handle::{Int, Common}`：整数句柄或编码后的 common handle 字节。
- `CopError`：封闭错误集合，包括 `InvalidRange`、`ColumnOffset`、`InvalidHandle`、`Transaction` 和 `DuplicateKey`。
- `wrap_in_begin_rollback<T>(start_ts, begin, operation, rollback)`：先 begin，再把 `start_ts` 传给操作；操作结束后调用 rollback，并返回操作本身的结果。
- `build_table_scan(rows, start, end, selection)`：校验范围后，复制满足 `[start, end)` 且通过谓词的行，并返回 `(rows, selection.is_some())`。
- `fetch_table_scan_result(rows, batch_size, consume)`：按 `slice::chunks` 顺序分批调用 consumer。
- `complete_error(error, index_name)`：只改写 `DuplicateKey` 的名字字段。
- `extract_datum_by_offsets(row, offsets, buffer)`：清空并复用 buffer，按 offsets 顺序克隆列值，最终再克隆一份作为返回值。
- `build_handle(datums, common)`：非 common 模式只接受首项 `Datum::Int`；common 模式将每个 Datum 的 Debug 文本按“8 字节大端长度 + 文本字节”拼接。
- `get_restore_data(_target, primary, common)`：非 common 返回空；common 时过滤 `Datum::Null` 后复制其余主键值。参数 `_target` 当前未参与计算。

本文件没有常量、trait、`impl` 块或条件编译项；所有上述类型和函数均为 `pub`。

## 执行流程

测试所组合出的典型简化流程如下（见 `pkg/ddl/export_test.rs::fetch_chunk_for_test` 和 `convert_row_to_handle_and_index_datum`）：

1. 调用 `build_table_scan`，拒绝 `start >= end`，再对内存行执行 key range 与可选 selection 过滤。
2. 调用 `fetch_table_scan_result`，将结果按 `batch_size` 切片并依次送给 consumer；consumer 返回错误时立即停止。
3. 对每一行分别调用 `extract_datum_by_offsets` 取得索引列和句柄列；offset 顺序决定输出顺序。
4. 将句柄列交给 `build_handle`，生成 `Handle::Int` 或简化的 `Handle::Common`。
5. common handle 场景可调用 `get_restore_data`，丢弃以 `Null` 标记为“不需要恢复”的主键列。

若扫描被放在事务清理外壳中，`wrap_in_begin_rollback` 的顺序是 `begin → operation(start_ts) → rollback`。begin 失败时后两步均不运行；operation 成功或失败都会尝试 rollback。

Go 的真实生产流程位于 `pkg/ddl/backfilling_operators.go`：它通过 `wrapInBeginRollback` 取得真实事务时间戳，调用 `buildTableScan` 构建并发送 DistSQL DAG 请求，再循环调用 `fetchTableScanResult` 填充 chunk。Rust 当前没有对应的生产调用边。

## 数据与状态

本文件不保存全局状态，也不持有数据库连接、事务、快照或 checkpoint。所有状态均来自参数或局部变量：

- `ScanRow.key` 按字节字典序参与 `[start, end)` 比较；输入 `rows` 的原始顺序被保留，本函数不排序。
- `build_table_scan` 和 `extract_datum_by_offsets` 都克隆数据，因此返回值与输入/复用 buffer 不共享可变所有权。`index_cop_test.rs::extraction_copies_each_rows_index_data_and_checks_offsets` 验证修改前一次返回值不会污染下一次结果。
- `extract_datum_by_offsets` 会先 `buffer.clear()`；中途遇到越界时，buffer 可能已经含有此前成功抽取的前缀，调用方不应把错误后的 buffer 当作完整结果。
- `fetch_table_scan_result` 自身不缓存进度；consumer 成功处理过的批次不会在后续错误时回滚。
- `get_restore_data` 不修改 `primary`，而 Go 版本会在传入的 `handleDts` 上用 Null 作标记并原地压缩切片。

## 依赖与调用关系

直接 Rust 依赖只有 `crate::backfilling::Key`；其余实现依赖标准库的切片、闭包、`Vec`、`String`、格式化和派生 trait。`pkg/ddl/Cargo.toml` 确认模块位于 `astersql-ddl` crate；本文件自身没有直接使用该 manifest 中的 DistSQL、expression、tablecodec 等依赖，这也说明当前实现尚未承载 Go 版本的真实 Coprocessor 请求。

RustCodeGraph 与源码引用给出的上游关系是：

- `pkg/ddl/lib.rs` 无条件公开 `index_cop`；同文件仅在 `cfg(test)` 下装配 `index_cop_test` 与 `export_test`。
- `pkg/ddl/index_cop_test.rs` 直接覆盖全部七个函数。
- `pkg/ddl/export_test.rs` 组合 `build_table_scan → fetch_table_scan_result`，以及 `extract_datum_by_offsets → build_handle`。
- `pkg/ddl/bench_test.rs` 使用 `extract_datum_by_offsets`；`pkg/ddl/index_presplit.rs`、`pkg/ddl/index_auto_presplit.rs` 使用 `Datum`。
- 对各公开函数执行 RustCodeGraph `callers` 未发现 Rust 生产调用者；这与 `rg` 的模块内引用结果一致。

Go 对照的真实上游是 `pkg/ddl/backfilling_operators.go`，行转换调用则位于 `pkg/ddl/index.go`；错误补全由 `pkg/ddl/rollingback.go` 调用。

## 错误处理与边界

- `build_table_scan` 将空区间和逆序区间统一视为 `CopError::InvalidRange`；边界严格为左闭右开。
- `fetch_table_scan_result` 将 `batch_size == 0` 视为 `InvalidRange`；空 rows 在合法 batch size 下直接成功且不调用 consumer。consumer 的首个错误原样向上传播。
- selection 闭包只返回 `bool`，不能表达过滤计算错误；这比 Go expression/DAG 转换的错误模型更窄。
- `extract_datum_by_offsets` 返回首个越界 offset；重复 offset 合法，并会产生重复 Datum。
- 非 common `build_handle` 要求至少一个 Datum 且首项必须是 `Int`；额外 Datum 会被忽略。common 分支接受空数组并返回空字节句柄，这只是当前代码事实，不代表 TiDB 的合法 common handle。
- `wrap_in_begin_rollback` 把 begin 的字符串错误映射为 `Transaction`；begin 失败不执行 rollback。rollback 错误总被忽略，不覆盖 operation 的成功值或错误，测试明确验证了这一 Go defer 契约。
- `complete_error` 只重写 `DuplicateKey`；其他错误保持身份和值不变。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道或内部共享可变状态；闭包均在调用线程同步执行。`selection` 是借用的 `Fn`，batch consumer 是顺序调用的 `FnMut`，`begin`/`operation`/`rollback` 是各消费一次的 `FnOnce`。

资源生命周期中唯一显式协议是 `wrap_in_begin_rollback`：begin 成功后一定会尝试一次 rollback，即使 operation 返回错误；但它不是 Rust RAII guard，也不处理 panic。如果 operation panic，普通控制流不会到达 rollback，这一点弱于 Go `defer` 在 panic 展开时的行为。该函数也不拥有真实事务，只由调用者闭包保证资源操作的正确性。

批处理只限制每次借给 consumer 的行数，并不会减少 `build_table_scan` 已克隆到完整 `Vec<ScanRow>` 的总内存；因此不能把它描述成流式/背压扫描。

## 与 Go 版本的对应关系

符号意图大致对应，但 Rust 是明显的语义子集：

- Rust `wrap_in_begin_rollback` 对应 Go `wrapInBeginRollback` 的 begin/operation/rollback 顺序和“忽略 rollback 错误”；它没有真实 `sess.Session`、`Txn().StartTS()`、failpoint，也不保留 panic 时 defer 清理。
- Rust `build_table_scan` 在内存中过滤 `ScanRow`；Go `buildTableScan` 调用 `buildDAGPB`，配置 startTS、key range、顺序、并发度、TiKV store、DDL request source，并通过 `distsql.Select`（可带 runtime stats）发请求。Rust 返回的布尔值只表示传入了 selection；Go 的 `conditionPushed` 表示表达式完整转换并实际下推成功，两者并不等价。
- Rust `fetch_table_scan_result` 做 slice 分批；Go 版本调用 `SelectResult.Next` 并填充虚拟列，以空 chunk 表示完成。
- Rust `complete_error` 改写抽象 `DuplicateKey`；Go `completeErr` 专门将函数索引的 invalid-JSON 错误补上索引名。
- Rust `extract_datum_by_offsets` 有越界错误并返回克隆；Go `ExtractDatumByOffsets` 依据 expression 类型把 chunk row 写入预分配 buffer，依赖 offsets/列定义已正确。
- Rust `build_handle` 的 Debug 文本编码不是 TiDB key codec。Go `BuildHandle` 会截断索引值、按 collation 和时区编码 common handle，并经 `errctx` 处理编码错误；整数分支构造 `kv.IntHandle`。
- Rust `get_restore_data` 仅实现 common 开关与 Null 过滤。Go 还要求新 collation、有效 common-handle 版本和主键索引，并按字段类型执行截断与尾随空格计数转换。

Go 集成测试 `pkg/ddl/index_cop_test.go::TestAddIndexFetchRowsFromCoprocessor` 使用 mock store 验证隐藏 row ID、整型聚簇主键和多列 common handle。Rust 测试覆盖相似形状，但仍是内存模型，不能作为真实 Coprocessor 集成等价证据。

## 扩展指南

若只是扩充简化模型，应在对应符号局部修改，并同步独立测试 `pkg/ddl/index_cop_test.rs`，不要把测试写入源文件：

- 新增 Datum 类型或真实编码规则：修改 `Datum`、`build_handle`、`get_restore_data`，增加整数符号、空值、字节串、复合键、排序规则和编码失败用例。
- 改变范围/批处理语义：修改 `build_table_scan` 或 `fetch_table_scan_result`，补充空输入、边界键、逆序范围、零批大小和 consumer 中途失败后的调用次数断言。
- 接入真实回填主链：需要在调用侧建立真实 session/snapshot、DistSQL request、chunk/virtual-column、runtime stats、取消和 checkpoint 协议，而不是继续扩大这里的内存模拟；应以 `pkg/ddl/backfilling_operators.go`、`pkg/ddl/index.go` 和相关 Rust reorg/worker 模块为对照另立实现任务。
- 强化事务清理：若要求 panic/取消安全，应引入明确的 guard 或 unwind 策略，并测试清理次数与错误优先级。

兼容风险主要在 handle 编码与 collation（会影响索引键兼容性）、selection 下推判定（会影响正确性）和 restore data（会影响唯一索引原值恢复）；性能风险主要是全量克隆、Debug 编码和非流式批处理。任何生产接线都应增加独立 Rust 集成测试，并继续与 Go `pkg/ddl/index_cop_test.go` 的三类句柄场景对齐。

## 验证依据

- 源码与模块边界：`pkg/ddl/index_cop.rs`、`pkg/ddl/lib.rs`、`pkg/ddl/Cargo.toml`、`pkg/ddl/doc.go`。
- Rust 测试与组合调用：`pkg/ddl/index_cop_test.rs`、`pkg/ddl/export_test.rs`、`pkg/ddl/bench_test.rs`；生产类型复用：`pkg/ddl/index_presplit.rs`、`pkg/ddl/index_auto_presplit.rs`。
- Go 对照与生产调用：`pkg/ddl/index_cop.go`、`pkg/ddl/index_cop_test.go`、`pkg/ddl/backfilling_operators.go`、`pkg/ddl/index.go`、`pkg/ddl/rollingback.go`。
- RustCodeGraph：`status` 显示目标文件已被索引；`files --filter pkg/ddl/index_cop.rs` 识别 25 个符号；`node --file` 核对了 198 行完整源码；对七个公开函数执行 `query`、`callers`、`callees`，未发现 Rust 生产函数调用边，测试导入与源码引用由后续模块查询交叉确认。
- 人工复核结论：文件存在是为了保存索引回填扫描/转换的最小 Rust 语义；当前运行方式是同步内存函数组合；安全扩展必须区分“简化模型增强”和“真实 Coprocessor 接线”，不能把前者误认为后者。
