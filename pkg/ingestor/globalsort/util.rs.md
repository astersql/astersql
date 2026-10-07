# `pkg/ingestor/globalsort/util.rs`

## 文件定位

本文件属于 `astersql-ingestor-globalsort` crate。crate 由 `pkg/ingestor/globalsort/Cargo.toml` 定义、以同目录 `lib.rs` 为入口；`lib.rs` 声明 `pub mod util` 并通过 `pub use util::*` 将这里的公共项提升到 crate 根。上层 `pkg/ingestor/doc.go` 将 globalsort 定位为：借助外部存储保存中间有序文件并执行归并排序，随后为 SST 导入准备键范围和文件信息。

它不是排序算法本体，而是全局排序链路的公共支撑层，集中承担四类工作：清理任务文件，构造测试用外部引擎文件，汇总/搬运已排序 KV 元数据，以及把待归并 data 文件规划成受节点数、并发度和目标文件上限约束的子任务。计划与子任务元数据路径也在这里统一生成。

## 核心职责

1. `CleanUpFiles`、`CleanUpFilesInDirectories` 根据任务目录找出普通路径和随机分区路径下的对象并批量删除；多目录版本只扫描对象存储一次。
2. `MockExternalEngine` 将等长的 key/value 输入每四条分块，写出成对的 `.data`/`.stat` 测试文件，使拆分逻辑能稳定覆盖多文件场景。
3. `SortedKVMeta`、`NewSortedKVMeta` 与其方法把 writer 摘要转换为半开键范围，累加大小、条数、文件统计与冲突信息，并能展开 data/stat 文件清单。
4. `ExternalMetaCodec` 与 `BaseExternalMeta` 定义“内联元数据 + 外部元数据”的协议；具体 JSON 格式由使用者实现 codec，本文件只按 `ExternalPath` 决定读写哪一种形态。
5. `PlanMetaPath`、`PreparedMetaPath`、`SubtaskMetaPath` 生成稳定的 `meta.json` 对象路径。
6. `DivideMergeSortDataFiles` 在每组输入数、执行并行度和归并输出文件数上限之间做平衡；`summary_for_file` 为单个 data/stat 文件对生成粗粒度范围属性。

## 主要符号

- 常量 `META_NAME = "meta.json"` 固定所有计划/子任务元数据的叶子文件名；`MAX_MERGE_SORT_FILE_COUNT_STEP = 4000` 同时封顶单组输入步长和允许的目标文件总数阈值。
- `CleanUpFiles(store, non_partitioned_dir)` 是单目录兼容入口，直接委托 `CleanUpFilesInDirectories`。
- `CleanUpFilesInDirectories(store, dirs)` 去除目录两端的 `/`，调用 `astersql_ingestor_simplesst::util::GetAllFileNamesFromScan` 从一次 `store.list_prefix("")` 结果中筛选普通/随机分区文件，再交给 `Storage::delete_files`。
- `MockExternalEngine(storage, keys, values)` 校验切片等长，以 `MOCK_ENGINE_KVS_PER_FILE = 4` 分块；data 内容由 `encode_kvs` 编码，stat 内容保存首尾 key 的小端 `u32` 长度和原始字节。
- `SortedKVMeta` 的公开字段为 `StartKey`、`EndKey`、`TotalKVSize`、`TotalKVCnt`、`MultipleFilesStats`、`ConflictInfo`。`EndKey` 是排他上界。
- `NewSortedKVMeta(Option<&WriterSummary>)` 对 `None` 或 min/max 同时为空返回默认值，否则复制摘要并用 `next_key(summary.max)` 构造排他上界。
- `SortedKVMeta::{Merge, MergeSummary, GetDataFiles, GetStatFiles}` 分别负责元数据合并、摘要转换后合并，以及按原统计顺序展开文件路径。
- `BytesMin`、`BytesMax` 按 Rust 字节切片的字典序返回原切片之一；相等时两者都返回第二个参数，但内容不变。
- `ExternalMetaCodec` 要求调用者分别实现全量、内部、外部序列化及外部反序列化；`BaseExternalMeta::{Marshal, WriteJSONToExternalStorage, ReadJSONFromExternalStorage}` 负责选择协议分支和访问 `Storage`。
- `PlanMetaPath`、`PreparedMetaPath`、`SubtaskMetaPath` 分别生成 `{task}/plan/{step}/{index}/meta.json`、`{task}/plan/prepared/meta.json`、`{task}/{subtask}/meta.json`。
- `adjusted_file_count_step` 与 `adjusted_overlap_threshold` 都计算 `min(250 * max(concurrency, 1), 4000)`，使用饱和乘法避免 `usize` 溢出。
- `DivideMergeSortDataFiles(data_files, node_count, merge_concurrency)` 返回保持原输入顺序的文件组，或 `InvalidArgument`/`TooManyDataFiles`。
- `summary_for_file(data_file, stat_file, kvs)` 每四条 KV 生成一个 `RangeProperty`，并构造单个 `MultipleFilesStat` 的 `WriterSummary`。

