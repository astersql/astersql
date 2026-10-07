# [`pkg/executor/aggfuncs/func_max_min.rs`](./func_max_min.rs)

## 文件定位

本文件属于 `astersql-executor-aggfuncs` crate；crate 根由 `pkg/executor/aggfuncs/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，`pkg/executor/aggfuncs/lib.rs` 通过 `pub mod func_max_min` 暴露本模块。它提供 MAX/MIN 的类型化状态、比较适配器，以及窗口极值所需的单调双端队列算法。

需要区分“算法模块”和“执行器实现”：本文件没有实现 `pkg/executor/aggfuncs/aggfuncs.rs` 中的 `AggFunc`、`Serializer` 或 `SlidingWindowAggFunc` trait，也不负责表达式求值、结果写入 chunk、spill 编解码或内存增量记账。`pkg/executor/aggfuncs/builder.rs::build_max_min` 和 `build_window_function` 会选择 `AggImplementation::MaxMin` 或 `AggImplementation::SlidingMaxMin`，但当前 Rust 生产代码没有把这些变体实例化成本文件的 `MaxMin<T>`/`SlidingMaxMin<T>`；固定名称检索显示这些泛型状态和 `MaxMin4*` 别名目前仅在本文件及独立测试中出现。因此它是已实现且已测试的核心算法/类型基础，尚不是 Go 版 MAX/MIN 执行链的完整替代。

## 核心职责

1. `MaxMin<T>` 保存普通分组聚合的一个可选极值；`None` 同时表达“尚未见到非 NULL 输入”和 SQL 全 NULL 组的最终 NULL 状态。
2. `MinMaxDeque<T>` 用单调队列保存滑动窗口仍可能成为极值的候选；`SlidingMaxMin<T>` 再为它补充绝对行号换算和窗口平移。
3. `TimeValue`、`DurationValue`、`NamedValue`、`BinaryJson`、`VectorFloat32` 把 SQL 特有比较语义适配为 Rust 排序或显式比较器。
4. `update_float32`、`update_float64`、`update_vector`、`update_collated_string` 为不能直接使用普通 `Ord`、或必须携带排序规则的类型提供更新入口。
5. `MaxMin4*` 与 `MaxMin4*Sliding` 类型别名列出普通聚合和滑动算法预期支持的类型集合；别名本身不增加执行器接线。

## 主要符号

- `TimeValue { packed, kind, fsp }`：`Eq`/`Ord` 只比较 `packed`，保留 `kind` 和 `fsp` 作为结果元数据；同一时刻但元数据不同的值视为相等。
- `DurationValue { nanos, fsp }`：只按 `nanos` 比较，`fsp` 不参与极值选择。
- `NamedValue { name, value }`：ENUM/SET 载体；默认排序只看 `name`，相等名称保留先到值的数值载荷。注意这只是按 Rust `String` 的字节序排序；需要真实 collation 时应使用显式比较器，而不能假设其等价于所有 SQL 排序规则。
- `BinaryJson { type_code, value }`：`as_types_binary_json` 转成 `astersql_types::json_functions::BinaryJSON`，`Ord::cmp` 委托 `CompareBinaryJSON`，从而使用 JSON 语义而不是原始类型码/字节序。
- `VectorFloat32(Vec<f32>)`：`compare` 逐元素调用 `f32::total_cmp`，首次不等即返回；公共前缀相同时较短向量更小。
- `MaxMin<T> { is_max, value }`：普通聚合状态。`new`、`reset`、`value` 管理生命周期；`update_by`/`merge_by` 接收自定义比较器；当 `T: Ord` 时，`update`/`merge` 使用 `Ord::cmp`。
- `Pair<T> { index, item }`：窗口候选值及其绝对行号。
- `MinMaxDeque<T> { is_max, values }`：封装 `VecDeque<Pair<T>>`。`enqueue` 维持单调性，`dequeue` 清理过期下标，`front` 始终是当前极值候选。
- `SlidingMaxMin<T> { deque, window_start }`：`set_window_start`、`update_by`、`slide_by` 把批内 offset 转换为绝对下标；`value` 读取队头。
- `MaxMin4Int` 至 `MaxMin4Set`：普通状态别名覆盖整数、浮点、十进制、字符串、时间、时长、JSON、向量、ENUM 和 SET。滑动别名只覆盖整数、浮点、十进制、字符串、时间和时长，与 `builder.rs::build_window_function` 允许转换为 `SlidingMaxMin` 的八类一致。

## 执行流程

普通聚合从 `MaxMin::new(is_max)` 开始。`update_by` 遍历输入并用 `flatten` 跳过 `None`；首个非空值无条件成为当前值。之后比较新值与当前值：MAX 只在 `Greater` 时替换，MIN 只在 `Less` 时替换，`Equal` 不替换，因此相等键稳定保留先到载荷。`merge_by` 先断言源、目标方向相同，再把源状态的可选值克隆后复用同一更新规则；空源无影响，空目标采纳非空源。

滑动窗口先用 `SlidingMaxMin::set_window_start` 确定当前批首行绝对下标，再由 `update_by` 把每个非 NULL 输入以 `window_start + offset` 入队。`MinMaxDeque::enqueue` 从队尾反复删除被新值支配的候选；相等旧值也会删除，保留下标更新的新值，使它能在窗口中存活更久。MAX 队列从头到尾递减，MIN 队列递增，所以 `front` 是极值。`slide_by(new_start, incoming_start, incoming, compare)` 先删除所有 `index < new_start` 的过期项，再以 `incoming_start + offset` 加入新行，最后更新 `window_start`。每项最多入队、出队各一次，连续滑动的摊还时间为每行 O(1)，状态空间最坏为窗口大小 O(w)。

类型特化比较中，浮点入口把 NaN 排在所有普通数值之前，两个 NaN 相等，这与 Go `cmp.Compare` 的约定一致；向量使用稳定全序；字符串比较由调用方传入 collation-aware 闭包；JSON 通过 `CompareBinaryJSON` 比较。

## 数据与状态

`MaxMin<T>` 的持久状态只有方向位和一个 `Option<T>`。`reset` 只清空值，不改变 MAX/MIN 方向。相等值不替换意味着时间/时长的显示元数据、ENUM/SET 的数值载荷、以及 JSON 的等价二进制表示都保留首次选中的实例。

`MinMaxDeque<T>` 自己拥有 `T`，不会借用输入批；队列只保留尚未被更晚且更优或相等的值支配的候选。`SlidingMaxMin::reset` 清空队列但刻意不重置 `window_start`，调用方在复用状态时必须显式设置正确起点。NULL 不入队；窗口内没有非 NULL 值时 `value()` 返回 `None`。

`BinaryJson::as_types_binary_json` 每次比较都会克隆底层 `Vec<u8>` 后调用外部比较函数，这保证转换拥有数据，但可能放大大 JSON 值的比较成本。`update_collated_string`、泛型状态与队列均持有传入的 `String`/`T` 所有权；文件内没有独立的堆内存增量字段，也没有 Go 侧按字符串、JSON、向量长度计算的 memory delta。

## 依赖与调用关系

- 上游装配：`pkg/executor/aggfuncs/lib.rs` 声明模块；`pkg/executor/aggfuncs/builder.rs::build` 经 `build_max_min` 为普通 MAX/MIN 生成 `AggImplementation::MaxMin { kind, is_max }`，`build_window_function` 对 Int、Uint、Float32、Float64、Decimal、String、Time、Duration 改选 `SlidingMaxMin`。
- 当前接线边界：`AggImplementation` 是描述元数据；`pkg/executor/aggfuncs/aggfuncs.rs::BuiltAggFunc::spill_function` 的匹配中没有 `MaxMin`/`SlidingMaxMin` 分支，仓库固定名称检索也没有找到本文件状态/别名的生产实例化。不能据 builder 的选择就声称 SQL 请求已经调用这些 Rust 算法。
- 下游依赖：十进制别名使用 `crate::func_sum::Decimal`；JSON 比较依赖 `astersql-types` 的 `CompareBinaryJSON`；队列依赖标准库 `VecDeque`。`Cargo.toml` 将 `astersql-types` 声明为非 optional 依赖，没有专门控制本模块的 feature。
- 反向复用：`TimeValue`、`DurationValue`、`VectorFloat32` 被 `aggfuncs.rs` 的 FIRST_ROW/spill 状态选择和 PERCENTILE 路径使用；对应测试也在 `func_first_row_test.rs`、`func_percentile_test.rs`、`window_func_test.rs` 引用这些包装类型。这类复用不等同于 MAX/MIN 状态已接线。
- 独立测试：`pkg/executor/aggfuncs/func_max_min_test.rs` 直接验证通用状态、队列和比较适配；Go 的端到端语义位于同目录 `func_max_min_test.go`。

## 错误处理与边界

本文件的大多数操作无返回错误：空队列的 `front`/`back`/`pop_*` 返回 `None`，过期清理在空队列上自然结束，NULL 输入被忽略。唯一显式失败条件是 `MaxMin::merge_by` 对 MAX 与 MIN 状态混合合并时触发 `assert_eq!` panic；方向一致是调用方必须维持的不变量。

比较闭包必须给出稳定、与 SQL 类型语义相符的全序；若比较器不满足传递性，普通状态和单调队列都无法保证正确。整数绝对下标使用 `u64`，`window_start + offset` 与 `incoming_start + offset` 没有显式溢出处理。`slide_by` 假定 incoming 对应窗口右侧新增、并以正确的绝对下标起点提供；它不验证下标单调、窗口边界关系或重复入队。

浮点不能调用 `MaxMin::update`（`f32`/`f64` 不实现 `Ord`），必须走 `update_float32`/`update_float64` 或等价比较器；字符串若要求 SQL collation，必须走 `update_collated_string`。`VectorFloat32::compare` 的 `total_cmp` 是本文件当前事实，扩展时应再次核对 Go `types.VectorFloat32.Compare` 的特殊值约定。

## 并发与资源生命周期

所有状态通过 `&mut self` 串行更新，文件内没有锁、原子、线程、异步任务、通道、事务或 I/O；它既未声明共享并发语义，也不会自行跨组共享状态。若执行器并行聚合，应为每个部分聚合分配独立 `MaxMin<T>`，最终用 `merge_by`/`merge` 汇总；不能在没有外部同步的情况下共享同一可变实例。

值和队列元素均由状态拥有，`reset` 会释放当前极值或清空队列中的逻辑元素；`VecDeque::clear` 通常保留已分配容量供复用。`BinaryJson` 比较时产生的临时克隆在比较返回后释放。该模块没有资源关闭动作，也没有对 Go 侧 `DefPartialResult*Size`、动态字符串/JSON/向量内存和 deque 容量的记账接口。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/executor/aggfuncs/func_max_min.go`。Rust `MaxMin<T>` 抽取了 Go `partialResult4MaxMin*` 中共同的 `val + isNull` 选择逻辑，`is_max` 对应 `baseMaxMinAggFunc.isMax`；`MinMaxDeque<T>` 对应 Go `MinMaxDeque`，两边入队都删除被支配以及相等的旧尾项。边界参数约定不同：Go `Dequeue(boundary)` 删除 `Idx <= boundary`，Go `Slide` 传入 `new_start - 1`；Rust `dequeue(boundary)` 删除 `index < boundary`，`slide_by` 直接传入 `new_start`，整体效果相同但调用约定不可混用。

