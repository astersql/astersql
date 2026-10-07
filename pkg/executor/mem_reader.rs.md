# `pkg/executor/mem_reader.rs`

## 文件定位

本文件属于 `astersql-executor` crate；crate 由 `pkg/executor/Cargo.toml` 定义，并在 `pkg/executor/lib.rs` 以 `pub mod mem_reader` 公开。它承载 UnionScan 的内存侧读取算法：把当前事务尚未提交的 KV 变更与临时表或缓存快照合并，再按表扫、索引扫、IndexLookUp 或 IndexMerge 的形态产出逻辑行。这里的“内存表读取”是事务写缓冲读取，不是 `pkg/executor/memtable_reader.rs` 所处理的 `information_schema` 集群虚拟表。

RustCodeGraph 对四个公开构造器的调用分析只发现本文件内部调用和 `pkg/executor/mem_reader_test.rs` 的测试调用；未发现生产 Rust 执行器调用。因此当前文件是已经公开并可独立测试的移植模块，但尚无证据证明它已接入 Rust SQL 执行主链。Go 的实际执行接线仍位于同路径 `pkg/executor/mem_reader.go`，由 `UnionScanExec` 及其子读取器提供真实会话、事务和表达式上下文。

## 核心职责

1. 通过 `MemReaderBackend` 把事务快照、临时/缓存快照、KV/行/句柄解码、条件求值、排序比较和句柄到范围转换隔离在后端边界。
2. `txnMemBufferIter`、`getSnapIter` 与 `union_range` 合并一个或多个半开区间 `[start, end)`：临时快照优先于缓存快照，事务侧同键值覆盖快照侧值，空值作为删除标记由上层遍历跳过。
3. `memTableReader` 和 `memIndexReader` 分别解码记录 KV 与索引 KV；二者既支持流式 `memRowsIter`，也在必须额外排序时物化全部结果。
4. `memIndexLookUpReader` 先从索引提取 `Handle`，转换成表记录范围后回表；`memIndexMergeReader` 对多个局部表扫/索引扫结果做句柄并集或交集，再统一回表。
5. 处理整型主键、Common Handle、分区包装句柄、全局索引分区过滤、物理表 ID 投影和缺失列补值等兼容语义。

## 主要符号

- 基础数据模型：`Datum`、`Handle`、`KeyRange`、`KvPair`、`FieldType`、`ColumnInfo`、`IndexInfo`、`TableInfo` 是本移植层的轻量类型；`Row = Vec<Datum>`，`Value` 为空表示删除标记。
- `MemReaderError`：区分 `Backend`、`Decode`、`Compare`、`Unsupported` 与 `Closed`。当前迭代器关闭后只令 `Valid` 返回 `false`，并不会主动产生 `Closed`。
- `MemReaderBackend: Send + Sync + 'static`：本文件唯一外部能力入口。所有存储访问、编码细节、表达式语义和行比较都由实现者提供，读取器共享 `Arc<B>`。
- `UnionScanSpec<B>`：把后端、表与投影列、过滤条件、升降序、保序要求、比较策略、物理表 ID 列位置和允许的分区集合聚合为构造参数。
- `compareExec`：`desc` 控制方向；`needExtraSorting` 表示范围迭代本身不足以保证目标顺序，需要物化排序。
- `memReader`：仅暴露 `getMemRowsHandle`，供 IndexLookUp/IndexMerge 复用表扫或索引扫的句柄提取能力。
- `memIndexReader` / `buildMemIndexReader`：索引读取与构造；关键方法为 `getTypes`、`decodeIndexKeyValue`、`getMemRows`、`getMemRowsIter` 和 `getMemRowsHandle`。
- `memTableReader` / `buildMemTableReader`：表记录读取与构造；关键方法为 `decodeRecordKeyValue`、`getRowData`、`getMemRows`、`getMemRowsIter` 和 `getMemRowsHandle`。
- `memIndexLookUpReader` / `buildMemIndexLookUpReader`：索引句柄到表行的两阶段读取器。其自身的 `getMemRowsHandle` 明确返回 `Unsupported`。
- `PartialMemReader`、`memIndexMergeReader` / `buildMemIndexMergeReader`：IndexMerge 的局部路径枚举与交/并集合并器。合并器自身同样不支持再提取句柄。
- `txnMemBufferIter`：跨多个 `KeyRange` 延迟加载当前范围的合并结果，保存范围下标、当前物化 KV、游标、延迟错误和关闭状态。
- `memRowsIter` 及 `defaultRowsIter`、`memRowsIterForTable`、`memRowsIterForIndex`：统一逐行 `Next`/`Close` 协议。
- 辅助函数：`iterTxnMemBuffer`、`getSnapIter`、`getColIDAndPkColIDs`、`hasColVal`、`handle_datum` 和 `sort_rows`。

