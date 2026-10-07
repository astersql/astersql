# `pkg/executor/aggfuncs/aggfuncs.rs`

## 文件定位

[`aggfuncs.rs`](./aggfuncs.rs) 是 `astersql-executor-aggfuncs` crate 的公共契约与聚合中间状态定义文件。crate 入口 [`lib.rs`](./lib.rs) 以 `pub mod aggfuncs` 声明并用 `pub use aggfuncs::*` 重新导出这里的 API；[`Cargo.toml`](./Cargo.toml) 的 `package.metadata.porting.go-package` 则把该 crate 明确对应到 Go 的 `pkg/executor/aggfuncs`。

它位于 SQL 执行层的聚合边界：上游 hash/stream/window 执行逻辑持有按分组键组织的 partial result，下游具体函数模块负责 COUNT、SUM、AVG、MAX/MIN、FIRST_ROW 等状态的更新与求值。文件本身不解析 SQL，也不选择聚合函数；选择实现的工厂位于 [`builder.rs`](./builder.rs)，具体算法分散在同目录的 `func_*.rs` 中。

当前 Rust 接线需要分两层理解：`AggFunc`/`Serializer` 定义通用契约，但仓库搜索到的显式 `AggFunc` 实现目前主要是 [`func_max_min_count.rs`](./func_max_min_count.rs) 的 `CountExtrema<T>`；已经落地的 typed spill 主链则通过本文件的 `BuiltAggFunc::spill_function`、`StateSerializer<T>` 和 `merge_spilled_partial_result` 工作。因此不能仅凭 trait 定义推断所有 Go 聚合实现都已统一接入该 trait。

## 核心职责

1. 定义聚合生命周期契约：`AggFunc` 覆盖分配、重置、批量更新、合并和最终写出，`Serializer` 覆盖 spill 编解码，两个滑动窗口 trait 描述窗口增量能力。
2. 提供安全的类型擦除状态：`PartialResult = Box<dyn Any + Send>` 让不同聚合状态可放进统一容器，同时保留所有权和跨线程传递约束。
3. 定义常用 partial-result 数据形状及内存计量常量，供具体函数模块共享，避免每种实现重复声明状态协议。
4. 提供通用反序列化循环 `deserialize_partial_result_common`，统一逐行恢复、内存增量累计和行数完整性检查。
5. 为当前 typed spill 路径建立状态协议：`SpillState` 描述复制、写出、读回和内存费用，`StateSerializer<T>` 将协议绑定到输出列序号。
6. 将 `BuiltAggFunc` 的具体实现枚举映射到可 spill 的状态模板，并在恢复同组数据时由 `merge_spilled_partial_result` 按真实类型合并。

## 主要符号

