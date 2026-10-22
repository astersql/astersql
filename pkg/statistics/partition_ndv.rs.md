# `pkg/statistics/partition_ndv.rs`

## 文件定位

本文件属于 `pkg/statistics/Cargo.toml` 定义的 `astersql-statistics` crate，由 `lib.rs` 的 `mod partition_ndv` 和 `pub use partition_ndv::*` 装配、导出。它负责从各分区的 FM Sketch 估算全局 NDV，保留各分区不同采样率的 GEE 放大系数。

冻结提交中存在生产调用：`pkg/statistics/handle/globalstats/global_stats.rs` 在各分区都有 detailed sketch 时构造并喂入 `PartitionNDVMerger`，使用其 NDV 并限制到全局行数；局部唯一键则走单独的 schema 推导分支。这里不是直方图或 TopN 合并器。

## 核心职责

- 普通草图合并继续使用 `FMSketch::MergeFMSketch`。
- 遇到 sampled 与其他分区合并时，转为每哈希一个 distinct 权重的 map。
- 仅单个分区样本内出现一次、且其他分区未见的哈希使用该分区的 `sqrt(rows / samples)`；样本内重复或跨分区出现的哈希贡献 1。
- 对齐掩码、容量超限时继续压缩，输出带 FM 抽样误差的估计值。

## 主要符号

- `PartitionNDVMerger`：`Default` 构造空状态，保存 `sketch: Option<FMSketch>` 与 `distinct_per_hash: Option<HashMap<u64, f64>>`。
- `Merge(FMSketch)`：消费分区草图，不返回 `Result`。
- `NDV()`：空状态返回 0，单草图或普通合并返回草图 NDV，转换后返回 `round((mask + 1) * sum(weights))`。
- `Sampled()`：单个 sampled 草图或已转换为 per-hash map 时为真。
- `mask()`：只读诊断接口，空状态为 0。
- `initDistinctPerHash()`：把已有草图的哈希/重复标志转换为权重并清除旧哈希和计数。
- `singletonScale()`：普通草图为 1，sampled 草图为 `sqrt(rows / samples)`。

## 执行流程

1. 首个输入直接移入 `sketch`，不建立额外 map，保留单分区 sampled NDV 的行数上限。
2. 后续输入若双方都是普通草图，直接原地合并。
3. 任一方 sampled 时，`initDistinctPerHash` 将当前每个重复哈希映射为 1，singleton 映射为当前分区 scale，然后清空原草图 hashset 和 ndvCounts。
4. 新草图掩码更大时先提升当前 mask，并过滤已有 per-hash map。
5. 遍历新草图中满足 mask 的哈希：重复或 map 已含该哈希则写 1，否则写输入分区 scale。
6. map 大小超过首个草图保留的 `maxSize` 时，将 mask 变为 `mask * 2 + 1` 并过滤。
7. `NDV` 对权重求和、乘 mask 倍率、四舍五入。合并器自身不施加全局行数上限，上游负责限制。

## 数据与状态

转换前 `sketch` 拥有有效草图；转换后其中仅 mask/maxSize 用作合并状态，哈希的权威副本是 `distinct_per_hash`。清空 ndvCounts 后 `Sampled()` 依赖 map 的存在性，不能只看内部草图的 `Sampled()`。

正常输入的保留哈希满足 `hash & mask == 0`。同一哈希无论跨多少分区重复都贡献 1，不可把多个分区 singleton 的 scale 相加。普通分区提供已知出现值，scale 恒为 1。NDV 仍受哈希碰撞、FM 压缩、采样代表性及浮点取整影响，不是精确计数。六种输入顺序的等值测试证明指定样例，不能推导任意容量、浮点输入下字节级或绝对数值不变。

## 依赖与调用关系

本文件直接依赖标准库 HashMap、crate 内 `FMSketch` 及其 crate 可见字段；没有额外外部依赖或 feature gate。`sampled_ndv.rs` 提供计数和 sampled 构造，`fmsketch.rs` 提供普通合并。

