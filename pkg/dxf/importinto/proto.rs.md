# `pkg/dxf/importinto/proto.rs`

## 文件定位

[`proto.rs`](proto.rs) 位于 `astersql-dxf-importinto` crate（见 [`Cargo.toml`](Cargo.toml)），是 `IMPORT INTO` 分布式任务的元数据协议层。它不执行 SQL、文件扫描或 SST ingest，而是定义任务级与步骤级状态的 Rust 表示、Go 兼容 JSON 线格式，以及编码/排序阶段共享的少量运行时状态。

该文件处于两条链路的交界处：

- 控制面：[`planner.rs`](planner.rs) 用 `TaskMeta` 和各 `*StepMeta` 生成持久化任务/子任务 payload；[`scheduler.rs`](scheduler.rs)、[`job.rs`](job.rs) 和 [`task_executor.rs`](task_executor.rs) 再解码这些 payload。
- 数据面：[`encode_and_sort_operator.rs`](encode_and_sort_operator.rs) 创建 `SharedVars` 和 `importStepMinimalTask`，writer 结果经 `SortedKVMeta` 汇总后回填步骤元数据。

模块由 [`lib.rs`](lib.rs) 中的 `mod proto;` 装配并再导出。文件头已有 `// Copyright 2026 AsterSQL.`，表明这份 Rust 实现已进入实际移植维护范围，而非仅保存 Go 草稿。

## 核心职责

1. 定义顶层任务协议 `TaskMeta`，在 `LogicalPlan::ToTaskMeta`、`LogicalPlan::FromTaskMeta` 与调度/执行器之间传递 job、导入计划、SQL、汇总、候选实例和 chunk map。
2. 定义 prepare、import/encode-sort、merge-sort、write-ingest、collect-conflicts、conflict-resolution、post-process 各阶段的元数据形状。
3. 用显式 wire 类型和 JSON 构造函数保持 Go 字段名、枚举值、`[]byte` base64 表示、空值及 `omitempty` 行为。
4. 支持 global-sort 大字段外置：`BaseExternalMeta.ExternalPath` 非空时，步骤 envelope 只保留路径及必要的小字段，调用者再从对象存储加载完整内容。
5. 聚合排序键范围、KV 数量/大小、多文件统计与冲突信息，并提供 data/index KV group 命名规则。
6. 承载 encode-sort worker 的共享状态、互斥保护、原子文件计数、panic 恢复标签与 checksum 转换。

## 主要符号

- `ServerInfo`：持久化执行实例的 `id`、`ip`、`listening_port`；serde 同时接受 Go 的 `ID`、`IP`、`ListeningPort`。
- `MultipleFilesStat`：一组外部 data/stat 文件的最小/最大键与最大重叠度。`MinKey`、`MaxKey` 通过 `serialize_go_bytes`/`deserialize_go_bytes` 与 Go `[]byte` 的 JSON 形式兼容。
- `WriterSummary`：writer 的内存结果，不直接序列化；包含可选键范围、KV 计数/大小、多文件统计和 `engineapi::ConflictInfo`。
- `SortedKVMeta`：可持久化的排序结果摘要。`Merge` 合并范围、计数、文件和冲突；`MergeSummary` 先调用 `new_sorted_kv_meta`；`GetDataFiles`/`GetStatFiles` 从成对文件名中投影对应一侧。
- `new_sorted_kv_meta`：空键范围返回默认值；非空最大键末尾追加 `0`，将闭合最大键变成排序范围的开区间上界。
- `DATA_KV_GROUP`、`index_id_to_kv_group`：data 使用固定字符串 `"data"`，索引使用十进制 index ID。
- `BaseExternalMeta`：保存 `ExternalPath`；其私有 `marshal_value` 统一将 JSON value 转为字节并映射错误。
- `TaskMeta`：顶层任务状态。`Marshal`/`Unmarshal` 通过 `TaskMetaWire`、`PlanWire`、`SummaryWire`、`ChunkWire` 做显式双向转换。
- `PreparedMeta`：prepare 阶段生成的 `ChunkMap`；路径为空时内联，路径非空时只输出 envelope。
- `ImportStepMeta`：engine ID、chunks、checksum、allocator 最大值、data/index 排序摘要和冲突总数；本文件提供双向解码。
- `MergeSortStepMeta`、`WriteIngestStepMeta`：global-sort 后续步骤的输入。前者携带待归并 data files；后者还携带 stat files、range job/split keys 与时间戳。
- `KVGroupConflictInfos`：按 KV group 聚合冲突。`addConflictInfo` 对零计数短路，其余调用 `ConflictInfo::Merge`。
- `CollectConflictsStepMeta`、`ConflictResolutionStepMeta`：冲突收集/解决协议，包括 checksum、冲突行文件与封顶/过多冲突标志。
- `PostProcessStepMeta`：后处理所需 checksum、删除行 checksum、冲突标志及 allocator 最大值；实际 JSON 构造在 `planner.rs::post_process_value`。
- `SharedVars`：单个 subtask 内 minimal task 的运行态；包含 importer、可选本地 engine、聚合结果、对象存储和原子文件计数。
- `importStepMinimalTask`：一个 chunk 的最小工作单元；`RecoverArgs` 提供 worker panic 标签与错误，`String` 输出 `chunk:<path>:<offset>`。
- `Checksum`、`newFromKVChecksum`、`Checksum::ToKVChecksum`：持久化 checksum 三元组与运行时 `verification::KVChecksum` 的互转。

