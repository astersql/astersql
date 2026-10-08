# `pkg/statistics/sample.rs`

## 文件定位

`sample.rs` 属于 `astersql-statistics` crate；crate 由 `pkg/statistics/Cargo.toml` 定义，`pkg/statistics/lib.rs` 通过 `mod sample` 纳入模块并以 `pub use sample::*` 重导出本文件的公开符号。它位于统计信息构建链的样本层：把输入 `Datum` 汇总为计数、FM Sketch、可选 CM Sketch 和固定容量样本，随后由 `pkg/statistics/builder.rs::BuildColumn` 或 `BuildHistAndTopN` 消费这些结果来生成直方图与 TopN。

当前 Rust 主链的直接证据是 `pkg/statistics/runtime_stats_builder.rs`：运行时统计构建器创建 `SampleCollector`、逐行调用 `Collect`、维护 `Ordinal` 和内存记账，再调用 `BuildHistAndTopN`。本文件还提供较窄的批量入口 `SampleBuilder::CollectColumnStats`，其直接使用目前只在 `pkg/statistics/integration_test.rs` 中可见；它不是 Go 版本完整 `RecordSet` ANALYZE 扫描器的等价替代。

## 核心职责

- `SampleItem` 保存一个样本值、句柄占位和进入收集器时的位置；`Ordinal` 之后被 `BuildHistAndTopN` 用于列相关性估计。
- `SampleCollector` 统计非空数、空值数、总字节数和不同值估计，并以蓄水池算法把任意长度输入压缩到 `MaxSampleSize` 个样本。
- `MergeSampleCollector` 合并多个局部收集器的计数和 sketch，同时把对方样本重新纳入目标蓄水池。
- `ExtractTopN` 依据样本频次选择候选，再从 CM Sketch 查询估算频次并把选中频次从 sketch 中扣除。
- `SampleCollectorToProto` / `SampleCollectorFromProto` 在内存结构与 `tipb::SampleCollector` 之间转换，并在解码时过滤异常长样本。
- `sortSampleItems` 为后续直方图构建提供按二进制 collation 排序的样本；`SampleBuilder::CollectColumnStats` 和 `RowToDatums` 是批量收集与 chunk 行转换的辅助入口。

## 主要符号

- `SampleItem { Value, Handle, Ordinal }`：`Value` 是真实样本；`Handle` 在本文件创建样本时恒为 `0`；`Ordinal` 由 `Collect` 设为从零开始的非空输入序号，运行时列统计路径还会在 `runtime_stats_builder.rs` 中改写成原始行位置。
- `EmptySampleItemSize`：`size_of::<SampleItem>()` 的结构体静态大小，不包含 `Datum` 指向或拥有的动态载荷。`runtime_stats_builder.rs::collector_memory` 在此基础上另计 `Datum`、sketch 和 TopN 内存。
- `sortSampleItems(&mut [SampleItem]) -> Result<(), SharedError>`：用 `Datum::Compare` 和二进制 collator 排序。比较错误被保存，比较器当次返回相等；排序结束后再返回该错误，因此错误发生时切片可能已经被部分重排。
- `SampleCollector`：公开持有 `FMSketch`、可选 `CMSketch`/`TopN`、样本与各项计数；`seenValues` 和 `randomState` 是内部蓄水池状态，`IsMerger` 控制是否跳过计数/sketch 更新。
- `SampleCollector::New`：预分配非负样本容量，创建 FM Sketch，并用固定非零常量初始化 xorshift 状态；CM Sketch 和 TopN 初始为空。
- `Destroy`：清空样本和可选结构，把 FM Sketch 换成容量为零的新实例，并归零公开计数。它不会重置私有 `randomState`，所以对象虽可析构式释放状态，却不等价于重新 `New`。
- `Collect` / 私有 `collect`：前者是实际实现，后者只是内部别名。`Collect` 负责 NULL 分支、计数与 sketch、总长度，以及蓄水池插入/替换。
- `MergeSampleCollector`、`CalcTotalSize`、`ExtractTopN`：分别处理合并、按当前样本重算大小和从 CM Sketch 分离 TopN。
- `SampleCollectorToProto` / `SampleCollectorFromProto`：序列化稳定统计状态；不序列化 `MaxSampleSize`、`seenValues`、`MemSize`、`IsMerger`、随机状态、`Handle` 和 `Ordinal`。
- `MaxSampleValueLength = 32_767`：反序列化时允许保留的单个样本最大字节数。
- `SampleBuilder`：仅包含样本数与 FM/CM Sketch 尺寸；`CollectColumnStats` 接收已物化的 `Vec<Vec<Datum>>`，按首行列数创建收集器。
- `RowToDatums`：按字段类型下标从 `chunk::Row` 提取 `Datum`，返回与 `field_types` 等长的向量。

