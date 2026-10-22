# `pkg/statistics/fmsketch.rs`

## 文件定位

`fmsketch.rs` 属于 `astersql-statistics` crate（`pkg/statistics/Cargo.toml`），实现统计信息采集使用的 Flajolet–Martin 基数草图。模块在 `pkg/statistics/lib.rs` 中以私有 `mod fmsketch` 装配，再通过 `pub use fmsketch::*` 对 crate 使用者重导出。

它位于 ANALYZE/统计构建链的数据结构层：`pkg/statistics/row_sampler.rs` 为每列和列组维护草图，`pkg/statistics/sample.rs` 在列样本收集及合并时维护草图，`pkg/statistics/runtime_stats_builder.rs::encode_fm_sketch` 为运行时构建的列或索引统计生成落盘字节。`pkg/server/extract_runtime.rs` 和统计存储读取测试还会消费其 protobuf 编解码 API。

## 核心职责

- 用 `FMSketch` 保存经过 MurmurHash3 哈希且满足当前采样层级的唯一 `u64`，在有界空间内近似不同值数量（NDV）。
- 用 `mask` 控制采样率；集合超过 `maxSize` 时提升层级并淘汰不再满足掩码的哈希。
- 对单个 `types::Datum` 和复合行值使用与语句时区、错误策略一致的 codec 编码，然后哈希并插入。
- 合并分片/采样收集器产生的草图，先统一到更稀疏的层级，再合并哈希集合。
- 在内存结构与 `tipb::FmSketch`、protobuf 字节之间转换，支持分布式 ANALYZE 传输及统计持久化。
- 提供 crate 内复用的 `murmur3Sum128`；`pkg/statistics/cmsketch.rs`、`sample.rs` 和 `histogram.rs` 也依赖它生成一致哈希。

## 主要符号

- `MaxSketchSize: usize = 10_000`：从 protobuf 字节恢复草图时采用的默认容量。该容量没有存入 protobuf；源码和 Go 对照均把它作为固定回填值。
- `FMSketch { hashset, ndvCounts, mask, maxSize }`：核心状态。字段为 `pub(crate)`，crate 外只能通过构造、插入、合并、查询和编解码 API 操作。
- `NewFMSketch(max_size)`：创建空草图并按上限预分配 `HashMap`。
- `FMSketch::Copy`、`NDV`、`MemoryUsage`：分别深拷贝、普通草图返回 `(mask + 1) * hashset.len()`，sampled 草图转调 `sampledNDV`；逻辑内存量为 `80 + 30 * len + (sampled ? 24 : 0)` 字节，属于与 Go 对齐的估算而非实际堆占用。
- `FMSketch::insertHashValue`：内部不变量维护入口；过滤不满足掩码的值、去重、超限时执行 `mask = mask * 2 + 1` 并压缩集合。
- `InsertValue`、`InsertRowValue`：公开插入入口，分别调用 `hashDatum` 和 `hashRow`；失败以 `astersql_errors::SharedError` 返回。
- `MergeFMSketch`：合并另一草图。若对方 `mask` 更大，先提升本草图的 `mask` 并过滤旧集合，再累计对方存在的 `ndvCounts`，通过 `mergeHashValue` 合并哈希及重复标志。
- `encodeValue`、`hashDatum`、`hashRow`：内部编码/哈希链。`hashRow` 逐个编码分量并按顺序拼接，因而列顺序属于哈希语义。
- `murmur3Sum128`、`murmur3Sum64`、`fmix64`：MurmurHash3 x64 128 位混合流程及 64 位摘要；草图取返回值的第一半 `h1`。
- `FMSketchToProto`、`FMSketchFromProto`：转换 `tipb::FmSketch`。输出保存 `mask`，按 bool 拆分 `hashset`（singleton）和 `multi_hashset`（repeated），不保存 counts 或 `maxSize`。普通 `FMSketchFromProto` 仅读取 `hashset` 并置标志为 false，不读取 repeated 集合；sampled 恢复必须走 `newSampledFMSketch`。
- `EncodeFMSketch`、`DecodeFMSketch`：protobuf 字节接口。`None` 分别对应空字节和无草图；解码成功后将 `maxSize` 回填为 `MaxSketchSize`。
- `mask`、`hash_values`：用于观测当前层级与稳定读取集合；后者排序后返回用于观测和断言；`FMSketchToProto` 直接遍历 HashMap，不保证字节序列稳定。