私有 wire/helper 符号包括 `ChunkWire`、`StepSummaryWire`、`SummaryWire`、`PlanWire`、`TaskMetaWire`，以及 `chunk_value`、`chunks_value`、`chunk_map_value`、`conflict_infos_value`、`go_byte_slices_value` 和 `go_conflict_info`。它们集中隔离协议兼容逻辑，避免依赖类型必须直接实现本 crate 所需的 serde 形状。

## 执行流程

顶层任务元数据流程如下：

1. `planner.rs::LogicalPlan::ToTaskMeta` 从逻辑计划组装 `TaskMeta` 并调用 `Marshal`。
2. `TaskMeta::Marshal` 将 importer 内部类型投影为 wire 类型：`PlanWire` 固定选项字符串和表信息，`SummaryWire` 固定各步骤摘要，`ChunkWire` 固定 source/compression 数值。
3. JSON 字节存入分布式框架任务。`scheduler.rs::prepareImportTask`、`task_executor.rs::GetEncodeSortStepExecutor`、`job.rs` 等调用 `TaskMeta::Unmarshal` 恢复状态。
4. `TaskMeta::Unmarshal` 先解析 JSON，再逐个 `TryFrom<ChunkWire>` 校验枚举；任何 chunk 非法都会使整个 map 解码失败。

步骤元数据流程如下：

1. `scheduler.rs::prepareImportTask` 生成 `PreparedMeta` 并写入对象存储；planner 可通过 `PreparedMeta::Unmarshal` 恢复 chunk map。
2. `planner.rs::generateImportSpecs` 为每个非索引 engine 建立 `ImportStepMeta`。`ImportSpec::ToSubtaskMeta` 调用其 `Marshal`。
3. `planner.rs` 中 `impl_meta_spec!` 为 merge、write-ingest、collect-conflicts 和 conflict-resolution spec 统一调用各自 `Marshal`。
4. 当 `ExternalPath` 为空，`Marshal` 内联大字段；非空则只保留路径和调度仍需的小字段。`task_executor.rs::EncodeSortStepExecutor::read_meta` 先读 envelope，再按路径读取外置 JSON，并用外置内容替换运行数据。
5. encode-sort worker 创建 `SharedVars`，将其移入 `importStepMinimalTask`；任务处理完再取回。writer summary 通过 `mergeDataSummary`/`mergeIndexSummary` 合并为步骤输出。
6. 后续 planner 用 `previous_import_metas`、`encoded_kv_metas` 等恢复前序结果，按 `DATA_KV_GROUP` 或索引 ID 生成 merge/write-ingest 计划。

