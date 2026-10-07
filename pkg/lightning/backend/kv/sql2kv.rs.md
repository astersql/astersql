# `pkg/lightning/backend/kv/sql2kv.rs`

## 文件定位

本文件属于 `astersql-lightning-backend-kv` crate，是 Lightning/IMPORT INTO 导入链中“SQL 行值到 TiKV record/index KV”的通用编码层。crate 入口 `pkg/lightning/backend/kv/lib.rs` 以 `pub use sql2kv::*` 对外重导出这里的类型和函数；`pkg/lightning/backend/kv/Cargo.toml` 声明它直接依赖 `encode` 的行/编码器接口、`verification` 的 `KvPair` 与校验和，以及 `tablecodec` 的 canonical 行解码能力。

该文件不是完整的导入控制器。字段映射、SET 表达式和 parser 输入整理位于 `pkg/executor/importer/kv_encode.rs`；本文件从已经形成的 `Datum` 行、`rowID` 和列置换出发，复用 `BaseKVEncoder` 完成类型处理、生成列求值以及 record/index KV 生成。生成后的 `Rows` 可被 `pkg/executor/importer/chunk_process.rs::DataDeliver::deliverLoop` 和 `pkg/dxf/importinto/encode_and_sort_operator.rs::GlobalDataEngineWriter::AppendRows` 等写入路径消费。

## 核心职责

- `tableKVEncoder` 把 `BaseKVEncoder` 包装成 `encode::Encoder` trait object，并用 `closed` 明确拒绝关闭后的编码。
- `Encoder::Encode` 按 `columnPermutation` 从输入行取值；缺列交给 `BaseKVEncoder::ProcessColDatum` 处理默认值、自增或自随机值；随后求值生成列、推进隐式 RowID allocator，再通过 `Record2KV` 产生记录键和索引键。
- `Pairs` 同时实现 `Row` 和 `Rows`，承载一行或一批扁平 `KvPair`；`GroupedPairs` 用有序的 `BTreeMap<i64, Vec<KvPair>>` 按索引 ID 承载分组结果。
- `ClassifyAndAppend` 按 tablecodec 键 discriminator 将记录 KV 与索引 KV 分流，并分别更新 `KVChecksum`。
- 文件还提供 trait object 的安全约定转换、AutoID 辅助函数、可比较 RowID 编码，以及供内部算法测试使用的 Datum 列表编解码。

## 主要符号

- `tableKVEncoder { BaseKVEncoder, closed }`：表级编码器。`BaseKVEncoder` 保存 Session、表定义、列、生成列表达式与 allocator；`closed` 是 Rust 版额外的生命周期状态。
- `NewTableKVEncoder(&EncodingConfig) -> Result<Box<dyn Encoder>, EncodeError>`：公开构造入口；调用 `NewBaseKVEncoder`，把字符串错误包装成 `EncodeError`。
- `GetSession4test`、`GetEncoderSe`：把 `dyn Encoder` 下转为 `tableKVEncoder` 后返回内部 `Session`。前者用带说明的 `expect`，后者只是委托前者。
- `CollectGeneratedColumns`：保留 Go API 形状，但 Rust 版不使用传入的 Session，而是委托 `CollectGeneratedColumnsFromTable(TableDefinition)`。
- `Pairs { Pairs, RowID }`：KV 列表及由 `comparableI64` 生成的 8 字节可比较 RowID。`Size` 只统计 key/value 字节，不把 `RowID` 计入。
- `GroupedPairs(BTreeMap<i64, Vec<KvPair>>)`：稳定按索引 ID 遍历；`Rows2KvPairs` 会按 map 的键序扁平化。
- `MakeRowsFromKvPairs` / `MakeRowFromKvPairs`：把已有 KV 列表装箱为 `Rows` / `Row`；二者都以空 `RowID` 开始。
- `Rows2KvPairs` / `Row2KvPairs` / `ClearRow`：依靠 `Any` 下转恢复或清空 `Pairs`。前两者返回 clone，不转移原容器所有权。
- `IsAutoIncCol`、`GetEncoderIncrementalID`、`GetActualDatum`、`GetAutoRecordID`：暴露列标志、AutoID 映射、单列最终值计算和自增记录 ID 提取。
- `isRecordKey`：只检查固定偏移 `TABLE_SPLIT_KEY_LEN + 1` 是否为 `b'r'`，避免键后部偶然出现 `_r` 被误判。
- `encodeDatumList` / `decodeDatumList`：测试向辅助函数；使用 canonical 旧行格式，解码时要求列 ID/value 成对出现。