- `DEF_*_SIZE`：用 `size_of` 给出整数、浮点、时间、行、布尔、Decimal、Duration 等固定大小；`DEF_INTERFACE_SIZE` 特意按两个机器字估算 Go interface，以保持移植时的记账口径。
- `PartialResult`：拥有状态的 `Box<dyn Any + Send>`。读取或修改具体状态必须分别使用 `downcast_ref::<T>()`/`downcast_mut::<T>()`，生产者和消费者必须约定同一 `T`。
- `AggPartialResultMapper`：`MemAwareMap<String, Arc<Vec<PartialResult>>>` 的 boxed 别名。键是编码后的分组键；`Arc` 共享整组状态向量，map 自身由 `MemAwareMap` 计量。`new_agg_partial_result_mapper[_with_capacity]` 是两个构造入口。
- `AggError(String)`：聚合层轻量错误包装，提供 `Display` 和 `Error`；目前 typed merge 的不匹配或不支持类型通过它返回。
- `Serializer`：`serialize_partial_result` 把单个状态写进 `Chunk`，`deserialize_partial_result` 从整列恢复一个状态数组并返回内存增量。
- `AggFunc: Serializer`：完整聚合协议。`alloc_partial_result` 返回状态与初始内存费，`update_partial_result` 处理同组行，`merge_partial_result` 合并并行/分阶段状态，`append_final_result_to_chunk` 按 `ordinal` 写最终列。
- `BaseAggFunc`：保存 `args`、输出列 `ordinal`、可空的 `return_type`。其 merge 和序列化方法是会 `panic!("Not implemented")` 的占位默认行为，不是可安全直接调用的通用实现。
- `SlidingWindowAggFunc` / `MaxMinSlidingWindowAggFunc`：前者用旧窗口边界和位移量增量更新状态，后者让 MAX/MIN 实现接收当前窗口起点。
- `deserialize_partial_result_common`：创建 `DeserializeHelper`，反复调用类型专用闭包，累加状态与内存费，并断言恢复数等于 `Chunk::NumRows()`。
- 通用状态：`MaxMinPartialResult<T>`（空标志和值）、`Avg*PartialResult`（sum/count）、`SumPartialResult<T>`（值/非空计数）、`GroupConcatPartialResult`（两个可复用字节游标）、`FirstRowState` 与 `FirstRowPartialResult<T>`。
- `SpillValue`：JSON 聚合 spill 使用的受限值集合；`memory_usage` 只计算 `String`、`BinaryJSON`、`Opaque` 的堆容量，固定大小变体返回 0。
- `JsonArrayPartialResult` / `JsonObjectPartialResult`：分别保存有序元素向量与键值 map。
- `SpillState`：具体 typed 状态的 spill 协议；`has_spill_state` 默认真，`fixed_spill_memory` 默认计入 `size_of::<Self>()`。
- `StateSerializer<T>`：保存列序号和配置模板。模板既用于复制空状态，也保留 `GroupConcat` 分隔符/长度或 `Percentile` 百分比等函数配置，而写盘只保存变化的 partial data。
- `BuiltAggFunc::spill_function`：依据 `AggImplementation` 和 `ValueKind` 构造 `(Serializer, 初始 PartialResult)`；当前支持 distinct count/sum/avg、approx count distinct、方差族、distinct group concat、部分 FIRST_ROW 与 PERCENTILE，其他分支明确返回 `None`。
- `merge_spilled_partial_result`：对恢复状态逐个尝试下转型，再调用对应类型的 `merge`；目标类型不匹配和未列入矩阵的类型分别返回不同 `AggError`。

## 执行流程

常规聚合契约的预期流程由 `AggFunc` 方法顺序表达：执行器先 `alloc_partial_result`，对同一组的一批 `Row` 调 `update_partial_result`，并行或多阶段执行时把 source 合并到 destination，最后向结果 `Chunk` 追加一列值；重复使用状态前调用 reset。这里仅定义流程，实际函数的 NULL、类型转换和算术规则由各 `func_*.rs` 决定。

当前 typed spill 的真实流程如下：

1. [`builder.rs`](./builder.rs) 产生含 `implementation`、`ordinal`、separator/max-len 等配置的 `BuiltAggFunc`。
2. `spill_function` 用实现枚举选择具体 `SpillState`，内部 `bind` 同时构造 `StateSerializer<T>` 和同类型初始状态。未覆盖的类型返回 `None`，调用者必须处理“不支持 spill”。
3. spill 写出时，`StateSerializer::serialize_partial_result` 将 `PartialResult` 下转成 `T`。有状态时调用 [`spill_serialize_helper.rs`](./spill_serialize_helper.rs) 的 `SerializeHelper::serialize_state` 并 `AppendBytes`；无状态（如 null percentile）写 NULL。
4. 恢复时，若模板 `has_spill_state` 为假，则为每一行复制模板且记账为 0；否则进入 `deserialize_partial_result_common`，逐行复制模板、用 [`spill_deserialize_helper.rs`](./spill_deserialize_helper.rs) 的 `deserialize_state` 覆写数据，并计入固定对象大小加恢复出的堆费用。
5. [`aggregate/agg_hash_partial_worker.rs`](../aggregate/agg_hash_partial_worker.rs) 的 `PartialResultSpill::restore_merged_partition` 遍历恢复出的分组 map：新键直接接纳状态，重复键先校验状态宽度，再对每列调用 `merge_spilled_partial_result`。
6. typed merge 宏先识别 source 类型，再要求 destination 为相同类型。distinct 容器和 HLL/方差类按各自 merge 规则合并；FIRST_ROW 保留先到状态；PERCENTILE 克隆 source 后调用 `merge_from`，避免直接消耗借用的 source。

`deserialize_partial_result_common` 的循环终止条件来自闭包返回 `None`。终止后强制校验恢复行数；这保证每个 spill chunk 的每个聚合列都与 chunk 行数一一对应，防止后续按 row index 拼装分组状态时错位。

## 数据与状态

