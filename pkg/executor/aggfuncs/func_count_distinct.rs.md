# `pkg/executor/aggfuncs/func_count_distinct.rs`

## 文件定位

本文件属于 `astersql-executor-aggfuncs` crate；crate 入口 `pkg/executor/aggfuncs/lib.rs` 以公开模块 `func_count_distinct` 暴露它，`pkg/executor/aggfuncs/Cargo.toml` 则把该 crate 对应到 Go 包 `pkg/executor/aggfuncs`。它保存 SQL 精确 `COUNT(DISTINCT ...)` 与 `APPROX_COUNT_DISTINCT` 的可合并部分状态，不负责解析 SQL、选择聚合模式或驱动分组执行。

上游 `builder.rs::build`/`build_approx_count_distinct` 根据函数名、参数类型和 `Complete`、`Partial1`、`Partial2`、`Final` 模式选择实现枚举；`aggfuncs.rs::spill_function` 再把这些实现枚举绑定到本文件的具体状态。因而本文件位于“表达式已求值之后、聚合结果输出之前”，是串行聚合、并行 partial merge 和 spill 恢复共同使用的状态层。

## 核心职责

- `CountDistinct<T>` 用 `HashSet<T>` 实现单列精确去重，忽略 `None`，提供清空、计数、批量更新和跨分区合并，并返回集合扩容的近似内存增量。
- `update_distinct_real` 把浮点数转成可哈希的位模式，同时专门对齐 Go map 的 `+0.0/-0.0` 与 NaN 键语义；`update_distinct_string` 在入集前应用调用方提供的校对键。
- `DistinctValue`、`encode_distinct_value` 和 `CountDistinctMulti` 将多列值编码成一条字节键；任一参数为 NULL 时跳过整行，并在合并时跨分区去重。
- `ApproxCountDistinct` 维护有上限的开放寻址 `u32` 哈希表。表过密时先扩容，到达上限后提高 `skip_degree` 进行确定性抽样，从样本估计原始基数；状态可序列化、反序列化合并或直接内存合并。
- 阶段别名（例如 `CountOriginalWithDistinct4Int`、`ApproxCountDistinctPartial2`）保留 Go 版本与聚合构建器使用的阶段命名，但目前都复用同一种 Rust 状态结构。

## 主要符号

- `CountDistinct<T> { values: HashSet<T> }`：单列精确状态。`Default` 创建空集合；`reset` 保留容量并清空元素；`count` 返回 `i64` 基数；`update` 通过 `flatten` 忽略 NULL；`merge` 克隆源集合元素后复用 `update`。
- `CountDistinctInt`、`CountDistinctReal`、`CountDistinctDecimal`、`CountDistinctDuration`、`CountDistinctString`：构建器和 spill 层使用的类型别名。Real 实际存 `u64` 位模式，Decimal/String 实际存已编码的 `Vec<u8>`，Duration 存 `i64` 表示。
- `update_distinct_real`：将零统一成正零位模式；遇到 NaN 且位模式已存在时递增 payload，保证每次观察到的 NaN 都占一个独立键。
- `update_distinct_string`：以 `Fn(&str) -> Vec<u8>` 注入 collation 规范化；本文件不自行选择字符集或排序规则。
- `DistinctValue`：多参数行的类型化中间值，覆盖 Int、Real、Decimal、Time、Duration、Json、VectorFloat32、String。
- `encode_distinct_value`：把一个非 NULL 值追加到目标缓冲区。Time 使用固定字段布局；Json/String 和向量带长度前缀；Decimal 写 coefficient 与 scale；函数签名返回 `Result<(), AggError>`，便于编码规则未来产生错误时沿聚合链传播。
- `CountDistinctMulti { encoded_rows: HashSet<Vec<u8>> }`：多列状态；`update` 跳过含 NULL 的整行并统计新键拥有的字节，`merge` 克隆源键并只计算首次插入的字节。
- `ApproxCountDistinct { size, size_degree, skip_degree, has_zero, buffer }`：近似状态。`size` 包含零哈希；零不能作为空桶哨兵，故由 `has_zero` 单独记录。
- `ApproxCountDistinct::{insert_hash64, insert_bytes, estimate, serialize, read_and_merge, merge}`：近似状态的公开数据面。`insert_bytes` 使用本文件的 FNV-1a 风格 `hash64`；`estimate` 在未抽样时精确返回样本数，抽样后按哈希空间作放大及对数修正。
- `allocate`、`place`、`good`、`insert_impl`、`shrink_if_needed`、`resize`、`rehash`、`reinsert`：开放寻址与抽样不变量的内部实现。
- `encode_uvarint`/`decode_uvarint`：近似状态序列化中的无符号变长整数编解码；`int_hash64` 用于估计值的确定性低位扰动。