## 执行流程

1. 调用方用 `SampleCollector::New(max_sample_size, max_fm_sketch_size)` 创建收集器；需要频率统计时另行给 `CMSketch` 赋值。`SampleBuilder::CollectColumnStats` 会为每列自动创建 CM Sketch。
2. `Collect` 在普通模式下先判断 NULL。NULL 只增加 `NullCount` 并立即返回，既不进入样本，也不增加 `seenValues`、`Count` 或 sketch。
3. 非 NULL 值增加 `Count`，插入 FM Sketch；若存在 CM Sketch则插入原始字节；`TotalSize` 增加编码字节长度减一（使用 `saturating_sub(1)` 避免空字节下溢）。任一 FM Sketch 插入错误会立即向上传播。
4. 收集器增加 `seenValues`。样本槽未满时直接追加；已满且容量大于零时，先以 `MaxSampleSize / seenValues` 的概率决定是否接纳，再随机删除一个旧样本并把新样本追加到尾部。两次随机数由私有 `nextRandom` 的 xorshift 状态生成。
5. 每次正常走到采样尾部后，`MemSize` 被设置为“当前样本数 × 空 `SampleItem` 大小”；它不是完整内存用量，完整估算见 `runtime_stats_builder.rs::collector_memory`。
6. `MergeSampleCollector` 先相加计数和总大小、合并 FM Sketch，并在双方都有 CM Sketch 时合并 CM Sketch。随后临时把目标 `IsMerger` 设为 `true`，逐个重新收集来源样本，使其只参与蓄水池选择而不重复累计统计，最后恢复原值。
7. 构建统计时，`builder.rs::BuildHistAndTopN` 调用 `sortSampleItems`，使用已保存的 `Ordinal` 计算相关性，再按相等样本区间产生 TopN 候选并构建直方图。另一条 `ExtractTopN` 路径直接从当前 `Samples` 与 `CMSketch` 生成 `TopN`。
8. protobuf 往返时，编码写入空值数、非空数、FM/CM Sketch、总大小和样本字节；解码恢复这些字段并过滤长度超过 `MaxSampleValueLength` 的样本，但把样本 `Handle`/`Ordinal` 及所有临时蓄水池状态留为默认值。

## 数据与状态

`NullCount` 与 `Count` 分别表示空值和非空值数量；普通模式下二者互斥增加。`FMSketch` 只接收非空值并估计 NDV，`CMSketch` 只在存在时统计非空值频率。`TotalSize` 在收集路径中累计 `GetBytes().len() - 1`，注释说明减去的是编码标志字节；`CalcTotalSize` 则直接累加当前样本完整字节长度，语义是“样本当前大小”，不是全量输入累计大小，调用者不能混用两种解释。

蓄水池的不变量是 `Samples.len() <= max(MaxSampleSize, 0)`；当 `MaxSampleSize <= 0` 时仍更新普通统计但不保留样本。`seenValues` 只计算真正进入采样决策的值：普通模式 NULL 提前返回，合并模式则会让来源样本进入。`Ordinal` 在本文件中源于 `seenValues - 1`，所以普通收集时是非空序号；`runtime_stats_builder.rs` 为列统计把最后追加样本的 `Ordinal` 改成包含 NULL 位置在内的原始行序号。

