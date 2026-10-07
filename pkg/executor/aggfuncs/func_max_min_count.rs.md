# `pkg/executor/aggfuncs/func_max_min_count.rs`

## 文件定位

本文件属于 `astersql-executor-aggfuncs` crate（见 `pkg/executor/aggfuncs/Cargo.toml`），实现 TiDB 扩展聚合函数 `MAX_COUNT` / `MIN_COUNT` 的 Rust 执行器：它不返回极值本身，而返回当前分组或窗口中“最大值/最小值出现了多少次”。模块由 `pkg/executor/aggfuncs/lib.rs` 公开为 `func_max_min_count`，公共聚合生命周期来自 `aggfuncs.rs` 的 `AggFunc`、`Serializer`、`SlidingWindowAggFunc` 和 `MaxMinSlidingWindowAggFunc`。

在应用执行链上，`pkg/executor/physical_plan_runtime.rs` 的标量聚合状态和 `pkg/executor/typed_hash_agg.rs` 的分组聚合状态都会调用 `build_count_extrema_function`，随后持有 `Box<dyn CountExtremaAgg>` 与其 `PartialResult`。窗口函数元数据也会通过 `builder.rs::build_window_function` 选择滑动版本。因此，本文件位于“聚合描述符/物理计划已经确定”与“逐行更新、并行合并、窗口滑动及 spill”之间。

## 核心职责

- `CountValue` 把整数、浮点、十进制、时间、时长、字符串、JSON、向量、Enum、Set 的求值、比较、复制、内存计量和 spill 编解码统一起来；`count_value!` 为多数类型生成实现，特殊类型有显式实现。
- `CountPartial<T>` 保存普通聚合的三元状态：当前极值 `value`、该极值的 `count`、尚无非 NULL 输入的 `is_null`。
- `CountExtrema<T>` 实现普通聚合的分配、更新、合并、输出及 spill，并根据 `is_max` 复用同一套逻辑支持 MAX_COUNT 与 MIN_COUNT。
- `CountDeque<T>` / `SlidingPartial<T>` 用单调队列维护滑动窗口的极值及其所有行索引，使窗口平移时无需重扫整个窗口。
- `BuiltAggFunc::instantiate_count_extrema` 和 `build_count_extrema_function` 将构建器元数据或真实 SQL `FieldType` 映射为具体泛型执行器。

核心不变量是：NULL 永远不参与比较或计数；普通状态非空时，`value` 必须是已见输入的目标极值，`count` 必须等于该极值按类型比较规则相等的出现次数；滑动状态的队首必须是窗口内目标极值，队首 `indices.len()` 就是结果计数。

## 主要符号

- `pub trait CountValue`：类型适配契约。`eval` 从第一个表达式取出 `Option<T>`，`compare` 定义极值及相等语义，`retained_size` 提供可变长数据的堆内存增量，`write/read` 对接 spill 格式。它要求 `Default + Send + 'static`，以便状态可装入 `PartialResult = Box<dyn Any + Send>`。
- `count_value!`：为 `i64`、`u64`、`f32`、`f64`、`MyDecimal`、`Time`、`Duration`、`String`、`BinaryJSON` 生成 `CountValue`。`VectorFloat32`、`Enum`、`Set` 因复制/求值路径不同而手写实现。
- `float_compare`：显式规定 NaN 排序；NaN 小于任意非 NaN，两个 NaN 相等。这一规则同时决定极值替换和重复计数。
- `CountPartial<T>`：普通部分结果，`Default` 为 `value = T::default()`、`count = 0`、`is_null = true`。`#[repr(C)]` 使布局稳定可计量，但 spill 仍经显式序列化而非直接转储内存。
- `Peer<T>` / `CountDeque<T>`：一个 `Peer` 保存某个候选值及相等值的全部绝对行索引；`enqueue` 从队尾淘汰劣于新值的候选、把相等值索引并入同一 peer，`dequeue` 按窗口边界逐个删除过期索引。
- `SlidingPartial<T>`：持有装箱的单调队列、缓存的 `count` 与 `is_null`；`refresh` 从队首刷新最终状态。
- `pub struct CountExtrema<T>`：真实执行器。`base` 保存表达式与输出列，`is_max` 选择方向，`collator` 决定字符串/Enum/Set 相等及排序，`reject_rows` 限制 final/partial2 的错误输入路径，`sliding` 选择状态类型，`start` 是初始窗口绝对下标。
- `CountExtrema::absorb`：普通更新与 merge 的共同核心。更优值替换目标并重置计数，相等值累加计数，更差值忽略；返回新旧值 `retained_size` 之差。
- `Serializer for CountExtrema<T>`：通过 `SerializeHelper::serialize_count_extrema` 和 `DeserializeHelper::deserialize_count_extrema` 写入/读出普通 `CountPartial<T>`。
- `AggFunc for CountExtrema<T>`：实现普通/滑动状态分配与重置、逐行更新、普通状态合并、向结果 chunk 写入计数。
- `SlidingWindowAggFunc` 与 `MaxMinSlidingWindowAggFunc`：`slide` 增量加入右侧新行、删除左侧过期索引；`set_window_start` 设置首次批量更新所使用的绝对索引基准。
- `pub trait CountExtremaAgg`：组合上述三个执行 trait，并提供便于执行器读取状态的 `result_count`。
- `BuiltAggFunc::instantiate_count_extrema`：把 `ValueKind` 分派为 12 种具体 `CountExtrema<T>`。
- `build_count_extrema_function`：面向真实表达式的桥接入口，依据 `FieldType`、unsigned flag、collation 和窗口能力构造执行器。

