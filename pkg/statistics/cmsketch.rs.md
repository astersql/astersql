# `pkg/statistics/cmsketch.rs`

## 文件定位

本文件属于 `astersql-statistics` crate。`pkg/statistics/Cargo.toml` 将 crate 根设为 `lib.rs`；`pkg/statistics/lib.rs` 以 `mod cmsketch` 装配本模块，并用 `pub use cmsketch::*` 将其公开类型和函数重导出。它实现统计信息中的点频率估计结构：用 `CMSketch` 保存普通值的近似频数，用 `TopN` 保存高频值的精确频数，并负责二者的样本构建、查询、合并和 tipb protobuf 编解码。

在应用链路中，`pkg/statistics/index.rs::Index::QueryBytes` 按“TopN → CMSketch → Histogram”查询编码键；`pkg/planner/cardinality/row_count_column.rs` 在旧版列统计的等值估计中调用 `QueryValue`；`pkg/statistics/sample.rs` 使用 `CMSketchToProto`、`CMSketchAndTopNFromProto` 在采样收集器与 tipb 之间传输草图。因此本文件不是独立算法工具，而是 ANALYZE 产物进入优化器基数估计链路的基础数据结构。

## 核心职责

- `NewCMSketchAndTopN` 从编码后的样本构造草图和可选 TopN，同时返回估计 NDV 与样本放大比例。高频值被 TopN 精确保存后不会再写入草图，故 `CMSketch::count` 不包含被抽走的 TopN 计数。
- `CMSketch::{InsertBytesByCount, QueryBytes, SubValue, MergeCMSketch}` 维护 Count-Min Sketch 矩阵。查询并非简单取各行最小值，而是先估计并扣除哈希噪声，再取中位数并受行最小值约束，最后可能回退到 `defaultValue`。
- `TopN` 以按编码字节升序排列的 `Vec<TopNMeta>` 提供精确点查、下界和半开区间 `[lower, upper)` 计数；`MergeTopN` 先合并相同键的计数，再按频数选出指定容量，其余作为 spill 返回。
- `CMSketchToProto`、`CMSketchAndTopNFromProto`、`EncodeCMSketchWithoutTopN`、`DecodeCMSketchAndTopN` 等函数承担内存结构、tipb 消息、持久化字节和系统表 TopN 行之间的转换。

## 主要符号

- `topNThreshold: u64 = 10`：只有候选 TopN 的样本频次总和至少达到 `sampleSize / 10` 时，`buildCMSAndTopN` 才启用 TopN。
- `CMSketch { table, count, defaultValue, depth, width }`：`table` 是 `depth × width` 的 `u32` 计数矩阵；`count` 是草图内累计计数；`defaultValue` 是低频或未充分观测值的兜底估计。字段私有，外部通过方法访问。
- `NewCMSketch`：要求 `depth > 0` 且 `width > 0`，否则断言失败；分配独立的二维 `Vec`。
- `newTopNHelper`、`buildCMSAndTopN`、`calculateDefaultVal`：私有构建流水线。前者按值聚合、按“频次降序/编码升序”稳定化结果，并最多把 `2 * num_top` 个非 singleton 候选纳入 TopN；后两者决定是否启用 TopN、估算未重复值的默认计数并填充草图。
- `NewCMSketchAndTopN`：公开的样本构建入口；空样本或 `row_count == 0` 返回 `(None, None, 0, 0)`，并把小于样本数的 `row_count` 提升到样本数。
- `CMSketch::queryHashValue`：核心估计器。每行用双哈希 `h1 + h2 * row` 选列，估计其他键造成的平均噪声，取校正值的中位数，并调用 `considerDefVal` 判断是否回退。
- `QueryValue`：按 `StatementContext` 时区把 `Datum` 编码为字节，先调用 `TopN::QueryTopN`，未命中再调用 `CMSketch::QueryBytes`。
- `TopN`、`TopNMeta`：前者持有公开字段 `TopN: Vec<TopNMeta>`；后者保存 `Encoded: Vec<u8>` 与 `Count: u64`。公开字段允许调用者直接替换/修改条目，因此排序不变量由调用者与本模块共同维护。
- `MergeTopNAndUpdateCMSketch`：合并源/目标 TopN，把超出 `num_top` 的条目回灌 `CMSketch`，并返回这些 spill 元数据。
- `SortTopnMeta`、`TopnMetaCompare`、`GetMergedTopNFromSortedSlice`：以“计数降序、同计数编码升序”决定保留项；最终保留的 TopN 会再次按编码升序排序以支持二分查找。
- 本文件没有 trait、宏、异步函数或条件编译项；测试通过 `pkg/statistics/lib.rs` 的 `#[cfg(test)] #[path = "cmsketch_test.rs"]` 保持在独立文件中。