`PartialResult` 拥有而非借用具体状态；`Send` 允许状态随并行执行器移动，但不提供共享可变访问。`AggPartialResultMapper` 中每个键对应一个 `Arc<Vec<PartialResult>>`，共享的是整个不可变向量所有权；若需修改内部状态，调用路径必须拥有或建立自己的可变状态容器，不能从 `Arc` 自动获得可变借用。

状态结构中的标志位有业务含义：MAX/MIN 的 `is_null` 区分“尚无有效输入”；SUM 的 `not_null_row_count` 决定空组是否输出 NULL；FIRST_ROW 同时区分“第一行已取得”和“该值自身为 NULL”。这些字段不能只从默认数值推断状态。

内存返回值均为增量而非总量。通用反序列化累加每行闭包的费用；`StateSerializer` 对真实 spill 状态计入 `size_of::<T>() + heap`。`merge_spilled_partial_result` 中不同类型的增量口径不同：HLL 通过 merge 前后 `memory_usage` 差值计量，distinct variance 用 capacity 增量乘元素大小，GROUP_CONCAT 当前按 distinct 容器长度增量计量，而固定状态通常返回 0。扩展时必须沿用具体状态的容量/所有权口径，不能一律使用元素个数。

`SpillValue::memory_usage` 也只报告变长 payload 的额外堆容量，不包含枚举本体；调用方若计算总费用，需要另行加入固定对象成本。

## 依赖与调用关系

- crate 边界：[`Cargo.toml`](./Cargo.toml) 将 `astersql-expression`、`astersql-expression-exprctx`、`astersql-types`、`astersql-util-hack`、`astersql-util-serialization`、`astersql-util-collate` 作为直接非可选依赖。本文件直接使用其中的表达式/求值上下文、字段与 SQL 值类型、内存感知 map、Chunk/Row 和编解码类型。
- 模块内下游：`StateSerializer` 调用两个 spill helper；`spill_function` 构造 `func_count_distinct`、`func_avg`、`func_sum`、`func_sum_int`、`func_varpop`、`func_group_concat`、`func_first_row`、`func_percentile` 中的状态；typed merge 再调用这些状态的合并方法。
- 直接上游：`agg_hash_partial_worker.rs::restore_merged_partition` 调 `merge_spilled_partial_result`；`func_max_min_count.rs` 调 `deserialize_partial_result_common`；`spill_helper_test.rs` 和 `aggregate/agg_spill_test.rs` 直接调用 `spill_function`/typed merge。`benchmark_test.rs` 使用 mapper 构造器。
- Go 主链证据：[`aggregate/agg_hash_partial_worker.go`](../aggregate/agg_hash_partial_worker.go) 在 partial worker 中构造 mapper；[`aggregate/agg_hash_final_worker.go`](../aggregate/agg_hash_final_worker.go) 按键合并各 `AggFunc` 状态；[`aggregate/agg_spill.go`](../aggregate/agg_spill.go) 的 `restoreFromOneSpillFile` 逐列反序列化、逐行按键合并。Rust typed spill 是这套数据流的局部可执行对应，而同目录部分 Rust aggregate 文件仍保留未完成的 Go 对照注释，不能据此声称整条执行器已完全迁移。

RustCodeGraph 将本文件识别为 108 个符号，并报告被 12 个文件使用；精确 callers/callees 命令未在 30 秒内返回结果，因此上述直接调用边由 `rg` 和相邻源码补证，而非臆测图关系。

## 错误处理与边界

- `AggFunc` 的 update/merge/finalize 和 `merge_spilled_partial_result` 用 `Result<_, AggError>` 传播可恢复错误；`AggError` 当前只携带字符串，没有结构化错误码或 source chain。
- `BaseAggFunc` 的 merge/serialize/deserialize 会直接 panic，调用者不能把它当默认空操作。具体实现若复用该基类，必须覆盖所需能力。
- `StateSerializer::serialize_partial_result` 的错误类型不在 trait 签名中；状态类型不匹配通过 `expect` panic。这是内部不变量：`spill_function` 返回的 serializer 必须始终与其配套状态及后续同型状态一起使用。
- `deserialize_partial_result_common` 在恢复数量与 chunk 行数不一致时 assert panic；它检测截断/过早终止，但不把损坏数据转换为 `AggError`。底层解码器的越界/格式行为仍由 serialization helper 决定。
- typed merge 对 source 可识别但 destination 类型不同返回 `"restored partial result type mismatch"`；source 不在支持矩阵时返回 `"unsupported restored partial result type"`。
- `spill_function` 的 `_ => None`、FIRST_ROW/PERCENTILE 的部分 `ValueKind => None` 是明确功能边界。新增 builder 变体若未同步这里，spill 路径会缺失，即使内存内聚合可用。
- 重复分组在进入 typed merge 前由 `restore_merged_partition` 检查状态向量宽度；宽度不同返回 `"partial result width mismatch"`，避免 zip 静默丢列。
- `MaxMinPartialResult<T>::default()` 将 `is_null` 设为 `false`，其语义是否适合具体 MAX/MIN 初始化必须由具体实现的 alloc/reset 核对，不能单独以该 `Default` 作为空组状态保证。