## 执行流程

精确单列路径如下：构建器选择 `CountOriginalDistinct(kind)` 或 `CountPartialDistinct(kind)`；`aggfuncs.rs::spill_function` 按 `ValueKind` 创建对应 `CountDistinct` 状态；执行器把已求值参数交给类型路径更新，NULL 被过滤；最终值由 `count` 读取。并行或 spill 恢复后，`aggfuncs.rs::merge_partial_result` 按实际状态类型调用 `merge`；浮点状态例外地把保存的位模式还原为 `f64` 并再次经过 `update_distinct_real`，维持 Go 的特殊浮点键规则。

多列路径在 `CountDistinctMulti::update` 中先检查整行：只要存在 `None` 就跳过。其余分量按原顺序依次由 `encode_distinct_value` 追加到同一缓冲区，形成复合键并插入 `encoded_rows`。跨分区 `merge` 对编码后的整键做集合并集，因此重复行只计一次。

近似路径由 `builder.rs::build_approx_count_distinct` 按返回类型和聚合模式映射到四个阶段别名。原始值经调用方编码/哈希后进入 `insert_hash64` 或 `insert_bytes`：`good` 先按当前 `skip_degree` 过滤，`insert_impl` 线性探测并去重，`shrink_if_needed` 在半满时扩容；元素超过 `2^16` 上限后增加 `skip_degree` 并 `rehash`，丢弃不再满足低位条件的样本。最终阶段调用 `estimate`。

spill/分布式合并时，`serialize` 写入 `skip_degree`、uvarint 元素数及每个四字节哈希。`read_and_merge` 先对齐较高的 skip，校验数量与剩余长度，必要时扩容，再逐个插入；内存态 `merge` 同样先对齐 skip，再合并零哈希与非零桶。

## 数据与状态

精确状态的逻辑不变量是“集合中每个键代表一个 SQL 非 NULL 等价类”。其内存增量只是可供聚合内存记账的近似值：`CountDistinct<T>` 计算 capacity 增量乘元素静态大小；多列状态另加新插入键的 payload 长度。它没有计算 `HashSet` 桶自身全部实现开销，也不会因 `reset` 或重复插入报告负增量。

复合键是类型编码的串联。可变长的 Json/String/Vector 带长度前缀，避免相邻分量边界歧义；Time 使用大端固定布局，而若干数值分支使用 native-endian。该状态会由本 crate 的 spill 层持久化，但不是跨平台网络协议；修改编码布局必须同步考虑已有 spill 数据在同一执行生命周期内的兼容性。

近似状态的 `buffer` 长度恒为 `2^size_degree`，零值表示空桶，非零哈希采用线性探测。`size` 与非零桶数加 `has_zero` 相符；`good(hash)` 要求被保留哈希的低 `skip_degree` 位全零。提高 skip 后必须同时删除不合格值并重建探测链，这正是 `rehash` 的职责。`UNIQUES_HASH_MAX_SIZE` 限制样本数为 `2^16`，避免基数增长导致内存无界。

## 依赖与调用关系

直接 Rust 依赖很小：`std::collections::HashSet`、`std::hash::Hash`、`std::mem::size_of`，以及本 crate 的 `aggfuncs::AggError` 和 `func_sum::Decimal`。`Cargo.toml` 没有为本文件声明专属 feature；它随 `lib.rs` 的公开模块无条件编译，所需 Decimal 与错误类型来自同 crate 模块。