## 执行流程

1. 构建时，`NewCMSketchAndTopN` 调用 `newTopNHelper` 汇总样本频次和 singleton 数。候选按频次降序排列；超过请求容量后，只有频次至少达到第 `num_top` 项的约三分之二者才继续进入候选，且频次为 1 时停止。
2. `calculateEstimateNDV`（定义在同 crate 的估算模块）计算总体 NDV 与 `scale_ratio`；`calculateDefaultVal` 用总体行数减去放大后的重复样本数，再除以预计剩余 NDV，分母至少为 1。
3. `buildCMSAndTopN` 检查 10% 阈值。启用时，把候选计数乘 `scale_ratio` 后写入并排序 TopN，再从 helper 中移除这些候选；其余非 singleton 以 `cnt * scale_ratio` 写入草图，singleton 以 `defaultValue` 写入。未启用 TopN 时，全部样本聚合项进入草图。
4. 插入时，`InsertBytesByCount` 用 `murmur3Sum128` 得到双哈希，在每一行定位一个计数器并增加计数，同时增加 `CMSketch::count`。`SubValue` 使用相同哈希路径做反向扣减，供 `pkg/statistics/sample.rs::ExtractTopN` 和直方图 TopN 提取逻辑把精确高频值从草图中移走。
5. 查询时，`QueryValue` 先按语句时区编码值并查 TopN。草图查询对每行计数扣除 `(count - original) / (width - 1)` 的平均碰撞噪声，排序后取中位数，并把结果限制为不超过最小原始计数加临时偏移；`considerDefVal` 对噪声区间内的小估计返回 `defaultValue`。
6. 合并时，`MergeCMSketch` 只接受相同维度，逐单元相加；`MergeTopN` 用哈希表聚合同键，截取频数最高的 `count` 项。`MergeTopNAndUpdateCMSketch` 再把未保留项写回草图，保持“TopN 精确计数与 sketch 计数分离”的约定。
7. 序列化时，矩阵行、默认值和可选 TopN 写入 `tipb::CmSketch`。反序列化以首行长度确定宽度，复制每行已有计数器并保留短行剩余位置为零；`DecodeCMSketchAndTopN` 允许草图字节为空而 TopN 行存在。

## 数据与状态

`CMSketch` 的主要不变量是 `table.len() == depth` 且每行通常具有 `width` 个计数器；正常构造由 `NewCMSketch` 保证。矩阵计数器为 `u32`，总计数和 TopN 计数为 `u64`。`InsertBytesByCount` 与 `MergeCMSketch` 对矩阵单元使用 wrapping 加法，明确允许 `u32` 环绕；总计数使用普通算术。`SubValue` 假设待扣计数不超过总计数及对应单元，否则普通减法会下溢。

`TopN` 的查找与区间统计依赖 `TopN` 向量已按 `Encoded` 字节升序排列。`NewTopN` 只预分配容量，`AppendTopN` 不自动排序；构建、解码和合并路径均显式调用 `Sort`。`BetweenCount` 用两次 `LowerBound` 得到半开区间边界。`MinCount`、`TotalCount` 每次遍历当前向量计算，所以直接修改公开字段后不会读到缓存旧值。

protobuf 不保存 `CMSketch::count`，反序列化时从行计数器重建。实现与 Go 一致地在遍历每一行时重置 `count`，因此最终值是最后一行计数器之和，而不是所有行总和；在合法 CMSketch 中每行累计插入量相同，这一表示成立。短于首行的后续行会把未覆盖位置保留为零，`cmsketch_proto_shorter_later_row_keeps_zero_counters` 固化了该兼容行为。

## 依赖与调用关系

下游依赖包括：`crate::{murmur3Sum128, calculateEstimateNDV, dataCnt, topNHelper}` 提供哈希与样本估算辅助；`codec::EncodeValue` 和 `types::Datum` 负责值编码；`stmtctx::StatementContext` 提供时区与错误策略；`tipb`、`protobuf` 负责交换格式；`chunk::Row` 提供从系统表读取 TopN 的行视图；`astersql_errors` 统一错误类型。对应 crate 依赖由 `pkg/statistics/Cargo.toml` 声明。