## 执行流程

表扫描从 `buildMemTableReader` 开始：构造列 ID 到投影偏移的映射，准备 Common Handle 主键列 ID 和解码缓冲，并在降序时反转范围顺序。`getMemRowsIter` 若要求保序且 `needExtraSorting` 为真，就调用 `getMemRows` 全量解码、过滤、排序后返回 `defaultRowsIter`；否则创建 `txnMemBufferIter`，由 `memRowsIterForTable::Next` 边扫描边调用 `decodeRecordKeyValue` 和后端条件求值。

每个表 KV 先由 `decode_row_handle` 取得句柄，再由 `decode_row` 取得已编码在 value 中的列。`getRowData` 对缺列逐项补齐：Common Handle 主键列通过 `decode_common_handle_column` 解出相应分量；整型主键或额外句柄列从 `Handle` 生成有符号/无符号 `Datum`；其他列通过 `default_column_value` 补值。最后 `decodeRowData` 严格按请求列顺序组装行，缺失列返回 `Decode` 错误。

索引扫描由 `buildMemIndexReader` 构造。`getTypes` 按“索引列 + 句柄列”生成解码类型：句柄可能是整型主键、Common Handle 的多个主键列，或普通隐藏整型句柄。扫描全局索引时先用 `decode_partition_id` 过滤不在 `partitionIDMap` 的项；`decodeIndexKeyValue` 再按 `outputOffset` 投影，并在 `physTblIDIdx` 指定的位置插入物理分区 ID。句柄提取还修正 Common Handle 表被解为 `Int` 的兼容情况，并保留或过滤分区包装。

`txnMemBufferIter::Valid` 在当前段耗尽时调用 `load_next_range`。`union_range` 先把临时表快照（不存在时才用缓存快照）写入 `BTreeMap`，再写入事务快照，所以同键以事务侧为准；映射自然按键升序，反向扫描再整体反转。`iterTxnMemBuffer` 与两个流式行迭代器都在推进游标后检查值，空值直接跳过，从而让事务删除遮蔽快照旧值。

IndexLookUp 对每个 `GroupedRanges` 调用索引读取器收集句柄，用 `table_handles_to_ranges` 生成记录范围；没有分组时使用表 ID 和索引读取器原范围。无句柄立即返回空迭代器，否则按方向整理范围并委托新建的 `memTableReader` 回表。

IndexMerge 对每条局部路径逐分组替换范围并收集句柄。分区模式下，普通句柄被包装为 `Handle::Partition`。`BTreeMap<Handle, usize>` 去重并计数：并集保留所有出现过的句柄，交集只保留计数等于局部读取器数量的句柄。随后把句柄转换成表范围、回表读取；`keepOrder` 为真时在最终结果上排序。

## 数据与状态

`KeyRange` 使用半开区间；范围顺序与每段内部方向共同决定扫描次序。`BTreeMap` 同时承担快照覆盖合并、句柄计数和稳定键序，`BTreeSet` 保存合法分区 ID。`Handle` 的 `Partition { partition_id, inner }` 使相同行标识在不同物理分区中仍可区分。

读取器保存 `addedRows` 等物化状态；`getMemRows` 会清空索引读取器的旧结果，但表读取器直接向局部 `rows` 收集并覆盖 `addedRows`。`addedRowsLen` 只用于索引句柄向量的容量预估，本文件没有更新它的逻辑，需由未来接线方正确初始化或接受零容量起步。

`allocBuf` 当前保存预分配的 `handleBytes` 与 `decoded` 映射，但 Rust 解码实际委托给后端，字段本身未参与热路径；这是与 Go `rowcodec` 解码缓存对应的迁移结构，不应据此宣称已获得同等的分配优化。

`txnMemBufferIter` 一次把一个范围的合并 KV 物化到 `curr`，而非持有底层存储迭代器；内存峰值至少与最大单范围 KV 数量成正比。额外排序路径及 IndexMerge 会进一步物化全部行或句柄。

## 依赖与调用关系

上游逻辑入口是四个公开构造器。文件内调用边为：`buildMemIndexLookUpReader -> buildMemIndexReader`，`buildMemIndexMergeReader -> buildMemIndexReader/buildMemTableReader`；表/索引读取器的流式和物化路径都汇入 `newTxnMemBufferIter` 或 `iterTxnMemBuffer`，再汇入 `union_range -> getSnapIter` 与 `MemReaderBackend`。