`TopN` 与 `CMSketch` 是配套状态：`ExtractTopN` 查询每个候选的 CM Sketch 计数，调用 `SubValue` 扣除该计数，再写入并排序新的 `TopN`。这意味着该方法会修改 CMSketch，重复执行并不保证幂等。`SampleCollectorFromProto` 能从 CM Sketch protobuf 同时恢复 `CMSketch` 与其中携带的 `TopN`。

## 依赖与调用关系

上游关系：

- `pkg/statistics/runtime_stats_builder.rs` 创建 `SampleCollector`、调用 `Collect`、设置样本 `Ordinal` 并把结果交给 `BuildHistAndTopN`，这是当前 Rust 运行时统计链的主要生产使用点。
- `pkg/statistics/builder.rs::BuildColumn` 读取样本、计数、NDV、空值数和总大小；`BuildHistAndTopN` 直接调用本文件的 `sortSampleItems`。
- `pkg/statistics/integration_test.rs` 直接调用 `SampleBuilder::CollectColumnStats`，验证“采样 → 直方图”链；仓库 Rust 生产代码中未找到该简化 `SampleBuilder` 的其他直接调用。
- `pkg/statistics/lib.rs` 把全部公开符号重导出为 `astersql_statistics::*` API。

下游依赖：

- `types::Datum` 提供 NULL 判断、比较、字节表示、构造和 chunk 取值；`stmtctx::StatementContext` 为 FM Sketch 插入和比较语义提供上下文。
- crate 内的 `FMSketch`、`CMSketch`、`TopN` 及其 protobuf 转换函数承担 NDV、频率和高频值状态。
- `collate::GetBinaryCollator` 固定了 `sortSampleItems` 的二进制排序规则；`chunk::Row` 是 `RowToDatums` 的输入。
- `protobuf::RepeatedField` 与 git 固定 revision 的 `tipb` 依赖定义 wire representation；具体边界由 `pkg/statistics/Cargo.toml` 约束。
- `astersql_errors::SharedError` 是比较、sketch 插入、sketch 合并和批量收集的统一错误通道。

RustCodeGraph `query` 将 Rust 符号与同路径 Go 符号匹配，并识别 `builder.rs::BuildColumn` 等引用；本次索引中的 `callers`/`callees` 查询无输出并超时，因此上述精确调用边由全仓 Rust `rg` 和直接源码复核补齐。

## 错误处理与边界

- `sortSampleItems` 不会在比较器内部直接返回 `Result`，而是记住最后一次比较错误并临时把失败比较视为相等；最终返回错误。调用方必须在错误时放弃依赖排序结果，不能把切片视为仍保持原顺序。
- `Collect` 会传播 `FMSketch::InsertValue` 错误；错误发生在 `Count += 1` 之后，因此失败不具备事务性回滚。CM Sketch 插入及蓄水池逻辑没有可返回错误。
- `MergeSampleCollector` 会传播 CM Sketch 合并或重新收集失败。它在循环前设置 `IsMerger = true`，但使用普通 `?` 提前返回时不会恢复旧值；因此出错后目标收集器可能残留 merger 模式，这是扩展或修复时应重点覆盖的状态一致性风险。
- 合并 CM Sketch 仅在双方均为 `Some` 时发生；一方缺失不会创建、替换或报错。计数和 FM Sketch 已在 CM Sketch 合并之前修改，所以合并错误同样可能留下部分更新。
- `ExtractTopN(0)` 是严格 no-op。正数路径若 `CMSketch` 为 `None`，候选仍会进入 TopN，但计数全部为零；当前签名不返回错误。
- `SampleCollectorFromProto` 对缺失 FM Sketch 使用新建的 `MaxSketchSize` sketch，对缺失 CM Sketch 保持 `None`；超长样本被静默丢弃，`Count` 与 `TotalSize` 仍保留 protobuf 中的全量值。
- `SampleBuilder::CollectColumnStats` 以首行决定列数；空输入返回空收集器列表，后续短行因 `zip` 只填充已有对，长行的多余值被忽略，因此不验证行宽一致性。
- `RowToDatums` 信任调用者提供的字段类型数量和 row 可访问性，本函数内没有显式越界恢复或错误返回。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务或通道。所有变更 API 都要求 `&mut self`，因此单个收集器的并发同步由调用方负责；`Clone` 产生各自拥有的 Rust 状态，但其深浅语义仍取决于成员类型的 `Clone` 实现。

