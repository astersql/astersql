# `pkg/statistics/row_sampler.rs`

## 文件定位

`row_sampler.rs` 属于 `astersql-statistics` crate，模块由 [`pkg/statistics/lib.rs`](lib.rs) 以 `mod row_sampler` 纳入，并通过 `pub use row_sampler::*` 将公开类型和函数重导出。crate 的 Go 包映射在 [`pkg/statistics/Cargo.toml`](Cargo.toml) 中声明为 `pkg/statistics`。

该文件承载 ANALYZE 所需的行级统计采样数据结构：固定容量的加权水库采样、按采样率保留行的伯努利采样、逐列及列组 FM Sketch/NULL 数/字节数累计，以及 `tipb::RowSampleCollector` 的序列化边界。RustCodeGraph 能确认文件内部的构造与调用链，但当前仓库搜索没有找到这些 API 在其他 Rust **生产文件**中的直接调用；现阶段可确认的外部 Rust 使用者是独立测试。对应 Go 文件 [`pkg/statistics/row_sampler.go`](row_sampler.go) 则被 `pkg/executor/analyze_col_sampling.go` 和 `pkg/store/mockstore/unistore/cophandler/analyze.go` 使用。因此，Go 版本位于实际 ANALYZE 主链，Rust 版本虽已由 crate 导出，其生产接线不能仅凭 Go 调用关系推断为已经完成。

## 核心职责

- 用 `RowSampleCollector` trait 统一两种采样器的合并、单行采样和公共状态访问。
- 用 `baseCollector` 保存样本、每列/列组 NULL 计数、FM Sketch、编码后总字节数、总行数和样本内存估算。
- `ReservoirRowSampleCollector` 保留权重最大的至多 `MaxSampleSize` 行，使多个局部收集器可以按相同权重规则合并。
- `BernoulliRowSampleCollector` 将权重确定性映射为 `[0, 1]` 概率，再按 `SampleRate` 决定是否保留行。
- `RowSampleBuilder::Collect` 遍历已经物化的 Datum 行，累计统计、执行采样，并复用单列列组对应列的统计结果。
- `baseCollector::{ToProto, FromProto}` 与 `RowSamplesToProto` 在内存结构和 tipb protobuf 之间转换。

## 主要符号

- `RowSampleCollector`：公开 trait。`MergeCollector` 合并一个 trait object；`SampleRow` 接收已生成的 `i64` 权重；`Base`/`BaseMut` 暴露公共状态。trait 本身没有 `DestroyAndPutToPool`，清理方法分别位于两个具体收集器上。
- `baseCollector`：公共载荷。`Samples` 是 `WeightedRowSampleHeap`，其余向量的逻辑长度应等于“列数 + 列组数”。`CollectColumns` 处理逐列统计，`collectColumnGroups` 处理多列组合，`ToProto`/`FromProto` 处理线格式。
- `ReservoirRowSampleItem`：一条样本，包含 `Handle: i64`、`Columns` 和 `Weight`。当前采样与反序列化路径都把 `Handle` 置为 `0`。
- `WeightedRowSampleHeap`：对 `Vec<ReservoirRowSampleItem>` 的封装，提供 Go 风格的 `Len/Swap/Less/Push/Pop`。实际采样实现以 `sort_by_key` 维护权重升序，索引 `0` 是最小权重；它并未实现 Rust 标准库的 `BinaryHeap`。
- `ReservoirRowSampleCollector`：`SampleRow` 在未满时插入；满后仅当新权重严格大于当前最小权重时替换。`MergeCollector` 先逐项合并统计，再把对方样本按权重重新送入水库。
- `BernoulliRowSampleCollector`：`SampleRow` 通过常数乘法混合 `weight`，用所得 `u64 / u64::MAX` 与 `SampleRate` 比较；`MergeCollector` 直接拼接双方样本。
- `RowSampleCollectorKind`：在无法返回 trait object 所有权的调用点承载两种具体收集器。
- `NewRowSampleCollector`：工厂函数；`max_sample_size > 0` 优先选择水库，即使 `sample_rate` 同时大于零；否则 `sample_rate > 0.0` 选择伯努利，两者均不启用则返回 `None`。
- `RowSampleBuilder`：配置 `ColGroups`、水库容量、采样率和 FM Sketch 容量；`Collect(&StatementContext, Vec<Vec<Datum>>)` 是文件中的最高层入口。
- `EmptyReservoirSampleItemSize` 与 `ReservoirRowSampleItem::MemUsage`：用 Rust 结构体大小加各 Datum 的内存量估算样本内存，不额外计入句柄内存。