生产上游 `handle/globalstats/global_stats.rs` 仅在 `has_complete_sketches` 成立时将每个 detailed sketch 的克隆移交本合并器；缺失 detailed 数据时回退既有聚合估计。独立测试位于 `fmsketch_test.rs` 和 `handle/globalstats/global_stats_test.rs`，没有同名 `partition_ndv_test.rs`。

## 错误处理与边界

空合并器返回 NDV=0、Sampled=false；单分区直接走 `FMSketch.NDV`，因此保留 sampled 的 `rows - nulls` 上限。多分区转换后没有同样的上限或额外行数计数。

内部 `expect` 依赖 Merge 状态机建立 sketch/map；公开接口不能传空指针。`singletonScale` 没有 samples=0 守卫，合法零样本草图应为空，不会写入无效权重；异常非空零样本、负计数、无效 mask 和算术溢出没有专门错误返回。不能把 `sampledNDV` 的零样本保护推广到本函数。

## 并发与资源生命周期

没有锁、线程、异步任务或 tracker。合并需要 `&mut self`；`Merge` 通过 Rust 所有权消费输入，来源调用者若要继续保留需先克隆。转换时 `HashMap::clear` 清除条目但可能保留容量，与 Go 直接设 nil 的释放行为不同；per-hash map 与旧草图最终由 Drop 回收。

转换和掩码提升会遍历 map，容量超限也执行 retain；空间随保留哈希和 HashMap 分配容量变化。没有实测内存或性能证据。

## 与 Go 版本的对应关系

对应 `pkg/statistics/partition_ndv.go`：延迟转换、分区 singleton scale、重复哈希归一、mask 对齐、容量压缩和最终 round 公式一致。Go `Merge(nil)` 是 no-op，Rust 通过值类型排除 nil；Go 的私有 `insertHash` 逻辑在 Rust 的 Merge 循环内实现。Go nil 接收者相关语义由 Rust 空 `Option` 状态分支表达。

Go 将旧 hashset 设 nil，Rust clear 可能保留分配；Rust 额外公开 `mask()` 供诊断。Go 异步全局合并也使用本算法，但不能据此宣称冻结 Rust 拥有相同完整异步生产链。

## 扩展指南

修改算法应同时检查普通直合并与 sampled 转换两条路径，保持跨分区重复归一和每分区 scale，避免用汇总 rows/samples 代替各自比例。扩展独立 `fmsketch_test.rs` 的混合普通/sampled、不同 mask、容量压缩、六种顺序、单分区 NULL 上限场景，并核对 Go 原测试。

接线变更检查 globalstats 的 detailed 完整性、局部唯一键优先级和上游行数上限。任何零样本/非法计数强化必须单独验证 Go 兼容边界；本次只记录事实，不修改算法。

## 验证依据

冻结新增文件完整 110 行及对应 Go 文件直接证明上述状态机。`fmsketch_test.rs::global_stats_sampled_ndv` 覆盖三分区六种顺序 NDV=7、全量草图合并 NDV=2、容量压缩 mask=1/NDV=10、再对齐 mask=3/NDV=20，以及单分区 rows=10/nulls=9 的 NDV=1。`handle/globalstats/global_stats.rs` 的 complete-sketch 分支与 `global_stats_test.rs::global_stats_sampled_ndv` 提供生产接线及静态测试依据。

本文以冻结范围 `ad0a6f48804e48373cc79c5f53f0072daf99ceb9..2c944fbcbfb2c904cc8cbddafe17954138bd8867` 为准：通过 `git diff ad0a6f4..2c944fb -- pkg/statistics/partition_ndv.rs` 审计增量，并用 `git show 2c944fb:路径` 读取目标源码、模块入口、Cargo、测试及 Go 对照。RustCodeGraph `status` 当前含 Rust，但图反映工作区而非冻结提交，未用它证明冻结版本调用关系；关系通过 `git grep 2c944fb` 和对应 blob 交叉核实。`pkg/statistics/doc.go` 不存在。

纯文档 Ready 等价验证：七篇说明均检查固定 11 个二级标题、路径与空白，执行任务指定的结构命令和 `git diff --check`。`.agents/skills/tidb-verify-profile` 缺失；按计划不运行 Cargo、构建或代码测试，测试仅作静态证据，不声称运行通过。