## 执行流程

1. 调用者通过 `NewFMSketch` 创建草图；采样收集器通常按列数/列组数创建一组实例。
2. `InsertValue` 或 `InsertRowValue` 把 Datum 交给 `encodeValue`。编码使用 `StatementContext::TimeZone()`；编码错误再交给 `StatementContext::HandleError`，由当前 SQL 错误策略决定返回错误还是忽略并用空字节继续。
3. `hashDatum` 对单个编码结果取 `murmur3Sum128(...).0`；`hashRow` 按输入顺序拼接每个分量的独立编码后再取相同摘要。
4. `insertHashValue` 先检查 `hash & mask == 0`。通过者写入 `HashMap<u64, bool>`；bool 记录样本内重复，重复值不会增加集合大小，sampled 目标再次遇到哈希则将标志置 true。
5. 若集合大小超过 `maxSize`，掩码从 `2^r - 1` 变为 `2^(r+1) - 1`，随后仅保留满足新掩码的哈希。普通 `NDV` 用新采样倍率乘保留项数；sampled `NDV` 另按 singleton 与行数做 GEE 估计。
6. 分片合并时，`MergeFMSketch` 选择两者中较大的 `mask` 作为基准，先过滤本地旧项，再逐项插入对方集合；插入过程仍可因容量上限继续提高层级。
7. 普通落盘仍写 FmSketch protobuf；sampled 落盘由 `EncodeFMSketch` 转调 `encodeSampled`，写 `[0, 1] + RowSampleCollector protobuf`，保留计数与两组哈希。`DecodeFMSketch` 根据首字节 0 分流，并将恢复容量设为 10,000。

应用链的直接证据包括：`row_sampler.rs::baseCollector::CollectColumns/collectColumnGroups` 插入列和列组，两个 `MergeCollector` 实现合并分片草图；`sample.rs::SampleCollector::Collect/MergeSampleCollector` 维护列采样草图；`runtime_stats_builder.rs::encode_fm_sketch` 区分列值与完整索引行后编码为持久化字节。

## 数据与状态

`ndvCounts: Option<NdvCounts>` 的存在性决定 sampled 模式。普通空 root 合并 sampled 输入时升级为 sampled，累加 rows/samples/nulls；普通输入没有计数可累加，因此混入非空全量草图不能推定其行数已计入 GEE 分母。跨分区不同采样率应使用 `partition_ndv.rs`，不要用简单累计代替各分区 singleton scale。`Copy` 会克隆 map 和可选计数。

`hashset` 的关键不变量是所有元素均满足 `value & mask == 0`。正常构造、插入和合并路径会维持该条件；`FMSketchFromProto` 信任输入 proto，不主动按其 `mask` 过滤，因此调用者应把 protobuf 看作受协议约束的数据，而不是任意集合修复接口。

`mask` 从 0 开始，只由 `mask = mask * 2 + 1` 提升，因此正常运行中依次为 0、1、3、7 等低位连续为 1 的值，对应保留概率约为 `1 / (mask + 1)`。NDV 是估计量，不保证等于真实基数；普通草图的小集合在未压缩时等于去重后的哈希数量；sampled 草图还会应用 GEE 放大及非 NULL 行数上限。

`maxSize` 是运行时容量策略，不属于 wire format。`FMSketchFromProto` 直接产生的对象把它设为 0；`DecodeFMSketch` 会回填 `MaxSketchSize`。因此若要继续向 proto 直接转换得到的草图插值，应先明确容量策略；`row_sampler.rs::FromProto` 的普通分支与 `sample.rs::SampleCollectorFromProto` 保留此直接转换行为；sampled 构造则直接设为 `MaxSketchSize`。

`HashMap<u64, bool>` 拥有哈希值，`Clone`/`Copy` 会深拷贝集合。`hash_values` 返回新的排序向量。结构体没有借用外部缓冲区或全局状态。

## 依赖与调用关系

上游直接调用者：