## 执行流程

1. `RowSampleBuilder::Collect` 从第一行推断列数，计算 `total_length = columns + ColGroups.len()`，再调用 `NewRowSampleCollector`。空输入时列数被视为零，但列组仍会分配统计槽。
2. 工厂按容量/采样率选择收集器；构造函数同时按 `total_length` 初始化 `NullCount`、`FMSketches` 和 `TotalSizes`。
3. builder 为每行把 `Count` 加一，调用 `CollectColumns`：NULL 只增加对应 `NullCount`；非 NULL 插入 FM Sketch，并将 `GetBytes().len() - 1`（饱和减法）加入 `TotalSizes`，以排除编码 flag 字节。
4. `collectColumnGroups` 跳过单列组；多列组把索引指向的 Datum 组成一行交给 `FMSketch::InsertRowValue`，并仅对非 NULL 分量累计各自的去 flag 字节长度。多列组的 `NullCount` 不在此更新。
5. builder 以从 1 开始的行序号乘固定常数并右移一位生成确定性权重，然后调用具体收集器的 `SampleRow`。水库保留最大权重集合；伯努利采样再对该权重做一次混合并比较采样率。
6. 全部行处理后，builder 对每个单列列组复制源列的 FM Sketch、NULL 数和总大小，避免重复构建相同统计。
7. 分布式或分片场景可调用 `MergeCollector`：两类实现都会累加行数、NULL 数、总大小并合并 FM Sketch；水库重新竞争全局 top 权重，伯努利则直接拼接已选样本。
8. 传输时，`ToProto` 调用 `RowSamplesToProto`，NULL 明确编码为单字节 `codec::NilFlag`；`FromProto` 重建样本、统计向量与 FM Sketch，并重新求和样本 `MemUsage`。

## 数据与状态

`baseCollector.Count` 表示看过的输入行数，不由 `CollectColumns` 隐式维护，而由 builder 或上层调用者负责增加。`row_sampler_test.rs::collect_columns_does_not_own_row_count_and_excludes_flag_byte` 固化了这一所有权边界。

`NullCount`、`FMSketches`、`TotalSizes` 使用相同槽位布局：前半是原始列，后半按 `ColGroups` 顺序存列组。单列组在扫描结束后复制源列状态；多列组维护组合 FM Sketch 和总字节数，但不维护 NULL 计数。调用者必须保证行宽、列组下标和这些向量的布局一致。

水库样本始终按 `Weight` 升序排列，因此第一个元素是淘汰候选。相等权重不会替换已有样本。`MemSize` 在水库 `SampleRow` 后通过对当前样本完整求和校准；合并后则按合并前双方内存之和乘以最终样本数/候选样本数进行比例估算。`FromProto` 直接按恢复后的样本求和。伯努利 `SampleRow` 当前没有增加 `MemSize`，只有 `MergeCollector` 累加对方已有值，因此该字段在伯努利本地采样路径中通常仍为零。

protobuf 不携带 Rust 的 `MemSize` 和样本 `Handle`；恢复时句柄固定为零，内存重新计算。`FromProto` 把每个 protobuf cell 作为 `NewBytesDatum` 恢复，包括线上 NULL 的 `NilFlag` 字节；本文件不在恢复阶段进一步将它解码为 `Datum::default()`。

## 依赖与调用关系

内部主链由 RustCodeGraph 验证为 `RowSampleBuilder::Collect → NewRowSampleCollector → NewReservoirRowSampleCollector/NewBernoulliRowSampleCollector`。`Collect` 再调用 `baseCollector::{CollectColumns, collectColumnGroups}` 和具体实现的 `SampleRow`；`baseCollector::ToProto → RowSamplesToProto`；水库 `MergeCollector → SampleRow`。

直接依赖如下：

- `types::Datum` 提供 NULL 判断、编码字节和内存估算。
- `stmtctx::StatementContext` 传给 FM Sketch 插入操作，用于其错误/语义上下文。
- crate 内 `FMSketch`、`NewFMSketch`、`FMSketchToProto`、`FMSketchFromProto` 提供 NDV 草图创建、合并与编解码。
- `codec::NilFlag` 规定 NULL 的行样本线格式。
- `tipb::{RowSample, RowSampleCollector}` 和 `protobuf::RepeatedField` 构成传输边界；对应 Cargo 依赖是固定 revision 且启用 `protobuf-codec` 的 `tipb`，以及精确版本 `protobuf = 2.8.0`。
- `astersql_errors::SharedError` 是收集 FM Sketch 时的错误返回类型。