## 执行流程

1. 调用方以 `EncodingConfig` 创建 `tableKVEncoder`；`NewBaseKVEncoder` 解析表、Session options、列、生成列和 AutoID 配置。
2. `Encode` 先检查 `closed`。关闭后立即返回 `EncodeError("encoder is closed")`，不会复用 Session 或记录缓存。
3. 编码器通过 `GetOrCreateRecord` 获得记录缓冲。对 `BaseKVEncoder.Columns` 的每个位置，读取 `columnPermutation[index]`；越界、缺失或负数都视作没有输入值。
4. `ProcessColDatum(index, rowID, datum, true)` 完成实际列值处理。任一步失败都会停止本行，并把底层字符串错误转换成 `EncodeError`。
5. 若 `GenCols` 非空，`EvalGeneratedColumns` 在已填充的 record 上求值；失败时用失败列名和原输入行经 `LogEvalGenExprFailed` 生成上下文错误。
6. 当表不是 `pk_is_handle` 时，取 `RowIDAllocType` allocator，并以本行 `rowID`、`allocIDs=false` 执行 `Rebase`，保持后续隐式 RowID 不倒退。
7. `Record2KV(record, row, rowID)` 生成 record/index KV。成功后把 `comparableI64(rowID)` 写入 `Pairs.RowID`，返回 `Box<dyn Row>`。
8. 写入前可调用 `Row::ClassifyAndAppend`：固定位置为 `r` 的键追加到 data 并更新 `dataChecksum`，其余键追加到 indices 并更新 `indexChecksum`。
9. 对全局排序写入器，`Rows2KvPairs` 把 `Pairs` 或 `GroupedPairs` 克隆并扁平化，随后 `GlobalDataEngineWriter::AppendRows` 逐项写出 key/value。

## 数据与状态

`tableKVEncoder` 是可变、带状态对象：`Encode(&mut self, ...)` 会复用 `BaseKVEncoder` 的 record/Session/allocator 状态；`Close(&mut self)` 关闭 Session 并设置 `closed=true`。`Close` 没有返回值，也没有显式幂等保护，但重复调用最终仍保持关闭状态。

`Pairs` 拥有 `Vec<KvPair>` 与 `Vec<u8>`，clone/clear 的成本和生命周期都由 Rust 所有权管理。`ClearRow` 同时清空两者；`Rows::Clear` 消耗 `Box<Self>`、清空后再返回该 box，便于调用方复用容量。与 Go 版不同，Rust `Pairs` 没有 `BytesBuf` / `MemBuf`，因此这里不存在共享 backing buffer 的转交与显式 recycle。

`GroupedPairs` 选择 `BTreeMap` 而不是哈希表，使 `Rows2KvPairs` 的跨索引分组输出顺序确定；每个分组内部仍保持原 `Vec` 顺序。它只满足有限的 `Rows` 形状，不能当作普通可清空、可切块容器使用。

`comparableI64` 将 `i64` 解释为 `u64` 后翻转最高符号位，再输出大端字节；因此字节序与有符号整数自然序一致。`isRecordKey` 的固定偏移建立在 TiDB table key 格式上，对过短键安全返回 `false`。

## 依赖与调用关系

下游依赖如下：

- `encode::{Encoder, Row, Rows, Datum, EncodingConfig, EncodeError}` 定义本文件实现的动态接口和输入数据模型。
- `BaseKVEncoder` 提供真正的列处理与 KV 生成：`NewBaseKVEncoder`、`GetOrCreateRecord`、`ProcessColDatum`、`EvalGeneratedColumns`、`TableAllocators`、`Record2KV`、`getActualDatum`。
- `verification::{KvPair, KVChecksum}` 是输出对象及 record/index 两路校验和。
- `tablecodec::codec::Decode` 与 crate 内 `encodeCanonicalRow` / `fromCanonicalDatum` 支持测试辅助的 Datum 列表往返。

已由 RustCodeGraph 和源码核对的上游关系包括：`pkg/executor/importer/chunk_process.rs::DataDeliver::deliverLoop` 用 `MakeRowsFromKvPairs` 包装数据 KV、用 `GroupedPairs` 包装索引 KV；`pkg/dxf/importinto/encode_and_sort_operator.rs::GlobalDataEngineWriter::AppendRows` 用 `Rows2KvPairs` 写入对象存储 writer；`pkg/lightning/backend/kv/sql2kv_test.rs` 直接覆盖构造、编码、分类与辅助接口。`pkg/executor/importer/kv_encode.rs` 还有更高层的 `TableKVEncoder`，它同样复用 `BaseKVEncoder`，但不是本文件小写 `tableKVEncoder` 的直接包装调用方，两者不要混淆。