## 执行流程

清理流程从 `CleanUpFiles` 或 DXF 的批量入口进入。空目录列表立即成功；非空列表先规范化斜杠，再全量列举一次对象名。`GetAllFileNamesFromScan` 同时识别任务直属路径和形如随机分区前缀下的任务路径，筛选结果最后一次性交给 `delete_files`。因此重复目录不会导致重复扫描，扫描失败时不会进入删除阶段。

元数据汇总流程通常始于 writer 关闭回调产生的 `WriterSummary`。`NewSortedKVMeta` 将 `[min, max]` 转换为 `[StartKey, next_key(max))`；后续 `MergeSummary` 再调用 `Merge`。若来源为空元数据则忽略；若接收者为空则完整克隆来源；否则键下界取字典序最小值、上界取最大值，计数按 `u64` 模 (2^{64}) 回绕相加，文件统计按输入顺序追加，冲突计数与文件列表交给 `ConflictInfo::merge`。

外部元数据流程由 `ExternalPath` 控制。路径为空时 `Marshal` 调用 `marshal_all`，读写外部存储均为空操作；路径非空时 `Marshal` 只返回 `marshal_internal`，`WriteJSONToExternalStorage` 把 `marshal_external` 的字节写到该路径，`ReadJSONFromExternalStorage` 读取同一路径并调用 `unmarshal_external`。具体哪些字段属于内部或外部由 codec 实现者决定。

归并分组先拒绝 `node_count == 0`，随后对空输入返回空组。非空输入先计算最大组大小 `max_files`，按“完整节点轮次”生成若干个恰含 `max_files` 的组：`file_count / max_files / node_count * node_count` 保证这一段组数为节点数的整数倍。然后用 `merge::getTargetFileCount` 估算完整组的输出文件数。余数部分的候选组数范围为 `ceil(remaining/max_files)..=max(1, min(remaining/32, node_count))`；从最大候选向下选择第一个使总目标文件数不超过阈值的方案。最后用商和余数均分，前 `extra` 组各多一个文件，保持输入顺序且不漏不重。

## 数据与状态

本文件不维护全局可变状态。所有持久状态要么存于调用者传入的 `Storage`，要么由值类型返回给调用者。

`SortedKVMeta` 的核心不变量是：非空摘要的 `StartKey` 为最小已写 key，`EndKey` 为最大已写 key 的 `next_key`，因此范围为半开区间；`MultipleFilesStats` 保持合并次序；`GetDataFiles`/`GetStatFiles` 只投影文件路径，不排序也不去重。空元数据用 StartKey、EndKey 同时为空来识别，其他计数字段不参与空判断。

`MockExternalEngine` 和 `summary_for_file` 都按四条 KV 建立小粒度结构，但用途不同：前者真正写对象并返回路径，后者只构造内存中的范围/大小/条数摘要。两者都假定输入 KV 已按期望顺序排列；`summary_for_file` 直接把首尾元素视为整体 min/max，并未自行排序。

归并分组只复制 `String` 路径，不修改输入；输出所有组连接后与输入顺序一致。`merge_concurrency == 0` 被内部计算按 1 处理，但错误对象仍保留调用者传入的原并发值，这一点扩展时不可悄然改变。

## 依赖与调用关系

直接下游依赖来自 crate 根和相邻 crate：`Storage` 提供对象读写/列举/删除，`encode_kvs` 编码测试数据，`next_key` 生成排他上界；`FilePair`、`RangeProperty`、`MultipleFilesStat`、`WriterSummary`、`ConflictInfo` 承载统计；`merge::getTargetFileCount` 与 `getGroupedTargetFileCount` 估算归并输出；`astersql-ingestor-simplesst` 提供跨随机分区目录的文件筛选；`astersql-ingestor-errdef` 构造可识别的 `TooManyDataFiles` 错误。`Cargo.toml` 将 globalsort 声明为独立 workspace crate，并显式依赖 errdef、simplesst、engineapi、membuf 和 workerpool 等本地 crate。

RustCodeGraph 对本文件给出的上游包括 `pkg/ddl/backfilling_dist_scheduler.rs`、`pkg/dxf/importinto/clean_up.rs`、`planner.rs`、`task_executor.rs` 等。直接代码证据如下：