## 数据与状态

- 键范围不变量：`SortedKVMeta.StartKey` 是最小键，`EndKey` 是开区间上界；由 writer 最大键构造时追加零字节。`Merge` 只在另一侧范围非空时参与合并。
- 计数语义：`SortedKVMeta::Merge` 对 `TotalKVSize`、`TotalKVCnt` 使用 `wrapping_add`，与 Go 无符号整数溢出行为一致；测试明确断言最大值回绕。
- 空值语义：Go `[]byte(nil)`/空 slice 在本协议中序列化为 JSON `null`，非空值为标准 base64；反序列化 `null` 得到空 `Vec<u8>`。
- `PlanWire` 是 `importer::Plan` 的可序列化子集。旧 payload 只有 `table_id`/`table_name` 时会构造最小 `TableInfo`；未知 checksum 字符串回退到 `Required`，未知 duplicate-key 模式回退到 error，非 `query` 数据源回退到 file。
- `ImportStepMeta.Checksum` 的 JSON object key 是十进制 `i64`；`MaxIDs` 的 key 只能是 `_tidb_rowid`、`auto_increment`、`auto_random`、`sequence`。
- `RecordedConflictKVCount` 是跨所有 sorted meta 的快捷总数，用于在无冲突时避免读取外置详情；若为零则按 Go `omitempty` 省略。
- `KVGroupConflictInfos.ConflictInfos` 在 Rust 中是默认空 `HashMap`，而 Go 初始值是 nil map；两者对协议输出和聚合行为保持“零冲突不创建条目”。
- `SharedVars` 既有持久化摘要对应物，也有纯运行时资源（`TableImporter`、opened engines、storage、mutex、atomics），因此它本身不参与 serde。

## 依赖与调用关系

上游调用者（RustCodeGraph 文件导航与直接引用搜索共同确认）：

- [`planner.rs`](planner.rs)：组装/恢复 `TaskMeta`，生成并序列化全部步骤 meta，合并 `SortedKVMeta`，处理外置 payload。
- [`scheduler.rs`](scheduler.rs)：prepare 时解码任务、生成 `PreparedMeta`，并在任务状态更新中重复读取/写回 `TaskMeta`。
- [`task_executor.rs`](task_executor.rs)：按步骤解码 `TaskMeta` 和 `ImportStepMeta`，加载 `ExternalPath` 指向的完整元数据，并消费 merge/write-ingest meta。
- [`job.rs`](job.rs)：从任务与快照中恢复 `TaskMeta`，读取进度摘要。
- [`encode_and_sort_operator.rs`](encode_and_sort_operator.rs)：构造 `SharedVars`/`importStepMinimalTask`，持有本地 engine 与 global-sort writer 汇总。
- [`conflict_resolution.rs`](conflict_resolution.rs)：消费任务和 conflict-resolution meta。

主要下游 crate 依赖由 [`Cargo.toml`](Cargo.toml) 声明：`astersql-executor-importer` 提供 Plan/Chunk/Summary/TableImporter，`astersql-ingestor-engineapi` 提供冲突信息，Lightning backend/verification/mydump/log 提供 engine、checksum、文件类型与日志，meta autoid/model 和 parser ast/mysql 提供 allocator、表结构及 SQL 模式，objstore storeapi 提供外置存储 trait，framework proto 提供 panic 指标标签。`serde`、`serde_json`、`base64` 实现 wire 编解码。

RustCodeGraph 的同名符号查询存在限制：`Marshal`/`Merge` 同时出现在 Go 和 Rust 文件中，未限定的 callers/callees 查询会落到 Go 定义或无法消歧；因此调用边以唯一符号查询、索引文件片段和 Rust 直接引用交叉验证，不把“used by 133 files”误当成该协议文件的精确调用者集合。

## 错误处理与边界