## 并发与资源生命周期

`PartialResult: Send` 表示拥有的状态可在线程之间移动；本文件没有 `Sync` 约束、锁、任务或 channel，也不承诺同一状态可被多线程同时访问。并行 hash aggregate 应通过 worker 间所有权移交或外部同步组织状态。

mapper 中 `Arc<Vec<PartialResult>>` 保持与 Go map 值中 slice backing storage 相近的共享生命周期；它减少向 map 插入/传递整组结果时的复制，但共享引用持续存在时会延长全部 partial result 的释放时间。`StateSerializer<T>` 自身持有模板副本，serializer 活多久，函数配置模板就活多久。

spill helper 的可复用 buffer 属于传入的 `SerializeHelper`，序列化调用要求 `&mut`，从类型层阻止同一个 helper 的并发写入。恢复结果以新的 `Box` 返回；合并后 source 的释放由拥有它的恢复 map 生命周期控制。PERCENTILE 合并显式克隆 source 再交给可能消耗/重排缓冲的 `merge_from`，保证借用 source 不被原地改变。

磁盘文件的创建、关闭与清理由 `aggregate/agg_hash_partial_worker.rs::PartialResultSpill` 管理，不在本文件中；其 `Drop` 会关闭文件，恢复路径也把文件视为 single-consumer。本文件只管理内存状态与编码列，不应承担磁盘资源生命周期。

## 与 Go 版本的对应关系

[`aggfuncs.go`](./aggfuncs.go) 是最近的权威对照。两端核心对应如下：

- Go `PartialResult unsafe.Pointer` 对应 Rust `Box<dyn Any + Send>`：Rust 用拥有型类型擦除替代裸指针，代价是每次具体操作需要运行时 downcast。
- Go `*hack.MemAwareMap[string, []PartialResult]` 对应 Rust boxed `MemAwareMap<String, Arc<Vec<PartialResult>>>`：Rust 用 `Arc` 明确共享 backing storage 的所有权。
- Go `serializer`、`AggFunc`、`SlidingWindowAggFunc`、`MaxMinSlidingWindowAggFunc` 分别对应同名/同义 Rust trait；Go `AggFuncUpdateContext` 类型别名在 Rust 中直接表现为 `&dyn EvalContext` 参数。
- `baseAggFunc` 的 `args`/`ordinal`/`retTp` 与 `BaseAggFunc` 的字段一一对应，且两端未实现的 merge/serializer 默认方法都会 panic。
- `deserializePartialResultCommon` 与 Rust helper 都逐行恢复、累计内存并强制结果数等于 chunk 行数。
- 大小常量保持同一目的；Rust 的 `DEF_INTERFACE_SIZE` 根据平台计算两个 `usize`，而 Go 代码固定为 16，二者在 64 位平台一致。

Rust 文件 396 行之后的通用 `SpillState`、`StateSerializer`、`BuiltAggFunc::spill_function` 和 typed merge 并不是 `aggfuncs.go` 中逐段同形的声明，而是对 Go 各具体 `SerializePartialResult`/`DeserializePartialResult` 及 `aggregate/agg_spill.go::restoreFromOneSpillFile` 行为的集中式 Rust 接线。它保留“函数配置不落盘、只编码 partial data”“同键恢复后合并”和内存增量返回等意图，但当前支持矩阵是枚举白名单，不能等同于 Go 文件顶部列出的全部实现集合。

测试对应上，[`aggfuncs_test.rs`](./aggfuncs_test.rs) 仅验证 `BaseAggFunc::return_type` 的可空契约和导出的真实 `FieldType`；Go 的 [`aggfunc_test.go`](./aggfunc_test.go) 及各 `func_*_test.go` 覆盖更完整的聚合语义。Rust typed spill 的主要行为证据实际位于 [`spill_helper_test.rs`](./spill_helper_test.rs) 和 [`aggregate/agg_spill_test.rs`](../aggregate/agg_spill_test.rs)。