Rust 的时间、时长、JSON、浮点与名称比较意图分别对应 Go 的 `Compare`/`CompareBinaryJSON`/`cmp.Compare`/collator 路径。`func_max_min_test.rs` 已覆盖：忽略 NULL、普通 MAX/MIN、队列过期、相等尾项替换、时间和时长忽略元数据、名称忽略数值载荷、JSON 跨有符号/无符号数值等价、NaN 排序。

仍存在明确的迁移差距。Go 的每个类型实现完整提供 `AllocPartialResult`、`ResetPartialResult`、表达式 `Eval*`、`AppendFinalResult2Chunk`、`MergePartialResult`、spill 序列化/反序列化和动态内存增量；滑动类型实现 `SlidingWindowAggFunc::Slide`。Rust 本文件只提供内存中的算法状态和比较包装，未实现这些接口。Go 字符串、ENUM、SET 使用真实 collation；Rust 的 `NamedValue::Ord` 只是名称的默认字符串序，只有 `update_collated_string` 能接受外部规则。Go 测试还覆盖多类型 merge、SQL 窗口查询、内存增量和 deque push/pop/reset，这些并未全部由 Rust 独立测试覆盖。

## 扩展指南

若只新增一种可全序值类型，优先复用 `MaxMin<T>`；实现正确的 `Ord`，或提供显式比较器并同时为 update、merge、滑动 enqueue 使用同一比较语义。若类型包含展示元数据或相等键的附加载荷，应明确相等时“保留先到值”的兼容要求。若增加滑动支持，应新增相应 `MaxMin4*Sliding` 别名，并同步核对 `builder.rs::build_window_function` 的允许类型列表。