## 执行流程

1. 构建阶段：`build_count_extrema_function` 读取第一个参数的字段类型。整数再按 unsigned flag 分为 `i64/u64`；浮点只接受 `TypeFloat/TypeDouble`；BIT 使用字符串/字节路径；Enum、Set 优先按具体 MySQL 类型识别，其余按 `EvalType` 选择十进制、字符串、时间、时长、JSON 或向量。未知或不一致类型返回 `None`。
2. 实现选择：仅当请求滑动、未设置 `reject_rows`，且类型不是 Enum、Set、JSON、VectorFloat32 时，选择 `SlidingMaxMinCount`；其余选择普通 `MaxMinCount`。`instantiate_count_extrema` 再创建带正确 collator 的泛型实例。
3. 分配阶段：普通聚合分配默认 `CountPartial<T>`；滑动聚合分配容量 64 的 `VecDeque<Peer<T>>`，并把固定结构体大小作为初始内存占用返回。
4. 普通逐行更新：`CountValue::eval` 求第一个表达式，NULL 跳过。`absorb` 在首次非 NULL 或遇到更优值时替换 `value` 并把 `count` 设为 1；相等时通过 `wrapping_add` 加一；更差时保持状态不变。
5. 普通并行合并：空源状态直接忽略；否则复制源极值并以源 `count` 调用 `absorb`。因此更优分区替换目标，同一极值跨分区的计数相加，更差分区不影响结果。
6. 输出阶段：普通或滑动状态为空时写 `0`，否则写缓存的 `count` 到 `base.ordinal` 指定的 Int64 列。该函数的 SQL 空输入语义是 0，而不是 SQL NULL。
7. 滑动更新：首次 `update_partial_result` 用 `start + 批内偏移` 入队所有非 NULL 值；`slide` 用 `[last_end, last_end + shift_end)` 的绝对索引加入新行，再删除所有 `<= last_start + shift_start - 1` 的索引，最后刷新队首计数。相等极值的索引独立保存，因此可逐个过期。
8. Spill：普通状态序列化为 bytes 列，恢复时逐项构造 `CountPartial<T>` 并报告固定布局加动态值大小。源码明确假定滑动状态不会 spill；对滑动状态调用该 serializer 会因 downcast 为 `CountPartial<T>` 失败而 panic。

## 数据与状态

普通状态的固定部分为 `CountPartial<T>`，动态内存由 `T::retained_size` 计量：数值、时间和时长返回 0；字符串按字节长度；JSON 按 `Value.len()`；向量按 `SerializedSize()`；Enum/Set 按名称长度。`absorb` 仅在极值被替换时报告新旧动态大小差，相等计数和更差输入返回 0。测试 `count_extrema_real_rows_all_types_merge_reset_and_spill` 与 `count_extrema_unsigned_collation_memory_and_parallel_merge` 验证这些约定。

滑动状态把每个仍可能成为极值的候选放入 `peers`。队列沿“从队首最优到队尾较差”的方向单调；每个 peer 内的 `indices` 按到达顺序递增。更优的新值会永久淘汰队尾较差候选，因为在新值离开窗口前那些候选不可能重新成为答案；相等值合并索引，保留准确 multiplicity。该实现返回的初始内存只包括 `SlidingPartial` 与 `CountDeque` 固定大小，未对 `VecDeque` 容量、peer、索引和动态值增长返回增量，这是当前源码事实和内存记账风险。

`CountExtrema<T>` 本身持有表达式、collator 与配置，聚合状态则由调用者分别拥有在 `PartialResult` 中。`start` 是执行器级可变字段，只通过 `set_window_start(&mut self, ...)` 在窗口初始化前设置。

## 依赖与调用关系

上游入口：