## 扩展指南

新增普通聚合能力时，先判断修改层次：如果只是新算法，应在独立 `func_<name>.rs` 中实现并在 `builder.rs` 接线；只有需要新的公共状态形状、生命周期方法或统一内存常量时才修改本文件。Rust 单元测试必须继续放在独立 `*_test.rs`，不要嵌入 `aggfuncs.rs`。

新增可 spill 的 `AggImplementation` 时至少同步以下位置：

1. 为状态实现 `SpillState`，明确 `copy_partial` 是否复制函数配置、`write_spill` 编码哪些可变字段、`read_spill` 返回何种堆内存费。
2. 在 `BuiltAggFunc::spill_function` 增加实现/值类型映射；若某些 `ValueKind` 不支持，保留显式 `None` 并在调用层测试该边界。
3. 在 `merge_spilled_partial_result` 加入同一状态类型的合并分支，确保 destination 类型错误仍返回错误，并正确计算容量增量。
4. 在 `spill_helper_test.rs` 扩充 serializer round-trip 矩阵，在 `aggregate/agg_spill_test.rs` 增加重复分组恢复/合并与 no-spill 结果对照；具体 SQL 语义仍在对应 `func_*_test.rs` 测试。
5. 与 Go 的具体 serializer、`restoreFromOneSpillFile` 合并规则和 NULL/空组行为逐项核对，不因 Rust 类型更方便而删减阶段或边界逻辑。

如果新增 `SpillValue` 变体，必须同时更新其 `memory_usage`、`SerializeHelper` 的值分派和 `DeserializeHelper` 的对应解码；遗漏任一处都会形成无法往返或少计内存的协议不一致。修改 `PartialResult` 或 mapper 值所有权则会影响并行 worker 和内存追踪，应先搜索所有构造与传递点。

兼容风险主要是 spill 格式和 Go/Rust 状态语义漂移；正确性风险主要是 downcast 类型矩阵不一致、NULL 状态丢失和 partial merge 非结合；性能风险主要是无意克隆大状态、按长度而非 capacity 记账以及 mapper/状态堆分配增加。

## 验证依据

- 目标源码：[`aggfuncs.rs`](./aggfuncs.rs)，完整读取 1–737 行；声明清单覆盖常量、类型别名、traits、通用状态、typed spill factory 与 merge。
- crate/模块：[`Cargo.toml`](./Cargo.toml)、[`lib.rs`](./lib.rs)、[`builder.rs`](./builder.rs)。
- 直接辅助与调用路径：[`spill_serialize_helper.rs`](./spill_serialize_helper.rs)、[`spill_deserialize_helper.rs`](./spill_deserialize_helper.rs)、[`aggregate/agg_hash_partial_worker.rs`](../aggregate/agg_hash_partial_worker.rs)。
- Rust 测试：[`aggfuncs_test.rs`](./aggfuncs_test.rs)、[`spill_helper_test.rs`](./spill_helper_test.rs)、[`aggregate/agg_spill_test.rs`](../aggregate/agg_spill_test.rs)；观察到 round-trip、NULL percentile、并行 typed distinct spill/no-spill 对照、恢复合并等覆盖。
- Go 对照：[`aggfuncs.go`](./aggfuncs.go)、[`aggregate/agg_hash_partial_worker.go`](../aggregate/agg_hash_partial_worker.go)、[`aggregate/agg_hash_final_worker.go`](../aggregate/agg_hash_final_worker.go)、[`aggregate/agg_spill.go`](../aggregate/agg_spill.go)、[`benchmark_test.go`](../benchmark_test.go)。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/executor/aggfuncs` 找到目标文件 108 个符号；`query` 精确定位 `PartialResult`（第 67 行）、`deserialize_partial_result_common`（第 201 行）、`spill_function`（第 460 行）、`merge_spilled_partial_result`（第 604 行）；`node --file ... --offset ...` 读取全文件并报告其被 12 个文件使用。精确 `callers`/`callees` 查询在 30 秒内无输出，因此调用边由 `rg` 与上述相邻文件补证。
- 本任务为纯文档分析，按任务说明不运行 Cargo。交付前使用任务规定的命令验证本文恰有 11 个固定二级标题，并人工复核没有把 trait 契约、注释占位或 Go 完整实现列表误写成 Rust 已接线事实。