RustCodeGraph 对 `NewCMSketchAndTopN` 给出的被调边是 `newTopNHelper`、`buildCMSAndTopN`、`calculateDefaultVal`；对 `QueryValue` 给出的被调边是 `TopN::QueryTopN` 与 `CMSketch::QueryBytes`。图索引未返回这些入口的 Rust 调用方，因此调用方又以源码引用核验：`pkg/statistics/sample.rs` 调用构造和 protobuf 转换；`pkg/statistics/index.rs`、`pkg/statistics/histogram.rs` 调用点查；`pkg/planner/cardinality/row_count_column.rs` 调用 `QueryValue`。RustCodeGraph 的文件节点还报告该文件被 32 个文件使用，但该数字包含测试和符号级间接引用，不能等同于 32 个生产调用点。

## 错误处理与边界

- `NewCMSketch` 对非正深度/宽度直接 panic。虽然独立测试确认 `(1, 1)` 可构造并计算内存，但 `queryHashValue` 的噪声分母是 `width - 1`，所以执行查询实际要求 `width > 1`；不能把“可构造”误写成“可查询”。
- `queryHashValue` 还依赖 `depth > 0` 以安全索引中位数，这由构造断言保证。反序列化要求 protobuf 首行至少有一个计数器，否则会触发宽度断言；它也假定后续行不会长于首行，较长行会越界。较短行被允许并补零。
- `MergeCMSketch` 对维度不一致返回 `SharedError`，不会部分合并。与接受可空指针的 Go 方法不同，Rust 签名使用引用，调用方必须显式处理 `Option`。
- `QueryValue` 的编码错误会交给 `StatementContext::HandleError`；若策略返回错误则向上传播，若策略吞掉错误则继续以空字节查询；没有上下文时直接返回编码错误。
- protobuf 写入和解析错误由 `EncodeCMSketchWithoutTopN`、`DecodeCMSketch` 转为 `astersql_errors::SharedError`。`None`/空输入是受支持的缺省状态：空 sketch 编码为空向量，空字节解码为 `None`，仅有 TopN 行时仍可成功解码 TopN。
- `BetweenCount` 假定上下界顺序满足 `lower <= upper`；反序会产生非法切片范围。对公开 `TopN` 字段的直接变更必须在查询前重新排序。

## 并发与资源生命周期

本文件没有锁、原子变量、通道、后台任务或异步资源。所有修改方法均要求 `&mut self`，共享只读查询要求 `&self`；跨线程同步由所有者负责。`Copy`/`Clone` 会深拷贝二维矩阵和编码字节，protobuf 转换也复制计数器及 TopN 数据，因此返回对象不借用输入缓冲区。

内存规模方面，草图主体是 `O(depth × width)` 个 `u32`，`MemoryUsage` 只计算这部分的 `depth * width * 4`，不含 `Vec` 元数据；TopN 是 `O(n)`，其 `MemoryUsage` 以固定元数据估值加每个编码缓冲区的 capacity。构建 helper 和 `MergeTopN` 还会临时持有按不同值数增长的 `HashMap<Vec<u8>, u64>` 与排序向量。与 Go 的连续 arena 不同，Rust 的每一行是独立 `Vec<u32>`。