RustCodeGraph 将 `func_count_distinct.rs::CountDistinct`、`CountDistinctMulti`、`ApproxCountDistinct` 定位为本文件结构体，并显示该文件被 `aggfuncs.rs`、`spill_serialize_helper.rs`、`spill_helper_test.rs`、`aggregate/agg_spill_test.rs` 等引用。源码核验得到的关键边为：`builder.rs::build` -> distinct 实现枚举；`aggfuncs.rs::spill_function` -> 各状态 `default`；`aggfuncs.rs::merge_partial_result` -> `CountDistinct::merge`/`update_distinct_real`/`CountDistinctMulti::merge`/`ApproxCountDistinct::merge`；`spill_serialize_helper.rs` 的三个 `SpillState` 实现 -> 状态序列化与恢复。

本文件没有线程、存储或网络依赖，也不直接调用表达式求值器。字符校对、SQL 类型到 `DistinctValue` 的转换、哈希输入编码、最终 chunk 写入和 spill 介质均由上层或相邻模块负责。

## 错误处理与边界

精确单列更新和合并没有可恢复错误；空输入和全 NULL 输入保持空集合，重复值不增加计数。浮点零被合并为同一键，而同一 NaN 被重复观察时仍产生多个键，这是 Go `map[float64]` 的比较语义，不是 IEEE 位相等语义。字符串等价性完全由传入的 `collator_key` 决定。

`CountDistinctMulti::update` 在任一列为 NULL 时忽略整行。当前 `encode_distinct_value` 的所有分支都成功，但保留 `AggError` 返回通道；调用方必须继续使用 `?`，不能假设未来编码永不失败。编码使用 native-endian 的分支不应被解释为稳定的跨架构格式。

`ApproxCountDistinct::read_and_merge` 明确拒绝空输入、超过最大样本数的声明、声明数量与 payload 长度不一致，以及 `decode_uvarint` 检出的非法/溢出 varint。四字节分块在长度校验后才 `try_into().unwrap()`，因此该 unwrap 由前置不变量保护。估计是概率值，哈希截断为 32 位并存在碰撞，不能把它当成精确计数。

## 并发与资源生命周期

所有状态都要求 `&mut self` 更新；文件内没有锁、原子、线程、异步任务、通道或全局可变状态。并行聚合通过“每个 worker/分区拥有独立状态，随后 merge”实现，而不是多个线程共享一个状态。`Clone` 只是深拷贝容器，线程安全性由所含类型的自动 trait 与上层所有权保证。