`Destroy` 用清空容器和替换 sketch 的方式提前释放引用；正常情况下 Rust `Drop` 也会自动回收资源。protobuf 转换创建拥有自身字节数据的新结构，解码出的样本也是新的 bytes datum。采样随机状态内嵌在收集器中，没有外部 RNG、锁或种子注入接口；固定初始种子使相同调用序列可复现，但合并顺序会改变随机消费顺序和最终样本。

内存生命周期上，`MemSize` 仅是本文件维护的粗略槽位计数；生产路径通过 `runtime_stats_builder.rs::collector_memory` 计算更完整的动态占用，并用 RAII guard 在正常返回、错误或 unwind 时释放外部 tracker 额度。修改 `SampleItem` 布局或新增动态字段时必须同步审查 `EmptySampleItemSize` 的消费者和完整内存估算。

## 与 Go 版本的对应关系

Rust 文件逐项对应 `pkg/statistics/sample.go` 的 `SampleItem`、`SampleCollector`、序列化、收集、合并、`SampleBuilder`、`RowToDatums` 与 `ExtractTopN`，但当前并非完全等价移植：

- Go `SampleItem.Handle` 是 `kv.Handle`，Rust 暂为 `i64` 且本文件创建时固定为零；Go 的快速 ANALYZE 句柄语义在这里没有完整表达。
- Go `sortSampleItems` 接收调用方 statement context，Rust 使用克隆的默认无警告 context；两者都用二进制 collator，但警告/类型上下文来源不同。
- Go 采样会深拷贝 `Datum` 以避免保留底层大 slice；Rust 把拥有的 `Datum` 移入样本或在合并时 clone，需按 Rust `Datum` 所有权实现理解，不能直接声称与 Go 的 GC 规避策略完全相同。
- Rust `MergeSampleCollector` 自行临时进入 merger 模式并返回错误；Go 方法依赖收集器既有 `IsMerger` 状态，且只记录 CM/collect 错误而不返回。Rust 的错误传播更显式，但存在错误路径不恢复 `IsMerger` 的风险。
- Go `SampleBuilder` 驱动 `sqlexec.RecordSet`，处理 PK builder、collation 解码/重编码、字段为空错误和 chunk 分批读取；Rust 版本只接受已物化且假定行宽一致的二维 `Datum`，没有这些生产语义。文档把它标为简化入口，不将 Go 能力归于当前 Rust 实现。
- Go `ExtractTopN` 根据字段类型和时区把旧样本编码解码后重编码，并返回转换错误；Rust 直接沿用样本 bytes，按样本出现次数选候选，且无字段类型、时区或错误返回。因此复杂类型/编码兼容仍有差距。
- Rust 的长度上限常量直接写为 `32_767`，数值对应 Go 的 `mysql.MaxFieldVarCharLength / 2`；两边都在 FromProto 时丢弃超长样本。
- Go protobuf 解码直接构造部分初始化对象；Rust 先 `New(0, MaxSketchSize)`，所以缺失 FM Sketch 时得到空 sketch，但临时采样容量仍为零。`pkg/statistics/sample_test.rs::destroy_and_proto_restore_reset_transient_collector_state` 明确验证该状态。

## 扩展指南