RustCodeGraph 未找到这些构造器的生产 Rust 调用者；唯一明确外部调用是 `pkg/executor/mem_reader_test.rs` 对 `buildMemTableReader` 的测试。因此应用主链位置只能按设计描述为 UnionScan 内存侧候选实现，当前接线状态必须标为未完成/未验证。`pkg/executor/lib.rs` 的公开模块声明只证明 API 可见性，不证明执行时可达。

Rust 源码仅直接依赖标准库的 `Arc`、`Ordering`、`BTreeMap`、`BTreeSet` 和格式化 trait；存储、编解码与表达式依赖被抽象进 `MemReaderBackend`。`pkg/executor/Cargo.toml` 声明 crate 名为 `astersql-executor`、库入口为 `lib.rs`、`nextgen` feature 与 dxf-importinto 相关；本文件没有条件编译项，也没有直接使用该 feature。

Go 主链的对应依赖更具体：`sessionctx.Context` 激活事务，`kv.MemBuffer` 与临时表数据提供迭代器，`transaction.NewUnionIter` 合并视图，`tablecodec`/`rowcodec` 解码，`expression.EvalBool` 求值，`distsql.TableHandlesToKVRanges` 回表。Rust 后端实现若要真正接线，必须完整承接这些语义，而不能只返回测试用内存向量。

## 错误处理与边界

所有后端操作返回 `MemReaderError` 并以 `?` 原样向上传播。显式边界包括：索引投影越界和投影列缺失返回 `Decode`；IndexLookUp/IndexMerge 的 `getMemRowsHandle` 返回 `Unsupported`；后端快照、解码、条件、比较和范围转换错误中止当前操作。

`txnMemBufferIter::Valid` 的签名不能返回错误，因此加载范围失败时把错误存入 `err` 并暂时返回 `true`；调用方必须按既定顺序在读取当前项前后调用 `Next`，由 `Next` 取出并返回延迟错误。本文件的两个流式迭代器和 `iterTxnMemBuffer` 都遵循这一顺序。绕过协议直接在错误状态调用 `Key`/`Value` 可能发生越界 panic，这是该低层接口的使用前置条件。

`sort_rows` 因 `slice::sort_by` 比较器不能返回 `Result`，会暂存最后一次比较错误，同时把该次比较视为 `Equal`；排序完成后再返回错误。调用方不得使用出错后的部分排序结果。空值删除标记不会传给解码或条件回调。

交集算法以“每个局部读取器都出现”为条件，但计数按每次扫描出现累加；其正确性依赖单个局部路径不会对同一逻辑句柄重复计数，或上游范围互不重复。扩展范围组织方式时需要保住这一不变量。

## 并发与资源生命周期

本文件没有创建线程、异步任务、锁或通道。`MemReaderBackend` 要求 `Send + Sync`，并通过 `Arc` 在克隆的读取器和迭代器间共享；后端内部若有可变缓存，需要自行保证同步与关闭语义。

`txnMemBufferIter::Close` 标记 `closed` 并清空当前范围；之后 `Valid` 恒为假。`memRowsIterForTable` 和 `memRowsIterForIndex` 的 `Close` 转发给 KV 迭代器，`defaultRowsIter::Close` 则把游标移到末尾。`iterTxnMemBuffer` 只在正常完成循环后显式关闭；回调或后端提前报错时局部迭代器依靠 Rust drop 释放 `Vec`/`Arc`，没有额外后端关闭回调。

Go 版本的注释明确要求 `memRowsIter.Close` 释放其持有的 snapshot，并由底层 `kv.Iterator.Close` 管理资源；Rust 版本当前只持有物化 `Vec<KvPair>`，因此关闭行为更轻。未来若后端改为真实流式存储迭代器，必须扩展 trait 或提供 RAII guard，不能继续假设清空向量等价于释放事务快照。

## 与 Go 版本的对应关系

`pkg/executor/mem_reader.go` 是逐名对应来源：`memIndexReader`、`memTableReader`、`txnMemBufferIter`、`memIndexLookUpReader`、`memIndexMergeReader`、三种行迭代器及辅助函数在 Rust 中均有对应物。主算法保持一致：降序反转范围；额外排序时物化；事务写覆盖临时/缓存快照；删除标记跳过；全局索引按分区过滤；IndexLookUp 先索引后回表；IndexMerge 按句柄交/并集后回表。