`reset` 清空逻辑值但通常保留精确集合容量；`ApproxCountDistinct::reset` 则重新分配初始 16 桶缓冲区并清零全部元数据。spill 生命周期由 `SpillState` 接线管理：状态序列化为字节，读回新状态，再通过聚合层合并。大 `Vec<u8>` 键由集合拥有，测试也验证恢复后不借用源缓冲区。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/executor/aggfuncs/func_count_distinct.go`。Go 为 Int、Real、Decimal、Duration、String 和 MultiArgs 分别定义 partial-result 结构、更新/合并/输出/序列化方法；Rust 用泛型 `CountDistinct<T>`、类型别名和 `CountDistinctMulti` 压缩重复结构，但保留 NULL 过滤、跨分区集合并集、校对键、多参数整行 NULL 排除及内存增量意图。

Go 的 Real 路径使用 `map[float64]struct{}`。Rust 不能直接用 `f64` 作为 `HashSet` 键，因此 `update_distinct_real` 将其映射为 `u64`：正负零归一化，而 NaN 通过未占用 payload 保持 Go 中 `NaN != NaN` 的逐次计数行为。`func_count_distinct_test.rs` 专门固定这两个兼容边界。

Go 的 `partialResult4ApproxCountDistinct` 同样包含 `size`、`sizeDegree`、`skipDegree`、零哈希标记和开放寻址缓冲区，并提供 `InsertHash64`、扩缩容、rehash、merge、Serialize、readAndMerge。Rust 对应实现保留最大样本量、skip 抽样、序列化字段次序和阶段别名；不过上层 Go 类型求值、chunk 输出与 Rust 状态适配由各自框架承担，不能仅凭本文件断言两个完整执行器已在所有输入编码上逐字节等价。

## 扩展指南

新增精确单列类型时，优先复用 `CountDistinct<T>`；同时必须在 `builder.rs` 的类型选择、`aggfuncs.rs::spill_function` 与 `merge_partial_result`、`spill_serialize_helper.rs`/反序列化层接入对应状态，并在独立测试文件增加 NULL、重复值、分区交叠、reset、内存增量和 spill 往返用例。不要把测试内嵌到本生产文件。

新增 `DistinctValue` 变体时，必须设计无歧义且与 Go `codec.HashGroupKey`/对应写入函数兼容的编码；特别检查相邻可变长字段、数值规范化、时区/FSP、collation、JSON 与向量表示。修改已有编码或端序前，应先确定 spill 格式是否需要版本化。

修改浮点键规则时须同时保护 `+0/-0`、多个 NaN、不同 NaN payload、普通重复值及 spill 后合并；最直接的测试位置是 `func_count_distinct_test.rs` 和 `func_distinct_agg_test.rs`。修改近似算法或序列化时，应同步 Go 对照、四种聚合模式、畸形输入错误、最大容量/rehash、不同行为阶段合并，以及 `spill_helper_test.rs::spill_approximate_count_and_variance_preserve_full_partial_state`。

性能风险主要来自克隆大字节键、HashSet 扩容、线性探测聚簇和 rehash；正确性风险主要来自复合键碰撞、校对规则遗漏、特殊浮点值及不同 skip 状态合并。优化时应保留返回内存增量的含义，并用串行结果与分区 merge 结果等价性作为基本不变量。

## 验证依据

- 源码与 crate 边界：`pkg/executor/aggfuncs/func_count_distinct.rs`、`lib.rs`、`Cargo.toml`。
- 上游与状态接线：`pkg/executor/aggfuncs/builder.rs::build_approx_count_distinct`，`aggfuncs.rs::spill_function`、`merge_partial_result`，`spill_serialize_helper.rs` 的 `SpillState` 实现。
- Rust 独立测试：`func_count_distinct_test.rs` 验证有符号零与 NaN；`func_distinct_agg_test.rs::test_parallel_distinct_count` 验证空/NULL/重复/类型/校对/多列与分区合并；`go_scenario_coverage_test.rs::test_parallel_distinct_count` 和 `test_decimal_distinct_sum_and_multi_distinct_nulls` 验证合并与整行 NULL；`spill_helper_test.rs::spill_distinct_count_preserves_int_real_decimal_duration_string_and_multi_keys`、`spill_approximate_count_and_variance_preserve_full_partial_state` 验证 spill 往返。
- Go 对照：`pkg/executor/aggfuncs/func_count_distinct.go` 中各 `baseCountDistinct4*`、`baseCountDistinct4MultiArgs`、`partialResult4ApproxCountDistinct` 及其 update/merge/serialize/readAndMerge 方法；相关 Go 测试为 `pkg/executor/aggfuncs/func_count_test.go`。
- RustCodeGraph：`status` 显示本仓库索引包含 11,467 个文件；`explore "pkg/executor/aggfuncs/func_count_distinct.rs CountDistinct count distinct aggregate"` 定位 `update_distinct_real`、`encode_distinct_value` 与 spill/aggregate 引用；`query CountDistinct --kind struct --json`、`query ApproxCountDistinct --kind struct --json` 将 Rust 结构分别解析到本文件第 30、213、278 行，并同时暴露同路径 Go 对照符号，故后续以文件限定结果消除了同名噪声。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的命令确认目标文档存在且恰有 11 个固定二级章节，并人工复核所有关键结论均可回指上述源、接线、测试或 Go 对照。