- 修改采样概率或替换策略时，从 `SampleCollector::Collect`、`nextRandom`、`seenValues` 和合并路径一起入手；必须维持容量上界、NULL 不采样、合并不重复计数，以及删除后追加所保持的样本顺序约定。同步扩展独立测试 `pkg/statistics/sample_test.rs`，不要把测试嵌入生产文件。
- 新增持久字段时，同时更新 `SampleCollectorToProto`、`SampleCollectorFromProto`、tipb schema 兼容性评估和 round-trip 测试；先明确该字段属于稳定统计状态还是 `seenValues`/随机状态这类不应跨边界的瞬态状态。
- 修复或增强合并时，优先给 `MergeSampleCollector` 增加错误注入后的回归测试，验证 `IsMerger` 必然恢复，并评估部分计数/sketch 更新是否需要两阶段处理。
- 增强 TopN 编码兼容时，修改 `ExtractTopN`，参考 Go 版本的字段类型/时区解码重编码流程，并同步检查 `cmsketch.rs`、`topn` 数据结构及 `sample_test.rs::extract_top_n_subtracts_selected_counts_and_zero_is_noop`。复杂类型、hash 碰撞和重复调用是主要正确性风险。
- 若要把 `SampleBuilder` 接入完整 ANALYZE 扫描，不能只扩充二维输入循环；还需对齐 Go 的 RecordSet/chunk 生命周期、PK 列处理、collation key、字段数量检查和错误上下文，并在独立集成测试中覆盖。当前生产路径更多由 `runtime_stats_builder.rs` 承担，接线前应避免形成两套语义分叉。
- 修改 `SampleItem` 或内存统计时，同步审查 `EmptySampleItemSize`、`runtime_stats_builder.rs::collector_memory` 和 tracker 释放测试，防止性能记账漂移。
- 修改排序或 `Ordinal` 时，同步验证 `builder.rs::BuildHistAndTopN` 的相关性计算；排序错误、NULL 行位置和替换后追加都会影响相关性，不应只验证直方图桶数。

## 验证依据

- 源码全貌：`pkg/statistics/sample.rs`，核对了 2 个 struct、2 个常量、4 个自由函数、`SampleCollector`/`SampleBuilder` 的全部方法及无条件编译事实；文件没有 `cfg` 条件项。
- crate 边界：`pkg/statistics/Cargo.toml` 与 `pkg/statistics/lib.rs`，确认 crate 名、`lib.rs` 模块装配、公开重导出，以及 `types`、`stmtctx`、`chunk`、`collate`、`protobuf`、`tipb` 等直接依赖。
- 生产调用：`pkg/statistics/runtime_stats_builder.rs` 与 `pkg/statistics/builder.rs`，确认 `Collect`、`EmptySampleItemSize`、`sortSampleItems`、样本字段及 `BuildHistAndTopN` 的实际消费关系。
- Rust 测试：`pkg/statistics/sample_test.rs` 验证蓄水池容量、NDV、protobuf 往返、NULL/计数、TopN 扣减与零上限、Destroy 和瞬态状态；`pkg/statistics/integration_test.rs` 验证简化批量采样到直方图管线；`pkg/statistics/go_merge_47_test.rs` 提供样本进入统计构建的补充证据。
- Go 对照：`pkg/statistics/sample.go` 与 `pkg/statistics/sample_test.go`，核对完整 RecordSet 收集、合并、序列化、长度过滤、TopN 编码和对应边界测试；差异已在上节逐项列出。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/statistics/sample.rs` 返回完整 324 行及 16 个使用文件；`query` 确认 Rust/Go 同名符号并识别 `builder.rs::BuildColumn`。`callers`/`callees` 在本索引上无输出且超时，故直接边由全仓 `rg` 与上述源码人工复核，不据此声称更多调用者。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前运行固定 11 章节结构命令，并人工复查只有 `pkg/statistics/sample.rs.md` 是生产物、未修改 `plan.md`、没有把 Go 专属能力描述为 Rust 已支持。