## 错误处理与边界

- 构造、列处理、生成列求值、`Record2KV` 走 `Result`；错误在此边界统一表现为 `EncodeError`。生成列错误会附带列名和原输入行上下文。
- `columnPermutation` 比列数短、目标下标为负或目标下标超过输入行长度时均视作缺列，而不是数组越界；后续是否能生成默认/AutoID 值由 `ProcessColDatum` 决定。
- `GetSession4test`、AutoID 辅助函数、`Row2KvPairs` 以及 `ClassifyAndAppend` 的容器参数都要求对象来自本模块的具体实现；传入其他 `Encoder` / `Row` / `Rows` 会 panic。这些是内部类型契约，不是可恢复的用户输入错误。
- `Rows2KvPairs` 只接受 `Pairs` 或 `GroupedPairs`，其他 `Rows` 会 panic；`GroupedPairs::SplitIntoChunks` 和 `GroupedPairs::Clear` 明确 `panic!("not implemented")`。独立测试以 `#[should_panic]` 固化了这两个限制。
- `GetAutoRecordID` 只接受 Float 目标搭配 `Datum::Float`，或 Integer 目标搭配 `Datum::Int/UInt`；其他组合 panic。Float 使用 `round()`，测试覆盖正负 1.6 分别得到 2 和 -2。
- `decodeDatumList` 将 tablecodec 解码错误转为字符串，并拒绝奇数个 canonical 项，防止孤立 column ID 被静默忽略。
- `isRecordKey` 对短键返回 `false`；测试还证明索引键后部嵌入 `_r` 不会被误分类。

## 并发与资源生命周期

本文件没有锁、原子量、通道、异步任务或内部线程。`Encode` 和 `Close` 都要求 `&mut self`，因此同一个编码器的 Session、record 缓冲和 allocator 状态在安全 Rust 中不会被两个调用同时可变访问；若上层需要并行编码，应为 worker 分配独立编码器，或在外部同步。