- `pkg/dxf/importinto/clean_up.rs::BatchCleanWithContext` 按外部存储 URI 聚合多个任务目录，再调用 `CleanUpFilesInDirectories`，实现一次扫描清理多个任务。
- `pkg/dxf/importinto/planner.rs::write_external_plan_meta` 用 `PlanMetaPath` 为各 pipeline spec 生成外部计划元数据路径；`generate_merge_sort_specs` 对 `SortedKVMeta::GetDataFiles()` 调用 `DivideMergeSortDataFiles`，将结果逐组转为 `MergeSortSpec`。
- `pkg/dxf/importinto/task_executor.rs` 用 `SubtaskMetaPath` 保存 encode/merge 子任务外部元数据；归并 writer 的关闭回调把摘要转换后交给共享 `SortedKVMeta::MergeSummary`，归并结束再保存汇总值。
- `pkg/dxf/importinto/scheduler.rs` 用 `PreparedMetaPath` 定位 prepared 阶段元数据。
- `MockExternalEngine` 的直接 Rust 使用者主要是 `pkg/ingestor/globalsort/split_test.rs`；`summary_for_file` 当前直接由 `util_test.rs` 验证，是测试/夹具辅助入口而非生产主链入口。

## 错误处理与边界

- `CleanUpFilesInDirectories` 对空列表成功且不访问存储；列举或删除错误通过 `?` 原样传播。它会先把全部匹配文件名收集到内存，再批量删除，Go 源码也标注了大任务可能产生高内存占用的 TODO。
- `MockExternalEngine` 对 keys/values 长度不等返回 `Error::InvalidArgument`；空输入返回两个空文件列表，不会触发块内 `expect`。一旦某个写入失败会立即返回，之前已经写出的文件不会回滚。
- `NewSortedKVMeta` 将 `None` 与 min/max 同时为空视作空摘要；只有一个端点为空时仍按非空摘要处理。
- `SortedKVMeta::Merge` 的计数使用 `wrapping_add`，明确匹配 Go `uint64` 的溢出回绕，而非 panic 或饱和。文件统计和冲突文件列表不去重。
- 外部元数据路径为空时，外部读写静默跳过；路径非空时 codec 序列化、`Storage::write/read` 和反序列化错误均立即传播。写入没有事务性回滚保证。
- `DivideMergeSortDataFiles` 在节点数为零时返回 `InvalidArgument("unsupported zero node count")`，即便输入为空也先报错；若估算目标文件数超过 `min(250*concurrency, 4000)`，返回 errdef 定义的 `TooManyDataFiles`。候选组不存在时同样报此错误。
- `summary_for_file` 可接受空 KV 切片并产生空范围、零计数和空 properties；只有非空 chunk 才执行内部 `expect`。

## 并发与资源生命周期

公共函数自身均为同步函数，不创建线程、任务、通道或锁。`Storage: Send + Sync` 允许调用者在并发上下文共享存储，但本文件的一次调用内按“列举后删除”或“顺序写 data/stat”串行访问。清理不是原子事务：扫描结束到删除之间对象集合可能变化，删除部分成功后的错误恢复语义取决于具体 `Storage` 实现。

`BaseExternalMeta` 不拥有存储句柄，只在方法调用期间借用 `&dyn Storage`；序列化对象也以 trait object 借用，不缓存引用。`SortedKVMeta::Merge` 需要 `&mut self`，并发聚合必须由上层同步。实际调用证据是 `pkg/dxf/importinto/task_executor.rs` 在多个 writer 回调之间用 `Arc<Mutex<SortedKVMeta>>` 串行执行 `MergeSummary`，归并结束后克隆最终快照。

