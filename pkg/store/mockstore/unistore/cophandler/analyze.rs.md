# `pkg/store/mockstore/unistore/cophandler/analyze.rs`

## 文件定位

本文件属于 crate `astersql-store-mockstore-unistore-cophandler`，crate 根由同目录 `Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，`lib.rs` 以 `pub mod analyze` 导出本模块，并在测试配置下把独立文件 `analyze_test.rs` 挂为模块。它位于 UniStore mock coprocessor 的 Analyze 分支：`cop_handler.rs::handle_cop_request` 匹配 `RequestPayload::Analyze` 后调用本文件的 `analyze`，将返回字节放入 `Response.data`；因此这里服务的是仓库内 Rust mock 请求模型，而不是直接解析 TiDB/TiKV 的 protobuf Analyze 请求。

RustCodeGraph 对 `analyze.rs` 的文件查询得到 28 个符号，并给出两个直接使用文件：`cop_handler.rs` 和 `analyze_test.rs`。这与源码装配关系一致：前者提供线上 mock 入口及 `Datum`、`KeyRange`、`KvReader`、`Row`、`CopError`，后者验证本模块的局部 Go 语义。

## 核心职责

1. 用 `AnalyzeType` 和 `AnalyzeRequest` 表示五类统计请求及桶数、采样数、sketch 尺寸、列偏移等参数。
2. 由 `analyze` 通过 `KvReader::scan(ranges, start_ts, false)` 一次性读取 MVCC 可见行，再按请求类型分派到索引、Common Handle、列、混合或全采样实现。
3. 构造 mock 统计结果：`Histogram`/`HistogramBucket`、确定性水库样本、Count-Min Sketch（CMS）矩阵和简化的 FM sketch 字节。
4. 由 `encode_result` 把结果压缩为本仓库自定义的 Analyze 响应字节，交给 `handle_cop_request`。

这些职责仅覆盖 mock 所需的简化统计协议。Go 同路径 `analyze.go` 使用真实 `tipb.AnalyzeReq`/响应 protobuf、table/row codec、`statistics` 包 builder、collation、时区、TopN 和版本分支；Rust 文件没有宣称也没有实现这些完整能力。

## 主要符号

- `AnalyzeType`：公开枚举，包含 `Index`、`CommonHandle`、`Columns`、`Mixed`、`FullSampling` 五种分派值。
- `AnalyzeRequest`：公开请求结构。`column_offsets` 是行中参与统计的列位置；`bucket_count` 限制直方图桶数；`sample_size` 控制样本量；`sketch_depth`/`sketch_width` 定义 CMS 矩阵；`primary_column_count` 限制 Common Handle 前缀列数。
- `HistogramBucket`、`Histogram`、`AnalyzeResult`：公开结果模型。桶的 `count` 是截至当前桶上界的累计计数，`repeats` 是该桶最后一个值的频次；`AnalyzeResult` 同时承载行数、多个直方图、样本、CMS 和 FM 字节。
- `analyze`：公开总入口，负责扫描、取 `KvPair.value`、分派与编码。
- `analyze_index`：公开索引统计入口。它为每行生成所有逐列累积前缀；完整前缀进入索引直方图和 FM，全部中间前缀进入 CMS。
- `analyze_common_handle`：公开 Common Handle 入口，把列偏移截断到 `min(primary_column_count, column_offsets.len())` 后复用 `analyze_index`。
- `analyze_columns`：公开列统计入口，对每个列偏移各建一个直方图，并生成水库样本。
- `analyze_mixed`：公开混合入口，以列统计为主体，再附加索引 CMS、FM 和首个索引直方图。
- `analyze_full_sampling`：公开全采样入口。先复用列统计；当 `sample_size == 0` 时把全部行设为样本，再从样本构建 CMS/FM。
- `build_histogram`：公开直方图构造器；`analyze_test.rs` 直接验证它的累计桶计数。
- `reservoir_sample`、`build_cms`、`build_fm_sketch`、`encode_selected`、`encode_selected_prefixes`、`encode_result`、`datum_size`、`seeded_hash`：模块私有辅助函数，分别处理采样、sketch、选列编码、响应编码、大小估算和带种子哈希。

## 执行流程

入口链路如下：

1. `cop_handler.rs::handle_cop_request` 收到 `RequestPayload::Analyze(request)`，把 `Request.ranges`、`Request.start_ts` 和请求传给 `analyze`。
2. `analyze` 调用 `reader.scan(..., descending = false)`；可见性约束由 `KvReader` 合同保证，即只返回 `commit_ts <= start_ts` 的版本。本函数丢弃扫描结果的 key 和 commit timestamp，只保留每个 `KvPair.value` 行。
3. `AnalyzeType::Index` 调用 `analyze_index`。每个选中列在写入累计字节后形成一个前缀：最终前缀用于直方图和 FM，所有前缀用于 CMS。空偏移会得到空字节值，而不是列越界错误。
4. `CommonHandle` 先截断偏移列表，再走索引流程；当主键列数为零时，也会统计空字节前缀。
5. `Columns` 按 `column_offsets` 顺序抽取每列；任一行缺少指定位置就立即返回 `CopError::ColumnOffset`。所有列成功后才采样。
6. `Mixed` 先完整执行列统计，再执行索引统计；最终直方图顺序是各列直方图在前、索引直方图在后，样本来自列统计，CMS/FM 来自索引统计。
7. `FullSampling` 先执行列统计。`sample_size > 0` 时 sketch 仅反映水库样本；`sample_size == 0` 时覆盖列统计返回的空样本并保留全部行，因此 sketch 覆盖全部行。
8. `encode_result` 写出行数、直方图数量、每个直方图的 NULL 数/NDV/桶数，以及每桶上下界、累计计数和重复数。当前编码不包含 `Histogram.total_size`、`samples`、`cms` 或 `fms`。
9. 成功结果成为 `Response.data`；错误由 `handle_cop_request` 统一转换为 `Response.other_error`（锁错误例外，但本文件自身不构造锁错误）。

## 数据与状态

本文件没有全局可变状态。一次请求的数据全部保存在局部 `Vec`、`BTreeMap` 和 `HashSet` 中：扫描结果先整体物化为 `rows`，之后各分析路径又会克隆选列值、编码前缀或样本，所以内存占用随行数、列数、样本量和 CMS 的 `depth × width` 增长。

`build_histogram` 用 `BTreeMap<Datum, u64>` 同时排序和聚合非 NULL 值。`Datum` 的排序与编码由 `cop_handler.rs` 定义：NULL 最小，数值类型可跨类型比较，字节按字典序；直方图在计数阶段排除 NULL。`total_size` 按 `datum_size` 估算（数值均为 8 字节，字节值取实际长度），`distinct` 是非 NULL 排序键数量。

桶目标值为 `ceil(non_null_count / bucket_count)`。遍历有序频次时，`count` 从不在切桶后归零，所以每个桶的 `count` 是累计值；`bucket_count_so_far` 才是决定当前桶何时结束的局部计数。最后一个桶无论是否达到目标都会写出。`bucket_count == 0` 或没有非 NULL 值时不产生桶，但仍保留 NULL 数、大小和 NDV。

`reservoir_sample` 使用固定初值的 xorshift64，因此同一行顺序和样本大小得到确定结果；返回的样本顺序是样本槽顺序，不保证输入排序。CMS 每层以 `row_index + 1` 为种子执行 FNV-1a 风格哈希。FM 实现去重哈希、升序排序、截断至 1024 个值，再把每个 `u64` 写成大端字节；它不是 Go `statistics.FMSketch` 的协议实现。

## 依赖与调用关系

上游直接调用关系由 RustCodeGraph 文件边和源码共同确认：

- `cop_handler.rs::handle_cop_request` → `analyze`；请求由 `RequestPayload::Analyze(AnalyzeRequest)` 携带。
- `analyze_test.rs` → `build_histogram`、`analyze_index`；测试直接构造 `Datum` 行和 `AnalyzeRequest`。
- `lib.rs` 负责公开模块和挂载独立测试，但不调用业务函数。

本文件内部主要调用边为：

- `analyze` → `KvReader::scan`，再 → 五个 `analyze_*` 分支，最后 → `encode_result`。
- `analyze_common_handle` → `analyze_index`。
- `analyze_mixed` → `analyze_columns` + `analyze_index`。
- `analyze_full_sampling` → `analyze_columns` + `encode_selected` + `build_cms` + `build_fm_sketch`。
- `analyze_index` → `encode_selected_prefixes` + `build_histogram` + `build_cms` + `build_fm_sketch`。

Rust 的直接类型依赖只有 `cop_handler` 模块与标准库集合类型；同目录 Cargo 清单虽声明许多可选的 TiDB 子 crate，但本文件没有直接导入它们，也没有 feature 段把它们接入此实现。应用层可达性来自 `cop_handler` crate 对外暴露及其调用者，不能据 Cargo 中的可选依赖推断本文件已采用真实 statistics/codec 实现。

## 错误处理与边界

- `KvReader::scan` 的任何 `CopError` 由 `?` 原样传播；因此范围、锁或取消等底层错误的来源在 reader/调用层，而不在本文件。
- `encode_selected`、`encode_selected_prefixes` 和 `analyze_columns` 在列偏移越界时返回 `CopError::ColumnOffset(offset)`。`Mixed` 可能在完成列阶段的中间分配后于索引阶段报错，但没有外部副作用需要回滚。
- 空行集是合法输入：行数为零，列直方图仍按请求列数生成但为空，索引路径生成一个空直方图；全采样结果为空。
- `bucket_count == 0`、CMS 深或宽为零均返回空结构而非错误。`sample_size == 0` 在 `Columns` 中表示不取样，在 `FullSampling` 中特意表示保留全部行，扩展时必须保留这一上下文差异。
- 当前算术没有显式溢出处理：CMS 槽和频次使用 `u64 += 1`，`target` 的分子含 `+ bucket_count - 1`。正常 mock 数据规模下可接受，但不应把它视为恶意输入下的硬化实现。
- `encode_result` 没有返回 `Result`，因为写入内存缓冲和 `Datum::encode` 当前均不会报告错误；它只编码部分 `AnalyzeResult` 字段。消费者若需要 samples/CMS/FM，不能从 `Response.data` 还原它们。
- 与 Go 入口不同，Rust `analyze` 不检查空 ranges、不验证外层请求类型、也不执行 protobuf 反序列化；这些条件由 Rust 强类型请求模型或上游承担。

## 并发与资源生命周期

`KvReader` 要求 `Send + Sync`，所以 reader 可以由外层并发共享；但 `analyze` 本身是同步函数，没有线程、异步任务、锁、通道或后台资源。扫描借用 reader 和 ranges，请求结束前把扫描结果完整拥有到本地；所有统计集合在函数返回时释放。

确定性采样不使用共享随机数源，sketch 哈希也无共享状态，因此同一输入顺序下可复现。并发请求之间没有模块级状态竞争。不过一次分析会持有完整扫描行以及派生统计的副本，资源风险主要是峰值内存和 CPU，而不是死锁或泄漏。Go `analyzeColumnsExec` 采用 RecordSet/Chunk 式逐段扫描，Rust 当前的一次性物化在大输入上的资源特征并不等价。

## 与 Go 版本的对应关系

Rust 五个公开分支名称对应 Go `analyze.go` 的 `handleAnalyzeIndexReq`、`handleAnalyzeCommonHandleReq`、`handleAnalyzeColumnsReq`、`handleAnalyzeMixedReq`、`handleAnalyzeFullSamplingReq`，总入口对应 `handleCopAnalyzeRequest`。已明确保留的局部语义包括：直方图桶使用累计计数，以及复合索引 CMS 对每一级累积列前缀计数；这两点分别由 `histogram_bucket_counts_are_cumulative_like_go_sorted_builder` 和 `composite_index_cms_counts_every_column_prefix_like_go` 验证。

但两者不是协议或算法的完整等价移植：

- Go 从 key 解码 index/common handle、从 value 解码列；Rust 对已经解码为 `Row` 的 `KvPair.value` 统一按列偏移处理，索引路径甚至不读取 key。
- Go 使用 tipb protobuf 请求/响应以及 `statistics.SortedBuilder`、`SampleBuilder`、`RowSampleBuilder`、`CMSketch`、`FMSketch`；Rust 使用自有结构、FNV 风格哈希和自定义部分响应编码。
- Go 处理 flags、时区、field type、collation、主键 handle、column groups、sample rate、统计版本、TopN 和 API V2 key；Rust 均未实现。
- Go full sampling 的随机源按当前时间初始化；Rust 水库采样固定种子，目的是 mock 跨语言/跨运行可复现，而不是复刻随机序列。
- Go Mixed 在一次扫描中同时推进 Common Handle 和列统计；Rust 分别执行两个内存行遍历，并把索引直方图追加到列直方图向量。

因此维护者应把 Rust 实现理解为具备关键行为轮廓的测试用 mock。若调用方开始依赖真实 TiKV wire compatibility 或统计精度，必须扩大设计与测试范围，不能只修改 `encode_result` 或单个 sketch 函数来宣称完成对齐。

## 扩展指南

- 新增分析类型：扩展 `AnalyzeType`，在 `analyze` 的穷尽匹配中接线，并在独立的 `analyze_test.rs` 增加分派与边界测试；不要把测试内嵌进生产文件。
- 调整索引语义：优先修改 `encode_selected_prefixes`/`analyze_index`，同时保留并扩展复合索引前缀 CMS 测试。需要区分 index key 与 row value 时，应先调整 `KvReader`/扫描结果边界，而不是继续把 value 假设为所有请求的统一来源。
- 调整桶算法：修改 `build_histogram` 时必须验证 NULL 排除、NDV、大小、重复值、零桶、桶上限，以及累计 `count` 不变量；与 Go `statistics.SortedBuilder` 的差异应在测试名或文档中明确。
- 调整采样：注意 `sample_size == 0` 在普通列与 full sampling 中含义不同，并保持固定随机源是否仍符合 mock 可复现要求。新增采样测试仍应放在 `analyze_test.rs`。
- 调整 wire 输出：当前 `encode_result` 丢弃样本和 sketch。若要输出这些字段，应先确认所有 Rust 消费者和 Go/tipb 兼容目标，给编码加版本或切换到正式协议，并为成功解码和旧格式兼容建立独立测试。
- 提升 Go 对齐度：按真实差距分步接入 codec、collation、statistics builder、TopN/版本、protobuf；不要把 Go 的完整子系统一次性简化成新的本地桩。任何 Rust 生产修改完成后应按仓库规则同步测试、运行 `cargo fmt --all`，并保留/补充文件顶部版权头。
- 性能评估：重点检查扫描结果全量物化、复合前缀反复克隆、Mixed 双遍历、直方图克隆以及 `depth × width` CMS 分配；优化时须保持错误先后顺序和可复现性。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/store/mockstore/unistore/cophandler` 找到目标源码、模块入口、Go 对照与独立测试；`node --file .../analyze.rs --offset 1 --limit 500` 返回完整 386 行、28 个符号及 `cop_handler.rs`/`analyze_test.rs` 两个使用文件；对 `cop_handler.rs` 的 node 查询核对了 `Datum::encode`、`KvReader::scan`、`CopError` 和 `handle_cop_request → analyze`。
- Rust 源码：`pkg/store/mockstore/unistore/cophandler/analyze.rs`，核对五类分派、直方图、采样、CMS/FM、编码及错误路径。
- crate 与入口：`pkg/store/mockstore/unistore/cophandler/Cargo.toml`、`pkg/store/mockstore/unistore/cophandler/lib.rs`、`pkg/store/mockstore/unistore/cophandler/cop_handler.rs`。
- Go 对照：`pkg/store/mockstore/unistore/cophandler/analyze.go`、`pkg/store/mockstore/unistore/cophandler/cop_handler.go`，核对真实请求分派、各 builder/processor、流式扫描和 protobuf 响应。
- Rust 测试：`pkg/store/mockstore/unistore/cophandler/analyze_test.rs`，现有两项回归分别覆盖累计桶计数和复合索引逐前缀 CMS 计数；同目录 Go `*_test.go` 未检索到 Analyze 专项测试引用，因此没有把不存在的 Go 测试覆盖写成证据。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另执行任务指定的 11 章节结构检查，并人工核对本文对定位、流程、依赖、边界、Go 差异和扩展入口的说明。