- `pkg/executor/physical_plan_runtime.rs` 对 `AggFuncMaxCount/AggFuncMinCount` 检查 DISTINCT/dedup，调用 `build_count_extrema_function` 创建 `ScalarAggregateState::CountExtrema`。
- `pkg/executor/typed_hash_agg.rs` 在构造期验证类型，在每个新 group 中再次创建 evaluator 和 partial state，后续逐组更新及输出。
- `pkg/executor/aggfuncs/builder.rs::build` / `build_window_function` 生成 `AggImplementation::{MaxMinCount, SlidingMaxMinCount}`；其中 `build_max_min_count` 拒绝 Dedup，并在 Final/Partial2 且参数超过一个时设置 `reject_rows`。

下游依赖：

- `astersql-expression` 提供 `Expression`、`EvalContext`、MySQL 类型与字段元数据。
- `astersql-util-collate` 的 `GetCollator` / `Collator::Compare` 定义字符串、Enum 和 Set 的排序与相等语义。
- `astersql-util-serialization` 提供 `Chunk`、`Row`、SQL 值类型和逐类型序列化函数。
- 同 crate 的 `aggfuncs.rs` 提供聚合 trait、`BaseAggFunc`、`PartialResult`、`AggError` 和公共反序列化循环；`spill_serialize_helper.rs` / `spill_deserialize_helper.rs` 定义 count-extrema 的具体 wire 编码。

RustCodeGraph 将目标文件识别为 59 个符号，并显示 `build_count_extrema_function` 构造 `AggImplementation::MaxMinCount`、`SlidingMaxMinCount` 和 `BuiltAggFunc`。图的 callers 查询未返回上游边，上述两个生产调用点由精确仓库搜索确认。

## 错误处理与边界

- 表达式求值错误统一转换为 `AggError(e.to_string())` 并立即返回；此前已经吸收的行不会回滚，调用方必须把错误视为该次聚合失败。
- `reject_rows` 为 true 时，`update_partial_result` 在读取任何行之前返回“row-based final aggregation ... is unsupported”；但 `merge_partial_result` 仍可合并 partial state，这正是 Final/Partial2 的受支持路径。
- 无参数、未知 `EvalType`、非 Float/Double 的 Real 类型会使构建入口返回 `None`；上游把它转换为“unsupported ... argument type”一类错误。DISTINCT/dedup 在更上层被拒绝，本文件不维护去重集合。
- NULL 被完全跳过；全 NULL 或空输入的输出固定为 0。普通 merge 的 NULL 源也被忽略。
- 整数计数使用 `wrapping_add`，极端超过 `i64` 容量时会回绕而非报错；这与源码行为一致，扩展时不能把它误述为饱和或检查加法。
- 类型擦除状态通过 `downcast_ref/downcast_mut` 恢复。除 serializer 的说明性 `expect` 外，多数位置使用 `unwrap`，因此“执行器配置与 partial state 类型严格匹配”是调用者必须维持的不变量。
- 滑动 `dequeue` 使用包含边界 `<= boundary`；`slide` 仅在新左边界至少为 1 时传入 `new_start - 1`，避免 `u64` 下溢。窗口索引和 `get_row` 必须与调用者的绝对下标约定一致。
- 普通 serializer 不支持 `SlidingPartial<T>`；当前代码依赖“滑动状态不 spill”的执行器约束，而非运行时返回可恢复错误。

## 并发与资源生命周期

单个 `CountExtrema`/`PartialResult` 的更新方法没有内部锁，也不允许并发可变访问；并行 hash aggregation 的正确模式是每个 worker 独占自己的 evaluator/state，再在协调线程调用 `merge_partial_result`。Rust 测试用四个线程分别建立 Partial1 状态并在 join 后由 Final evaluator 合并，验证 MAX_COUNT 得 12、MIN_COUNT 得 8，而不是共享一个状态。

`PartialResult` 的 `Box` 拥有状态；普通 reset 以默认值整体替换，释放旧动态值；滑动 reset 清空 peers 并重置缓存。`CountDeque` 及各 `Peer` 的 `VecDeque` 在状态存活期间复用分配，drop 时自动释放。普通 spill 恢复会为每个有效条目生成新的 boxed state；失败条目由 helper 返回 `None` 并不加入结果。