`pkg/statistics/lib.rs` 公开重导出该模块。RustCodeGraph 的 `explore` 能找到独立测试对采样、合并和 protobuf 的调用，但仓库级 Rust 搜索未找到其他生产文件直接引用这些入口；不要把 Go 的 `pkg/executor/analyze_col_sampling.go` 调用自动视为 Rust 调用。Go 侧真实上游还包括 mockstore cop handler，其结果经 tipb 在 TiKV/TiDB 分工边界传递。

## 错误处理与边界

`Collect` 只传播 `CollectColumns`/`collectColumnGroups` 内 FM Sketch 插入产生的 `SharedError`。构造选择无效（容量为零且采样率不大于零）不是错误，而是 `Ok(None)`；循环在 collector 为 `None` 时立即停止。

代码依赖若干调用者不变量而非返回错误：

- 合并时三个统计向量长度必须相同，否则 `assert_eq!` panic。
- 行宽必须与初始化槽位和第一行推断的列数一致；列组索引必须有效，否则直接索引可能 panic。
- 单列组目标槽与源列索引必须有效。
- `SampleRate` 没有显式限制到 `[0, 1]`；负数使工厂返回 `None`，大于等于 1 的值会保留所有由当前映射得到的概率，`NaN` 也因 `> 0.0` 为假而禁用采样。
- `FromProto` 假设 protobuf 各统计向量彼此语义对齐，不校验长度，也不保留句柄。
- `RowSamplesToProto` 能处理空样本并返回空向量；NULL 使用 `NilFlag`，由独立测试固定。

`MaxSampleSize == 0` 不会构造水库；若直接构造零容量水库，`SampleRow` 的显式 `self.MaxSampleSize > 0` 守卫可避免索引空样本。总大小使用 `saturating_sub(1)`，空字节 Datum 不会产生负数，这比 Go 的直接 `sizes[i]-1` 更防御性。

## 并发与资源生命周期

本文件没有锁、原子、线程、异步任务或通道；所有变更都要求 `&mut self`，因此单个收集器的并发协调必须由调用方完成。`MergeCollector` 借用来源收集器而不消费它，合并过程中会克隆样本列或整个样本列表，来源状态保持可观察。

构造函数分配统计向量和样本容量；样本拥有 `Vec<Datum>`。`DestroyAndPutToPool` 和 `baseCollector::destroyAndPutToPool` 只清空 `FMSketches`，不会重置计数、样本、大小数组或 `MemSize`；独立测试明确验证这一点。名称沿用 Go 的对象池语义，但 Rust 代码没有对象池，资源最终仍由所有权和 `Drop` 释放。若要复用实例，调用者不能把该方法误认为“恢复到全新状态”。

## 与 Go 版本的对应关系

Rust 文件以 [`pkg/statistics/row_sampler.go`](row_sampler.go) 为直接语义对照，保留了同名 collector、A-Res 最高权重保留规则、列/列组 FM Sketch、单列组复用、tipb 序列化和合并公式。Go 的 `pkg/statistics/sample_test.go::{TestWeightedSampling, TestDistributedWeightedSampling}` 对应 Rust `pkg/statistics/sample_test.rs::{weighted_sampling_keeps_highest_priority_rows, distributed_weighted_sampling_merge_matches_global_top_weights}` 的核心意图。

当前实现并非逐项等价，重要差异包括：

- Go builder 从 `sqlexec.RecordSet` 分 chunk 拉取、复制 Datum，并按字段类型/collator 把字符串转换为 collation key；Rust builder 接收已物化的 `Vec<Vec<Datum>>`，没有 RecordSet、字段类型、collator、时区或 `tablecodec` 转换。
- Go 从注入的 `rand.Rand` 生成水库权重和伯努利随机数；Rust从行序号生成确定性权重，伯努利再确定性哈希该权重。Rust测试因此验证 top 权重，而不是 Go 的多轮频率分布。
- Go 使用 `container/heap`，未满时达到容量才 heap-init；Rust每次插入/替换后排序整个 `Vec`。语义仍是保留最高权重，但复杂度由堆操作的 `O(log k)` 变为排序的 `O(k log k)`。
- Go `ReservoirRowSampleItem.Handle` 是可选 `kv.Handle` 并计入内存；Rust 是未参与采样逻辑的 `i64`，固定为零且不额外计内存。
- Go `FromProto` 接受 `memory.Tracker` 并以 buffered consume 进行配额核算；Rust只计算 `MemSize`，不连接内存 tracker，也没有快失败路径。
- Go `RowSampleCollector` interface 包含销毁方法；Rust trait 不含该方法，只在具体类型上提供。
- Go 构造器先创建容量为零的 FM Sketch slice，再由 builder 补齐；Rust构造器立即创建全部草图，并额外接收 `max_fm_sketch_size`。