Rust 将 Go 的具体 TiDB 类型压缩为本地模型和 `MemReaderBackend`，并用字符串表示条件。这使算法可单测，但尚未包含 Go 的 `context.Context`、tracing、会话时区与 SQL mode、真实 `expression.Expression`、新旧 rowcodec 双路径、chunk 复用、事务激活及底层 iterator 生命周期。Go 对 Common Handle 的补列只在 value 缺失时从句柄恢复，并考虑前缀聚簇索引；Rust `getRowData` 也先保留 `decode_row` 已有值，再仅对缺列调用 `decode_common_handle_column`，保留了这一关键顺序。

Go 使用 `kv.HandleMap` 支持 TiDB 句柄相等性，Rust 依赖 `Handle` 派生的 `Ord`；接入真实编码前必须验证两者对 Common/Partition Handle 的等价关系。Go 的 `transaction.NewUnionIter` 是流式合并，Rust `union_range` 用 `BTreeMap` 物化单范围；行为目标相同，但资源和性能特征不同。

独立 Rust 测试 `pkg/executor/mem_reader_test.rs::common_handle_fills_each_primary_key_column_from_its_encoded_component` 验证普通及分区包装 Common Handle 会按列序恢复两个主键分量。Go 的端到端覆盖主要位于 `pkg/executor/union_scan_test.go::TestUnionScanForMemBufferReader`，并包含降序表扫、索引扫和 IndexLookUp benchmark；分区与 IndexMerge 还有 `pkg/executor/partition_table_test.go` 等执行测试。它们证明 Go 行为意图，不等同于 Rust 接线测试。

## 扩展指南

接入 Rust UnionScan 主链时，应从 `UnionScanSpec` 和四个 `buildMem*Reader` 构造器入手，提供连接真实事务/临时表/缓存、tablecodec/rowcodec、表达式和范围转换的 `MemReaderBackend`。必须新增独立测试文件，不能把测试嵌入 `mem_reader.rs`；最近的单元测试位置是 `pkg/executor/mem_reader_test.rs`，模块注册在 `pkg/executor/lib.rs`。

新增列或句柄形态时同步检查 `Datum`、`Handle`、`getTypes`、`decodeIndexKeyValue`、`getRowData`、`handle_datum` 和后端 trait。新增分区行为时同步检查全局索引过滤、`GroupedRanges`、Partition Handle 包装、`table_handles_to_ranges` 的 table ID 约定及句柄排序/相等语义。

改变快照来源或覆盖规则时优先修改 `getSnapIter`/`union_range`，并补齐“临时优先缓存、事务覆盖快照、事务空值删除遮蔽旧值、正反向次序、多范围跨段”的测试。若改成流式合并，需要同时设计错误返回与可靠关闭，避免当前 `Valid` 延迟错误协议造成越界访问。

性能修改应分别测量流式路径、额外排序路径、IndexLookUp 回表和 IndexMerge 句柄物化；不得把 Go 的复用缓冲优化当作 Rust 已实现。兼容性风险集中在编码/时区/SQL mode、Common Handle 前缀恢复、无符号主键、全局索引分区 ID 和重复句柄交集计数。

## 验证依据

- 目标源码：`pkg/executor/mem_reader.rs`（RustCodeGraph 显示 1314 行、116 个符号），逐段核对类型、构造器、读取流程、合并迭代器、错误和关闭实现。
- crate 与模块：`pkg/executor/Cargo.toml` 的 `[package]`、`[lib]`、`[features]`、`[package.metadata.porting]`；`pkg/executor/lib.rs:142-144` 的生产模块与独立测试模块声明。目标包不存在 `pkg/executor/doc.go`。
- RustCodeGraph：`status` 显示索引包含 11467 文件、307296 节点、1848419 条边；`files --filter pkg/executor/mem_reader.rs` 确认目标文件；`node --file` 覆盖全部 1-1314 行；`explore` 确认构造器调用边及当前无生产 Rust 调用者。
- Rust 测试：`pkg/executor/mem_reader_test.rs`，验证 Common Handle 多主键分量及分区包装的透明解码。
- Go 对照：`pkg/executor/mem_reader.go`，核对同名类型/函数、UnionScan 会话接线、KV 合并、解码、条件、排序与生命周期；`pkg/executor/union_scan_test.go` 和 `pkg/executor/partition_table_test.go` 提供端到端行为与分区场景证据。
- 本任务为纯文档分析，按计划不运行 Cargo；最终结构检查要求本文恰好具有规定的 11 个二级章节。