`MockExternalEngine` 写出的对象由传入存储持有，本函数不自动清理；测试或调用者应使用清理入口管理生命周期。若中途失败，调用者也需处理残留对象。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/ingestor/globalsort/util.go`，Rust 独立测试对照 `util_test.rs` 与 `util_test.go`。

- 清理语义一致：都支持多个非分区目录并只遍历对象存储一次，也都会把匹配文件名整体收集后删除。Go 版本额外接受 `context.Context` 和 `skipCleanUpFiles` failpoint；Rust 接口没有取消上下文或该 failpoint。
- Go `MockExternalEngine` 使用真实 simplesst writer、内存/块大小和 property 距离配置来制造多文件；Rust 为确定性测试直接每四条写一个自定义 data/stat 对。它保留“产生多文件供 split 测试”的意图，但 stat 二进制并非完整 Go writer 产物，不应当作生产格式替代。
- `SortedKVMeta` 字段、半开范围、合并规则和文件展开顺序对应 Go。Rust 显式 `wrapping_add`，保证 debug/release 都符合 Go `uint64` 回绕；Rust 返回拥有所有权的 `Vec<String>`，Go 返回新 slice。
- Go 依靠反射和 `external:"true"` struct tag 自动筛出内外字段；Rust 无同等运行时反射，改为 `ExternalMetaCodec` 由具体类型显式实现四个操作。这是接口实现差异，内外分离语义由 `test_read_write_json` 对照验证。
- 路径函数的格式与 Go `path.Join` 结果相同；Rust 使用 `format!`，因此若将 `step` 扩展为含 `/`、`.` 或 `..` 的非规范片段，不会自动清理路径。现有调用传入框架步骤名，不依赖额外规范化。
- `DivideMergeSortDataFiles` 对应 Go 同名函数的两阶段算法：完整节点轮次 + 余数均分，候选组优先取更多并行组，并用相邻 merge 模块的目标文件估算守住 ingest 上限。Rust 额外显式拒绝零节点，并避免按巨大节点数预分配容量；`util_test.rs` 覆盖了百万节点场景的容量边界。
- Go 的 `marshalInternalFields`/`marshalExternalFields` 反射助手没有一一对应的 Rust 公共函数；这是有测试说明的迁移差异，不应在文档中声称 Rust 支持任意结构体的自动字段筛选。

## 扩展指南

修改清理规则时，应优先改 `CleanUpFilesInDirectories` 与 `astersql_ingestor_simplesst::util::GetAllFileNamesFromScan` 的契约，并同步 `util_test.rs` 中单次扫描、重复目录、随机分区、邻近任务保留及 scan/delete 错误传播用例。若要解决全量文件名驻留内存问题，需要同时设计 `Storage` 的分页列举和分批删除语义，不能只在本文件切片已有结果。

为 `SortedKVMeta` 增加字段时，应同步 `NewSortedKVMeta`、`Merge`、默认/空值判定、外部协议实现和所有 JSON/协议转换点，尤其是 `pkg/dxf/importinto/task_executor.rs`、`planner.rs` 及其独立测试。新增计数必须明确是 Go 式回绕、饱和还是报错，不能依赖 Rust 构建模式。文件路径展开若改变排序或去重规则，会影响归并规划，应同时更新 planner 测试。

扩展外部元数据时，具体类型必须在其 `ExternalMetaCodec` 中同步维护全量、内部、外部三种序列化与外部反序列化，并验证“内联信息足以定位外部对象、读取后能恢复完整逻辑状态”。若需要原子发布，应在 `Storage` 层提供临时对象/提交协议，本文件当前的直接写入无法保证。

修改 `DivideMergeSortDataFiles` 时必须维持四个性质：所有输入恰好出现一次且顺序不变；每组不超过调整后的步长；完整轮次能均匀占用节点；所有组经 `splitDataFiles` 后的目标文件总数不超过阈值。应同步 `util_test.rs` 的典型尺寸、阈值前后、非单调估算、超大节点数、零节点和空输入用例，并对照 Go `util_test.go`，不要用仅能通过小样例的简化分组替代现有算法。

测试必须继续放在独立的 `pkg/ingestor/globalsort/util_test.rs`（crate 入口通过 `#[path = "util_test.rs"] mod util_test` 接入），不要嵌入本源文件。

## 验证依据

- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点；`node --file pkg/ingestor/globalsort/util.rs` 读取了完整 357 行并报告六个使用文件；`query SortedKVMeta` 与精确 `explore` 识别了 DDL/DXF 调用者；`callees DivideMergeSortDataFiles` 识别内部步长函数和 `TooManyDataFiles` 错误边。
- 生产源码：`pkg/ingestor/globalsort/util.rs`；crate 装配和公共基础类型：`pkg/ingestor/globalsort/lib.rs`；crate 边界和依赖：`pkg/ingestor/globalsort/Cargo.toml`；模块定位：`pkg/ingestor/doc.go`。
- 上游调用：`pkg/dxf/importinto/clean_up.rs`、`pkg/dxf/importinto/planner.rs`、`pkg/dxf/importinto/scheduler.rs`、`pkg/dxf/importinto/task_executor.rs`，以及 RustCodeGraph 列出的 `pkg/ddl/backfilling_dist_scheduler.rs`。
- Go 对照：`pkg/ingestor/globalsort/util.go`；Go 测试：`pkg/ingestor/globalsort/util_test.go`。
- Rust 独立测试：`pkg/ingestor/globalsort/util_test.rs`；测试夹具使用者：`pkg/ingestor/globalsort/split_test.rs`。已人工核对的边界包括随机分区清理、扫描/删除错误、计数回绕、外部字段拆分、分组阈值与超大节点容量。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证只检查文档固定结构、路径及事实引用。