因此，扩展时应以 Go 行为作为兼容目标，但必须先判断差异是尚未移植的能力还是有意的 Rust 接口设计，不能用当前简化输入模型推断完整 ANALYZE 等价。

## 扩展指南

- 增加新的逐列统计时，优先扩展 `baseCollector`、两个构造函数、两类 `MergeCollector`、`ToProto/FromProto`，并保持三个统计向量与列/列组槽位完全对齐。
- 改变采样算法时，以 `SampleRow`、水库 `MergeCollector` 和 `WeightedRowSampleHeap` 为接入点；必须同时验证局部采样后合并与一次性全局采样的保留集合一致，并评估当前全量排序的性能。
- 接入真实 ANALYZE 输入时，应围绕 `RowSampleBuilder::Collect` 增加 RecordSet/chunk、collation key、原始编码长度和错误上下文能力，而不是在 `CollectColumns` 内猜测字段类型。Go `RowSampleBuilder::Collect` 是行为基线。
- 扩充 protobuf 字段时，同步修改 `baseCollector::{ToProto, FromProto}` 和 `RowSamplesToProto`，明确 NULL 解码、句柄保留及内存核算规则，并检查固定 tipb revision 是否已提供字段。
- 调整伯努利采样时，需要明确是否追求 Go 的随机流语义；若保留确定性方案，应补充采样率边界、重复运行稳定性和分片合并测试，并修正或明确 `MemSize` 的维护合同。
- 测试必须继续放在独立文件，不内嵌进生产源。最近的目标测试是 [`pkg/statistics/row_sampler_test.rs`](row_sampler_test.rs)，算法分布/合并覆盖在 [`pkg/statistics/sample_test.rs`](sample_test.rs)；Go 对照测试在 [`pkg/statistics/sample_test.go`](sample_test.go)。至少覆盖零容量、零/越界采样率、空输入、行宽/列组下标契约、NULL、多列组、相等权重、合并、protobuf 往返和销毁后的可观察状态。
- 任何声称 Rust 已进入 executor/mockstore 生产主链的修改，都应新增可检索的 Rust 调用边及端到端验证；仅有 `lib.rs` 重导出不构成生产接线证据。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件可通过 `node --file pkg/statistics/row_sampler.rs` 完整读取。
- RustCodeGraph `explore "row_sampler.rs RowSampleBuilder RowSampleCollector"`：确认 `NewRowSampleCollector` 到两类构造器的调用边，以及 `CollectColumns`、`MergeCollector`、`ToProto/FromProto`、`RowSamplesToProto` 被测试和内部流程使用。单独的 `callers NewRowSampleCollector` 查询在超过 60 秒仍无输出后终止，调用范围由 explore 结果和仓库级精确符号搜索交叉核验。
- 已读生产与配置：[`pkg/statistics/row_sampler.rs`](row_sampler.rs)、[`pkg/statistics/lib.rs`](lib.rs)、[`pkg/statistics/Cargo.toml`](Cargo.toml)、[`pkg/statistics/row_sampler.go`](row_sampler.go)。`pkg/statistics` 下不存在 `doc.go`，因此无更近的包契约文件可读。
- 已读独立测试：[`pkg/statistics/row_sampler_test.rs`](row_sampler_test.rs)、[`pkg/statistics/sample_test.rs`](sample_test.rs) 的行采样相关用例，以及 Go [`pkg/statistics/sample_test.go`](sample_test.go) 的加权和分布式采样用例。
- 关键测试事实：`CollectColumns` 不拥有行计数且排除 flag 字节；单列列组复用源列统计；NULL protobuf 编码为 `NilFlag`；反序列化重算样本内存；销毁只释放 FM Sketch；水库保留最大权重；分片合并得到全局最高权重集合。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令确认本文恰有 11 个固定二级标题，并人工复核链接、当前接线状态、Go/Rust 差异和扩展风险均有上述源码或测试依据。