- 所有 JSON 解析/序列化错误统一转换成 `errors::SharedError`，保留底层错误字符串。
- `ChunkWire -> importer::Chunk` 只接受 source type `0..=6`、compression `0..=6`；其它值明确报 `invalid source type` 或 `invalid compression`。
- `ImportStepMeta::Unmarshal` 要求 `ID` 是 `i32` 范围内整数，`ExternalPath` 是字符串，冲突计数是 `u64`，checksum map key 可解析为 `i64`，allocator 名称属于固定集合。
- 缺失或 JSON `null` 字段大多恢复为空集合、零值或 `None`，用于兼容 Go 的零值和旧 payload；但错误类型、越界值与非法枚举不会静默接受。
- `BaseExternalMeta` 只负责 envelope 编码，不负责 I/O；路径存在性、读取失败与内外层合并由 planner/executor 处理。
- 多数 step meta 只有 `Marshal`，其反序列化由调用者按实际消费字段完成；不能假设所有结构都支持本文件内的完整 round trip。
- `SortedKVMeta::GetDataFiles`/`GetStatFiles` 假设 `Filenames` 元素始终是长度为二的数组，类型本身保证索引安全，但不验证路径非空或文件存在。
- `Checksum` 转换只复制数值，不执行 checksum 校验；真正验证发生在上层后处理逻辑。

## 并发与资源生命周期

`SharedVars.mu` 保护 `SortedDataMeta`、`SortedIndexMetas` 和 `RecordedConflictKVCount` 的组合更新；`mergeDataSummary` 与 `mergeIndexSummary` 都在同一临界区内先聚合 meta 再增加冲突数。锁中毒通过 `expect("shared import summary mutex poisoned")` 转成 panic，而不是业务错误。

`ShareMu` 与 `dataKVFileCount`/`indexKVFileCount` 为其它 encode-sort 路径提供共享同步和原子计数；本文件只定义所有权，不实现它们的访问协议。`globalSortStore`、`DataEngine`、`IndexEngine` 以 `Arc` 共享资源，`SharedVars` 随 `importStepMinimalTask` 在 worker 中移入/取回。`encode_and_sort_operator.rs::ThreadOwnedEncodeSortWorker::Close` 在 worker 结束时合并 checksum/allocator 最大值并关闭 `TableImporter`。

`importStepMinimalTask::RecoverArgs` 只提供 workerpool panic 恢复所需的指标标签、任务名和统一错误文本；不捕获 panic。文件也不创建线程或异步任务，实际并发调度位于 operator/executor。

注意当前方法接收 `&mut self` 后再锁定内部 mutex；这使单个 `SharedVars` 的安全访问依赖上层所有权安排，而不是仅靠 mutex 提供任意 `&self` 并发写。扩展时应保持与 worker 持有模型一致。

## 与 Go 版本的对应关系

直接对照文件是 [`proto.go`](proto.go)，基础结构基本逐项对应，但 Rust 为跨 crate 类型和 Go wire 兼容增加了显式适配层：

- Go 直接由 `encoding/json` 反射结构体；Rust 用 `PlanWire`、`SummaryWire`、`ChunkWire` 等避免修改依赖 crate 类型并控制字段别名。
- Go 的 `globalsort.BaseExternalMeta.Marshal(m)` 根据 `external:"true"` tag 外置大字段；Rust 当前 `Marshal` 只根据已设置的 `ExternalPath` 决定是否省略大字段，实际写外置文件/设置路径由 planner/scheduler 完成。
- Go `SortedKVMeta` 来自 `pkg/ingestor/globalsort`；Rust 在本文件本地保留同义类型，源码注释明确说明这是 globalsort crate 暴露相应生产模块前的过渡安排。
- Go 的 `SortedIndexMetas` 存指针，Rust 存值；合并语义保持一致，但 Rust 不表达 nil meta 元素。
- Go 的 nil map 与 Rust 空 `HashMap` 在内存形态不同；协议 helper 通过 `null`/省略规则尽量维持 wire 行为。
- Go `SharedVars` 依靠 `sync.Mutex`/`atomic.Int64`，Rust 对应 `Mutex<()>`/`AtomicI64`，engine 与 store 用 `Arc` 表达共享所有权。
- Go `importStepMinimalTask` 持有 `*SharedVars`；Rust 当前按值持有并由 worker 在每次处理前后搬移。

