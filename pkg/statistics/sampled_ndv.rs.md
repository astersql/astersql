# `pkg/statistics/sampled_ndv.rs`

## 文件定位

在 FM 保留哈希之外携带行数和 singleton/repeated 信息，使用 GEE 估算总体非 NULL NDV，并提供持久化封装。

## 核心职责

- 组合 `tipb::FmSketch` 两组哈希与全量/样本/NULL 计数。
- 用 FM 估算的样本 distinct 和 singleton 数推算总体 NDV。
- 区分 sampled 与普通草图，提供观察到的非 NULL 行数。
- 以带版本 envelope 保存计数和哈希，校验恢复所需字段。

## 主要符号

- `NdvCounts`：crate 可见、可复制，含 `rows`（可见总行数）、`samples`（NDV 样本行数，包含 NULL）、`nulls`（缩放至全量的 NULL 行数）。
- `newSampledFMSketch`：内部构造，先插入 singleton=false，再插入 repeated=true；重叠哈希最终按 repeated 处理。
- `NewSampledFMSketch`：接受 proto 和三项计数的公开包装。
- `NewSampledFMSketchFromHashes`：从 singles/multis 切片构造 proto 后调用公开包装。
- `FMSketch::Sampled`、`NotNullRows`：报告计数是否存在，以及 sampled 的 rows-nulls；普通草图后者退回自身 NDV。
- `sampledNDV`：crate 内的 GEE 估计实现，由 `FMSketch.NDV()` 调用。
- `encodeSampled`、`decodeSampledFMSketch`：sampled 字节封装与恢复。
- `sampledFMSketchMarker=0`、版本=1、头长=2：固定 envelope 格式。

## 执行流程

1. 构造函数复制两组哈希到 `HashMap<u64,bool>`，设置 `ndvCounts=Some`、proto mask 和 `maxSize=MaxSketchSize`（10,000）。它不做插入压缩或 mask 过滤。
2. `sampledNDV` 在 samples=0 时直接返回 0，避免除零。
3. 令 `w=mask+1`，估算样本 distinct：`d=min(samples, w*hashset.len())`；计算 singleton 数并令 `f1=min(d, w*singles)`。
4. 输出 `min(round(d + (sqrt(rows/samples)-1)*f1), rows-nulls)`。NULL 比例假定在样本与总体一致，放大比例因而使用 rows/samples。
5. 编码为 `[0,1] + RowSampleCollector protobuf`，其中 count、ndv_sample_count、单项 null_counts 和单项 fm_sketch 保存恢复所需状态。
6. 外层 `DecodeFMSketch` 检查 marker 后调用本解码器；解码器检查头/版本，解析 collector，要求一个草图、sample count 字段存在和一个 NULL 计数，再重建 sampled sketch。

## 数据与状态

sampled 模式由 `Option` 的存在性决定，samples=0 仍为 sampled。计数独立于行采样器生命周期，序列化后可用于全局统计；大小字节数不参与 NDV。singleton/repeated 是针对样本观察的哈希频次，不能直接解释为总体唯一/重复值。

`d` 不超过样本行数，`f1` 不超过 d，最终估计不超过非 NULL 总行数。这些是合法非负计数下的上限；代码未显式施加零下限，也未验证 nulls<=rows 或 samples<=rows。采样误差、FM 抽样、哈希碰撞和四舍五入都可能影响结果，不能把它称为精确 distinct 计数。

## 依赖与调用关系

直接依赖 `crate::{FMSketch, MaxSketchSize}`、tipb、protobuf、标准 HashMap 和 `astersql_errors`。没有新 Cargo 依赖或 feature 分支。

`row_sampler.rs` 恢复采样响应，`fmsketch.rs` 提供 NDV 和字节入口；`partition_ndv.rs` 使用 counts 计算每分区 singleton scale。`handle/globalstats/global_stats.rs` 通过 detailed FMSketch 接入分区合并，`handle/storage/dump_test.rs` 使用公开 FromHashes 构造器验证存储。这些关系区别于 Go executor 远端扫描主链，不能以模块导出替代调用证据。

## 错误处理与边界

零 samples 直接估计为 0；合法全 NULL 且空哈希输入也得到 0。`NotNullRows` 对 sampled 返回 rows-nulls，普通返回估计 NDV，并非普通草图实际扫描行数。

短头或版本不为 1 返回 `unsupported sampled NDV sketch format`；protobuf 解析失败转换为共享错误；缺失 sample count、草图数或 NULL 计数数目不为 1 返回 `invalid sampled NDV sketch payload`。该函数自身不再检查 marker，依赖外层分流；数值合法性和 mask/哈希一致性没有进一步验证。

marker 0 是无效 protobuf tag，使旧的普通草图解码器拒绝 sampled 数据；它是格式区分，不意味着旧消费者可透明读取。普通 `FMSketchFromProto` 不恢复 counts/repeated，必须选择正确接口。

## 并发与资源生命周期

没有线程、锁或外部资源。构造复制哈希，计数按值持有；读估计用不可变借用。编码创建 collector、payload 和带头 Vec，解码创建拥有自身 map 的新对象。内存由 Drop 回收，没有 tracker 或对象池；复杂度主要为哈希复制、singleton 扫描和 protobuf 分配。

## 与 Go 版本的对应关系

Go `Sampled` 和 `NotNullRows` 支持 nil 接收者，分别返回 false/0；Rust 方法要求实际实例，无 nil 接收者。Rust 额外提供两个公开构造包装供其他 crate 和测试使用，内部 counts 仍为 crate 可见。

## 扩展指南

修改估计时保留样本数包含 NULL 的合同、d/f1 上限和总体非 NULL 上限；对照 Go 原测试并在独立 `fmsketch_test.rs` 扩展用例。修改 wire format 时同时核查外层 marker 分流、版本拒绝和普通格式兼容，覆盖短头、缺字段、额外 sketch/NULL 项及双向持久化恢复。

不同采样率的分区合并使用 `PartitionNDVMerger` 的每分区 scale，不应只累计 counts 后套用单个比例。唯一键 schema 保证通过 `SampleCollector.Unique` 在构建阶段覆盖估计，不修改本模块公式。