`CountValue: Send` 允许 partial state 在线程间转移，但 trait 没有 `Sync` 要求，且表达式/collator 的线程安全能力由外部 trait 对象约束决定。`set_window_start` 需要独占 `&mut self`，应在开始处理该窗口状态之前完成，不能在状态更新中途与 `slide` 交错修改。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/executor/aggfuncs/func_max_min_count.go`。Rust 用泛型消除了 Go 中按类型重复的 `maxMinCount4Int/Uint/Float32/...` 与对应 partial-result 结构，但保留了同一语义：`baseMaxMinCountAggFunc.shouldReplace/shouldAccumulate` 对应 `CountExtrema::absorb`；各类型 `UpdatePartialResult/MergePartialResult` 对应 `CountValue::eval/compare` 加统一的 `AggFunc` 实现；各类型 spill 方法汇总到统一 `Serializer`。

Go 的 `minMaxCountDeque`、`partialResult4MaxMinCountSliding`、`refreshCount4MaxMinCountSliding` 分别对应 Rust 的 `CountDeque<T>`、`SlidingPartial<T>`、`SlidingPartial::refresh`。两端都按索引保存相等极值并在滑动时逐个过期。Go 为 int/uint/float/decimal/string/time/duration 提供滑动实现；Rust 的构建条件同样排除 Enum、Set、JSON、VectorFloat32。

Go 的 `unsupportedRowBasedFinalMaxMinCount` 对应 Rust `reject_rows` 分支；二者都拒绝 Final/Partial2 的逐行输入，却允许 partial state merge。Go 测试 `TestMaxMinCountAllMaxMinTypes`、`TestMaxMinCountSpecialTypes`、`TestMaxMinCountDuplicateSemantics`、`TestMergePartialResult4MaxMinCount`、`TestRowBasedFinalMaxMinCountUnsupported`、`TestMemMaxMinCount`、`TestMaxMinCountSQL`、`TestMaxMinCountSlidingWindow` 提供 SQL 与移植语义基线。

已确认的实现表达差异包括：Rust 把多套结构折叠为 `CountValue + CountExtrema<T>`；Rust 计数用 `wrapping_add` 明确回绕；Rust 的动态类型不匹配会 panic；Rust 滑动状态的动态队列内存没有逐次报告。文档不据此推断行为缺陷，但这些是后续对齐时需要重点复核的兼容与资源风险。

## 扩展指南

- 新增值类型时，先为类型实现 `CountValue`，明确 SQL 求值函数、总序（尤其 NaN/NULL）、collation 是否参与、深复制、动态内存以及稳定 spill 编码；然后同步 `ValueKind`、`instantiate_count_extrema` 与 `build_count_extrema_function` 的字段类型映射。
- 若新类型支持滑动窗口，必须证明比较形成适合单调队列的稳定全序，并把它加入构建器的滑动白名单；若不支持，应继续回退普通实现而不是让 `SlidingPartial` 进入未知路径。
- 修改比较规则时须同时复核“替换”和“相等累计”，并覆盖分区 merge 与滑动重复值逐索引过期。字符串、Enum、Set 必须使用字段 collation，不能退化为 Rust 字节序。
- 修改 spill 格式时须同步 `spill_serialize_helper.rs`、`spill_deserialize_helper.rs` 和兼容策略；不可对滑动状态直接复用普通 downcast serializer，除非先实现并接线独立格式。
- 修改内存计量时须保持 `alloc_partial_result` 固定大小与 `absorb` 动态 delta 的口径一致，并单独考虑滑动队列容量、peer、索引和动态值。
- 测试必须放在独立文件 `pkg/executor/aggfuncs/func_max_min_count_test.rs`，并同步核对 Go 对照测试；至少覆盖普通更新、NULL/空输入、重复极值、无符号、NaN/排序规则、merge 替换与累计、final 拒绝行、spill 往返、滑动边界和内存增量。不要把测试内嵌到生产 `.rs`。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点；`files --filter pkg/executor/aggfuncs/func_max_min_count.rs` 确认目标文件已索引；`node --file ... --offset 1/520` 读取完整 654 行；`query CountExtrema` 找到核心 struct、trait、构建入口和测试辅助符号；`node build_count_extrema_function` 确认它构造 `MaxMinCount`、`SlidingMaxMinCount`、`BuiltAggFunc`。callers/callees 对该泛型/trait 路径未返回完整边，因此使用精确文本搜索补证。
- 生产源码：`pkg/executor/aggfuncs/func_max_min_count.rs`；公共接口 `pkg/executor/aggfuncs/aggfuncs.rs`；构建规则 `pkg/executor/aggfuncs/builder.rs`；crate 边界 `pkg/executor/aggfuncs/Cargo.toml` 与 `lib.rs`；生产调用者 `pkg/executor/physical_plan_runtime.rs`、`pkg/executor/typed_hash_agg.rs`；spill helper 两文件。
- Go 对照：`pkg/executor/aggfuncs/func_max_min_count.go`；Go 测试 `pkg/executor/aggfuncs/func_max_min_count_test.go`。
- Rust 独立测试：`pkg/executor/aggfuncs/func_max_min_count_test.rs`。它覆盖 12 种值路径、空状态与 NULL、动态内存、普通 merge/reset/spill、滑动窗口、重复索引逐个过期、Final/Partial2 拒绝行、unsigned、collation、并行 partial merge、较优/较差/空源合并。
- 本任务为纯文档分析，按计划不运行 Cargo；验收仅执行任务指定的 11 章节结构命令，并人工复核所有重要结论均可回溯到上述符号与文件。