- `pkg/statistics/row_sampler.rs`：列/列组采集、tipb 转换及水库/伯努利收集器合并。
- `pkg/statistics/sample.rs`：`SampleCollector` 的创建、收集、合并和 tipb 转换。
- `pkg/statistics/runtime_stats_builder.rs`：从文本化运行时行构造 Datum，并生成 FM Sketch 存储字节。
- `pkg/server/extract_runtime.rs`：解码持久化字节后重新转为 proto。
- `pkg/statistics/handle/handletest/handle_test.rs`：验证存储读取链得到的 FM Sketch。

下游依赖由 `pkg/statistics/Cargo.toml` 声明：`types` 提供 Datum，`stmtctx` 提供时区与错误处理，`codec` 提供值编码，`tipb` 定义 wire message，`protobuf = 2.8.0` 执行字节序列化，`astersql-errors` 统一错误类型。标准库 `HashMap` 负责去重与保留过滤。

冻结版本的调用关系通过 Git blob 与符号引用核对；sampled 算法和跨分区状态分别位于 `sampled_ndv.rs` 与 `partition_ndv.rs`。

## 错误处理与边界

带首字节 0 的输入交给 sampled 解码器；它拒绝不支持的版本、缺失采样数或 sketch/NULL 向量长度不为 1 的 payload。普通 `FMSketchFromProto` 不恢复 `multi_hashset`，不能用于 sampled 持久化恢复。sampled 数值边界与零样本行为见 `sampled_ndv.rs.md`。

- Datum 编码错误先经过 `StatementContext::HandleError`。若其返回错误，插入失败且草图不变；若策略吞掉错误，`encodeValue` 返回空字节并继续哈希。扩展此处必须保持 SQL 模式/警告策略与 Go 版本一致。
- protobuf 写入、解析错误被转换为 `astersql_errors::New(error.to_string())`；无额外上下文包装。非法字节会返回 `Err`，不会产生部分草图。
- `EncodeFMSketch(None)` 返回空 `Vec`，但 `DecodeFMSketch(Some(&[]))` 会尝试解析空 protobuf；它与 `DecodeFMSketch(None)` 的“无对象”语义不同。
- `NewFMSketch(0)` 后首次插入就会触发提升和过滤；这被 `sample.rs::Destroy` 用作清空后的占位状态，不应误当作正常采集容量。
- 算术按源码类型执行：掩码更新和 NDV 转换没有显式溢出错误。对极端规模或恶意 proto 输入进行强化时，需要同时评估 Go 的 `uint64/int64` 行为和 wire 兼容性。
- `MemoryUsage` 是与 Go 对齐的估算口径，不包含 Rust `HashMap` 的桶、分配器和装载因子开销，不能当作实际堆占用。

## 并发与资源生命周期

`FMSketch` 不包含锁、原子变量、通道、异步任务或后台资源。所有变更方法要求 `&mut self`，Rust 借用规则保证同一实例不会在安全代码中并发写；跨线程共享时由调用者选择互斥或所有权转移策略。

构造时 `HashMap::with_capacity` 申请容量，结构体销毁时由 RAII 自动释放；`Copy`、proto 转换和 `hash_values` 会分配独立集合或向量。与 Go 版本为减少哈希器分配而使用 `sync.Pool` 不同，Rust 在栈上执行自有 MurmurHash3 状态混合，没有池化资源，也没有归还步骤。