Go 版 `TopN` 用 `sync.Once` 缓存最小值和总数；Rust 版没有共享可变缓存，`onceCalculateMinCountAndCount` 只是同一次遍历的兼容命名。因此 Rust 查询在并发只读场景不需要内部同步，但 `MinCount`/`TotalCount` 是每次 `O(n)`。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/statistics/cmsketch.go`，行为测试是 `pkg/statistics/cmsketch_test.go`，Rust 独立测试是 `pkg/statistics/cmsketch_test.rs`。Rust 保留了主要算法与公开命名：TopN 候选的三分之二阈值、10% 启用门槛、NDV/default value 构建、双哈希矩阵、噪声校正查询、spill 回灌、排序规则和 tipb 格式均可逐项对应。

已确认的实现差异如下：

- Go `newTopNHelper` 的确定性同频排序受 `StabilizeV1AnalyzeTopN` failpoint 控制；Rust 始终以编码字节作为同频次次级键，结果固定，但没有该 failpoint 分支。
- Go `QueryBytes` 有 `mockQueryBytesMaxUint64` failpoint，`QueryValue`/`QueryTopN` 还接收 planner context 以供调试追踪；Rust 没有这些 failpoint/trace 参数，只保留 `StatementContext` 的编码时区和错误处理。
- Go `NewCMSketch` 使用单块 arena 优化分配，Rust 使用逐行 `Vec`；Rust 额外断言维度为正。
- Go 的 nil receiver 方法通常返回零值或 no-op；Rust 通过 `Option` 和引用在类型层面表达缺失，没有 nil receiver 语义。
- Go `TopN` 用 `sync.Once` 缓存统计量；Rust 每次重算。Rust `DecodedString` 接收解码闭包，而 Go 版本接收 session context 与列类型并内部调用 `ValueToString`。
- Rust `CMSketchAndTopNFromProto` 统一承载 Go 中该函数与 `DecodeCMSketch` 的重建逻辑；Rust 独立测试额外固定了后续短行补零和最终 `count` 取最后一行和的 Go 兼容语义。

Go 测试还覆盖大规模 Zipf 数据的平均绝对误差和编码字节长度；当前 Rust 独立测试覆盖算法结构与边界，但未等量移植这些大规模误差阈值和固定编码长度断言。本文只记录该覆盖差异，不把未移植测试视作已验证行为。

## 扩展指南

- 修改构建启发式时，应同时检查 `newTopNHelper`、`calculateDefaultVal`、`buildCMSAndTopN` 和同 crate 的 `calculateEstimateNDV`；同步更新 `pkg/statistics/cmsketch_test.rs` 中 TopN 阈值/唯一值用例，并与 Go 的 Zipf 与 unique-data 测试比较精度和兼容性。
- 修改查询公式、哈希或计数宽度时，要保持 `InsertBytesByCount`、`SubValue`、`queryHashValue` 使用完全相同的列定位，并验证 `sample.rs::ExtractTopN` 的扣减语义、`index.rs::QueryBytes` 的回退顺序以及 planner 等值估计。应新增独立 Rust 回归测试，尤其覆盖碰撞、`defaultValue`、宽度边界、上/下溢和合并后精度。
- 修改 TopN 存储或排序时，必须维持“持久形态按 Encoded 升序、选择阶段按 Count 降序/Encoded 升序”的双重约定；同步验证 `FindTopN`、`LowerBound`、`BetweenCount`、`MergeTopN` 与 spill 回灌。
- 修改 protobuf 格式时，应成对审查 `CMSketchToProto`/`CMSketchAndTopNFromProto`、`EncodeCMSketchWithoutTopN`/`DecodeCMSketch` 以及 `sample.rs` 的收集器转换，并保留空输入、仅 TopN、短行和解析失败测试。tipb 是 Git revision 依赖，字段变化还需检查上游 schema 兼容性。
- 测试必须继续放在独立的 `pkg/statistics/cmsketch_test.rs`，不要内嵌到生产源文件。若要补齐 Go 行为，优先移植现有 `pkg/statistics/cmsketch_test.go` 的真实断言，而不是降低规模或简化算法。

## 验证依据

- RustCodeGraph：`status` 确认索引可用（11,467 文件、307,296 节点）；`query CMSketch`、`query TopN` 定位 Rust/Go 对照符号；`node --file pkg/statistics/cmsketch.rs` 读取完整 677 行；`callees NewCMSketchAndTopN` 与 `callees QueryValue` 核对关键被调边。`callers` 对这些符号未返回结果，因此没有将缺失结果当成“无调用者”。
- 源码与装配：完整阅读 `pkg/statistics/cmsketch.rs`；读取 `pkg/statistics/lib.rs` 确认模块装配、公开重导出和独立测试文件；读取 `pkg/statistics/Cargo.toml` 确认 crate 边界及 errors、codec、chunk、protobuf、stmtctx、tipb、types 等依赖。
- 直接调用证据：读取 `pkg/statistics/sample.rs`、`pkg/statistics/index.rs`、`pkg/statistics/histogram.rs`、`pkg/planner/cardinality/row_count_column.rs` 的相关调用上下文，并用 `rg` 检索所有 Rust 引用。
- 语义与测试：完整阅读 `pkg/statistics/cmsketch_test.rs`；读取 `pkg/statistics/cmsketch.go` 和 `pkg/statistics/cmsketch_test.go` 的构建、查询、合并、编解码、TopN 与精度用例。Rust 测试明确覆盖 `(1,1)` 构造、样本构建、TopN 合并 spill、插入/扣减、维度错误、两类 protobuf 往返、短行兼容、阈值、排序与区间统计。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前执行任务规定的 11 章节结构检查，并人工检查所有行为结论均能回指上述符号或文件，未把 Go 独有 failpoint、trace 或大规模精度测试写成 Rust 已支持能力。