[`proto_test.go`](proto_test.go) 只验证 KV group 冲突聚合。独立 Rust 测试 [`proto_test.rs`](proto_test.rs) 保留该 Go 测试的草稿，同时可执行地覆盖冲突计数、checksum 往返、import meta round trip、`i32` 溢出、非法 chunk type、空 summary 与无符号计数回绕，覆盖面比当前 Go 单测更广。

## 扩展指南

- 新增 `TaskMeta`/`importer::Plan` 字段时，应同步修改对应 wire struct、双向 `From`/`TryFrom`、字段 alias/default 与 [`proto.go`](proto.go) JSON 语义；在 [`proto_test.rs`](proto_test.rs) 添加新旧 payload 往返和缺省值测试。
- 新增 chunk source/compression 枚举时，必须同时扩展 `ChunkWire::try_from` 的数值映射，并验证与 Go 枚举编号一致；否则已有合法 Go payload 会被 Rust 拒绝。
- 新增大字段时，必须明确 inline 与 `ExternalPath` envelope 两种表示，并同步 planner 的 `set_spec_external_path`、`previous_value` 或 executor 的读取逻辑；仅在 struct 上加字段不会自动获得 Go `external:"true"` 行为。
- 新增 allocator 类型时，应同步 `ImportStepMeta::Unmarshal` 的名称映射、`Marshal` 使用的 `AllocatorType::as_str()` 约定和 post-process 转换。
- 修改排序范围或合并规则时，应同时核对 `pkg/ingestor/globalsort/util.go`/`.rs` 和 planner 的 `encoded_kv_metas`、`skip_merge_sort`，并在独立测试中覆盖空范围、相邻键、溢出及文件顺序。
- 修改并发汇总时，应在 [`encode_and_sort_operator_test.rs`](encode_and_sort_operator_test.rs) 或新的独立 `*_test.rs` 中测试，不要把测试嵌入 `proto.rs`；需审查锁粒度、panic 行为、原子顺序和 worker close 时的资源回收。
- 协议字段名属于持久化兼容面。重命名应保留旧 alias，并用真实 Go JSON fixture 验证双向读取，避免只验证 Rust 自身 round trip。

主要风险是 wire 兼容（字段大小写、null/省略、枚举编号）、外置元数据丢字段、键范围边界和并发汇总遗漏；性能风险集中在大 map/vector 的 clone、JSON value 中间分配和错误地内联本应外置的数据。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/dxf/importinto` 定位目标、Go 对照和独立测试；`node --file` 完整读取 `proto.rs`，并读取 `planner.rs`、`task_executor.rs`、`scheduler.rs`、`encode_and_sort_operator.rs` 的直接调用片段。
- RustCodeGraph 符号查询：确认 `TaskMeta`、`SortedKVMeta`、`ImportStepMeta`、`SharedVars` 的 Rust/Go 定义及 `read_meta`、`previous_import_metas`、operator 构造等唯一使用点；同名方法的图查询歧义已在“依赖与调用关系”记录。
- 源与边界：[`proto.rs`](proto.rs)、[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)。
- Go 对照：[`proto.go`](proto.go)、[`proto_test.go`](proto_test.go)，以及 globalsort 类型来源的 `pkg/ingestor/globalsort/util.go`（由符号查询确认）。
- Rust 测试：[`proto_test.rs`](proto_test.rs)；调用面还通过 `rg` 核对 `planner.rs`、`scheduler.rs`、`job.rs`、`task_executor.rs`、`encode_and_sort_operator.rs`、`conflict_resolution.rs` 及其独立测试引用。
- 本任务是纯文档分析，按计划不运行 Cargo。交付结构验证要求本文恰有十一个固定二级标题，并由指定 shell 命令检查。