`NewTableKVEncoder` 创建并拥有一个 `BaseKVEncoder`/Session；`Close` 是显式生命周期终点。关闭后 `Encode` 返回错误，而读取型测试/诊断辅助函数仍能下转并访问其中的 Session。`Pairs`、`GroupedPairs` 和扁平化结果均为拥有所有权的容器；`Rows2KvPairs` / `Row2KvPairs` 会 clone，调用频繁时应计入内存带宽和峰值内存成本。校验和更新与追加发生在同一个同步循环中，不存在“已追加但尚未计入 checksum”的跨线程窗口。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/lightning/backend/kv/sql2kv.go`，主要名称和职责一一保留：`tableKVEncoder`、构造/关闭、生成列收集、`Pairs`/`GroupedPairs`、Rows/Row 转换、编码、AutoID 辅助、大小统计以及 KV 分类。

当前 Rust 实现的可见差异必须在扩展时保留或明确评估：

- Go 构造器接收可选 `metric.Metrics` 并记录 open/close；Rust 构造器没有 metrics 参数或计数逻辑，但增加了 `closed` 检查。
- Go `Pairs` 持有 `BytesBuf`/`MemBuf`，分类时把共享 buffer 转交给 data，`Clear` 时 recycle；Rust 完全依赖拥有所有权的 `Vec`，分类和转换通过 clone 实现。
- Go 的 `GroupedPairs` 是 map，扁平化迭代顺序不保证；Rust 用 `BTreeMap` 得到按 index ID 的确定顺序。两版都故意不实现其切块和清空。
- Go `CollectGeneratedColumns` 从 TiDB expression schema 构建表达式并可返回错误；Rust 表达式已经抽象在 `TableDefinition` 中，函数委托 `CollectGeneratedColumnsFromTable`，不使用 Session 且直接返回 `Vec<GeneratedCol>`。
- Go `Encode` 包含 warning 截断、详细 `LogKVConvertFailed`、显式 `_tidb_rowid` 列处理和 context-aware allocator rebase；Rust 的列/隐式 handle 语义下沉到简化的 `BaseKVEncoder`/`TableDefinition` 模型，错误与 allocator 接口也更轻。不能仅凭同名函数假设两边所有 TiDB SQL 兼容细节已完全等价。
- Go `GetAutoRecordID` 根据完整 MySQL `FieldType` 判断多种浮点/整数类型；Rust 将目标收敛为 `AutoIDFieldType::{Float,Integer}`。

Rust 独立测试覆盖主链行为，但比 Go `sql2kv_test.go` 的严格/非严格 SQL mode、精确错误文本、allocator base 和 buffer recycle 断言更窄；涉及这些语义时应同时回看 Go 测试，不能把现有 Rust 测试数量当成完整对齐证明。

## 扩展指南

- 新增列值规则、默认值或 AutoID 行为时，优先修改 `BaseKVEncoder::ProcessColDatum` / `getActualDatum`；只有编排行级步骤或错误边界变化才修改本文件的 `Encoder::Encode`。同步扩展独立的 `pkg/lightning/backend/kv/sql2kv_test.rs`，不要把测试嵌入生产文件。
- 新增输出键类别时，必须同时审查 `isRecordKey`、`Pairs::ClassifyAndAppend`、两路 checksum 语义以及下游 data/index writer；固定 discriminator 偏移变化还需与 tablecodec 键格式共同验证。
- 若要让 `GroupedPairs` 支持 `Clear` 或切块，应先定义按索引 ID 保序、chunk 大小以及 checksum/写入器预期，再替换两个明确的 panic，并更新对应 `#[should_panic]` 测试。不要只为满足 trait 返回空结果。
- 若要接受新的 `Row`/`Rows` 实现，应把当前 downcast/panic 契约改成显式错误或扩展 trait；同时检查 `Rows2KvPairs` 的 clone 成本和调用方对顺序的依赖。
- 修改编码结果必须做 encode/decode 往返，并覆盖普通索引、唯一索引、row format v1/v2、Null/Bytes/时间、自增缺值、生成列、AUTO_RANDOM/分片 RowID。Go 对照用例是兼容性基线，但 Rust 测试应保持在独立 `sql2kv_test.rs`。
- 引入并行共享前，必须明确 Session、allocator 和 record 缓冲的隔离方式；当前 `&mut self` 是重要的不并发不变量。
- 性能敏感改动重点观察 `Row2KvPairs` / `Rows2KvPairs` / `ClassifyAndAppend` 的全量 clone，以及 `BTreeMap` 扁平化；API 兼容性重点观察 trait object downcast 和 panic 行为。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；目标目录中识别到 `sql2kv.rs` 与独立 `sql2kv_test.rs`。
- RustCodeGraph `node --file pkg/lightning/backend/kv/sql2kv.rs`：核对了全文件 335 行及上述 41 个符号；`callees Encode --file ...` 明确识别 `comparableI64`，`callees ClassifyAndAppend --file ...` 明确识别 `isRecordKey`；`explore` 还识别了 `MakeRowsFromKvPairs`、`Rows2KvPairs` 的导入链调用者。
- 源与 crate 边界：`pkg/lightning/backend/kv/sql2kv.rs`、`pkg/lightning/backend/kv/lib.rs`、`pkg/lightning/backend/kv/Cargo.toml`。
- 直接上游证据：`pkg/executor/importer/chunk_process.rs::DataDeliver::deliverLoop`、`pkg/dxf/importinto/encode_and_sort_operator.rs::GlobalDataEngineWriter::AppendRows`、`pkg/executor/importer/kv_encode.rs::TableKVEncoder`。
- Rust 测试：`pkg/lightning/backend/kv/sql2kv_test.rs` 的 `TestEncode`、`TestDecode`、`TestDecodeIndex`、`TestDecodeUniqueIndex`、`TestEncodeRowFormatV2`、`TestEncodeTimestamp`、`TestEncodeDoubleAutoIncrement`、`TestEncodeMissingAutoValue`、`TestEncodeExpressionColumn`、`TestDefaultAutoRandoms`、`TestShardRowId`、`TestClassifyAndAppend`，以及 record marker 和 `GroupedPairs` panic 回归测试。
- Go 对照：`pkg/lightning/backend/kv/sql2kv.go` 与 `pkg/lightning/backend/kv/sql2kv_test.go`；重点核对编码步骤、buffer 生命周期、metrics、SQL mode 错误、AutoID/allocator 与分类校验和语义。
- 本任务是纯文档分析，按计划未运行 Cargo；最终结构验证要求本文恰好包含上述十一个固定二级标题。