合并是同步、原地操作，期间不会生成任务或持有外部锁。调用者若需要并行合并，应在外部先分片，再串行取得目标草图的独占可变访问。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/statistics/fmsketch.go`，独立测试是 `pkg/statistics/fmsketch_test.go`；Rust 独立测试位于 `pkg/statistics/fmsketch_test.rs`，没有把测试内嵌到生产源文件。

核心语义保持一致：字段含义、`MaxSketchSize = 10000`、掩码升级公式、集合过滤、NDV/内存估算、合并时取较大掩码、tipb 字段以及解码后回填容量均与 Go 对应。Rust 用 `Option` 表示 Go 的 nil：`FMSketchFromProto(None)` 与 `DecodeFMSketch(None)` 返回 `None`，`FMSketchToProto(None)` 返回空消息，`EncodeFMSketch(None)` 返回空向量。

实现层差异包括：Go 用 `map[uint64]bool` 与 `maps.DeleteFunc`，Rust 用 `HashMap<u64, bool>` 与 `retain`；Go 用 `sync.Pool` 复用 `twmb/murmur3.New64`，Rust 内置 x64-128 算法并取 `h1`；Go 的指针接收者显式允许部分 nil 调用，Rust 方法只能在实际对象上调用。Rust 与 Go 的 proto 输出均遍历 map，不承诺序列化字节顺序；集合语义和解码结果不依赖顺序。`hash_values` 的排序只用于显式调用该辅助方法的场景。

测试证据：Rust `fmsketch_test.rs` 覆盖 200 个值触发压缩与字节往返、小容量去重/上限、proto 重复项去重和 `None` 契约、不同 mask 合并；Go `fmsketch_test.go` 还用固定统计样本核对 NDV 数值、合并结果和编码前后 NDV。`pkg/statistics/cmsketch_test.rs` 直接使用 `murmur3Sum128`，说明该哈希实现也是同 crate 其他草图的兼容基础。

## 扩展指南

- 修改采样算法或容量策略时，优先从 `insertHashValue`、`MergeFMSketch` 和 `NDV` 入手，并保持“集合元素满足 mask”“合并后采用不低于任一输入的层级”两个不变量。
- 修改值哈希时必须同时审查 `encodeValue`、`hashDatum`、`hashRow`、`murmur3Sum128`，并核对 `cmsketch.rs`、`sample.rs`、`histogram.rs` 的复用。哈希或编码变化会影响历史统计兼容、跨 Go/Rust 节点合并和结果稳定性。
- 增加 wire 字段（例如持久化 `maxSize`）需要同步 tipb 定义/依赖版本、`FMSketchToProto/FromProto`、`Encode/Decode` 以及 Go 对照；旧消息缺省值和滚动升级兼容必须明确。
- 新增行为测试应放在独立的 `pkg/statistics/fmsketch_test.rs`，并同步对照 `pkg/statistics/fmsketch_test.go` 的原测试意图；不要把测试加入生产文件。涉及上游采样/持久化链时，再扩展最近的 `row_sampler_test.rs`、`sample_test.rs`、`runtime_stats_builder` 相关测试或 handle 存储测试。
- 性能评审重点是每值编码分配、`hashRow` 拼接、压缩时全表 `retain`、合并逐项插入以及 `hash_values` 排序的临时分配。任何优化都应先证明不改变哈希字节序列、掩码层级和 proto 兼容性。

## 验证依据

增量包括 HashMap 重复标志、可选 sampled counts、NDV 分流、合并计数、内存估算及 sampled envelope。完整读取冻结 `fmsketch.rs`，核对 `sampled_ndv.rs`、`partition_ndv.rs`、`row_sampler.rs`、`lib.rs`、`Cargo.toml` 和 Go 同名文件。`fmsketch_test.rs::sampled_ndv_merges_hash_multiplicity_and_row_counts` 静态断言双向合并 NDV=8、NULL=20、MemoryUsage=224、零样本以及普通 root 升级；`global_stats_sampled_ndv` 验证 `[0,1]` envelope、旧 protobuf 拒绝及无效 payload。

本文以冻结范围 `ad0a6f48804e48373cc79c5f53f0072daf99ceb9..2c944fbcbfb2c904cc8cbddafe17954138bd8867` 为准：通过 `git diff ad0a6f4..2c944fb -- pkg/statistics/fmsketch.rs` 审计增量，并用 `git show 2c944fb:路径` 读取目标源码、模块入口、Cargo、测试及 Go 对照。RustCodeGraph `status` 当前含 Rust，但图反映工作区而非冻结提交，未用它证明冻结版本调用关系；关系通过 `git grep 2c944fb` 和对应 blob 交叉核实。`pkg/statistics/doc.go` 不存在。

纯文档 Ready 等价验证：七篇说明均检查固定 11 个二级标题、路径与空白，执行任务指定的结构命令和 `git diff --check`。`.agents/skills/tidb-verify-profile` 缺失；按计划不运行 Cargo、构建或代码测试，测试仅作静态证据，不声称运行通过。