若目标是让 SQL 执行真正使用本模块，修改范围不会只在本文件：还需在执行器实例化层把 `AggImplementation::{MaxMin, SlidingMaxMin}` 绑定为实现 `AggFunc`/`Serializer`/`SlidingWindowAggFunc` 的具体类型，接入表达式求值、chunk 输出、spill codec 与内存记账。这些行为必须逐项对齐 Go，不能用当前纯状态类型替代完整接口。

测试应继续放在独立的 `pkg/executor/aggfuncs/func_max_min_test.rs`，不要内嵌到生产源文件。至少增加 NULL/全 NULL、重复相等值、merge 方向不一致、窗口清空与重新起点、NaN/无穷、collation、JSON 大值和下标边界用例；执行器接线还应同步 Go `func_max_min_test.go` 的多类型 merge、内存增量、spill 以及真实 SQL 窗口场景。兼容风险集中在 collation、NaN/JSON/向量排序和相等值稳定性；性能风险集中在 JSON 比较克隆、拥有型字符串复制和最坏 O(w) 队列内存。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/executor/aggfuncs` 确认 Rust/Go 实现与测试均在索引中。
- RustCodeGraph 源码/符号：读取 `func_max_min.rs` 全部 426 行；`query MaxMin`、`query SlidingMaxMin`、`query maxMin4Int` 确认 Rust 泛型、别名和 Go 逐类型实现；`node build_window_function` 及其调用轨迹确认 builder 对普通/滑动变体和支持类型的选择。
- RustCodeGraph 调用边：`build_window_function` 调用 `build_max_min` 并实例化 `AggImplementation::SlidingMaxMin`；`aggfuncs.rs::BuiltAggFunc::spill_function` 的实际分支用于确认 MAX/MIN 尚无 spill 绑定。对本文件辅助函数执行 callers 查询未得到生产调用边，随后以精确固定名称检索复核未发现生产实例化。
- 已读 Rust 路径：`pkg/executor/aggfuncs/func_max_min.rs`、`lib.rs`、`Cargo.toml`、`builder.rs`、`aggfuncs.rs`、`func_max_min_test.rs`；并检查复用证据所在的 `func_first_row_test.rs`、`func_percentile_test.rs`、`window_func_test.rs` 引用位置。
- 已读 Go 路径：`pkg/executor/aggfuncs/func_max_min.go` 的 deque、各类型符号及整数、字符串、JSON、向量、ENUM/SET代表实现；`pkg/executor/aggfuncs/func_max_min_test.go` 的测试清单和关键测试体。
- 人工复核结论：本文件存在是为抽取类型化 MAX/MIN 与 O(1) 摊还滑动极值算法；安全扩展必须保持比较器一致、绝对下标约定、NULL 与相等值稳定性，并在需要 SQL 可用性时补齐文件外的完整执行接口。
