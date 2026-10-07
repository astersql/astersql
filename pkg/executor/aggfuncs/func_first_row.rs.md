# `pkg/executor/aggfuncs/func_first_row.rs`

## 文件定位

本文件位于 `astersql-executor-aggfuncs` crate，crate 入口 `pkg/executor/aggfuncs/lib.rs` 以 `pub mod func_first_row` 暴露它。它定义 FIRST_ROW 部分结果的泛型状态机，而不是完整的 SQL 表达式求值器：文件中没有 `AggFunc` 实现、行表达式求值、输出 `Chunk` 或错误类型。聚合描述符由 `builder.rs::build_first_row` 选择为 `AggImplementation::FirstRow(ValueKind)`；当前能直接确认的生产接线集中在 `aggfuncs.rs::BuiltAggFunc::spill_function` 和 `aggfuncs.rs::merge_spilled_partial_result`，用于 spill 状态分配、恢复及恢复结果合并。

`pkg/executor/aggfuncs/Cargo.toml` 将该 crate 的 Go 对照包声明为 `pkg/executor/aggfuncs`。本文件的具体值类型来自同 crate 的 `func_max_min` 与 `func_sum`，没有条件编译项、模块级常量或独立外部依赖声明。

## 核心职责

- 用 `FirstRow<T>::state: Option<Option<T>>` 同时表达三种状态：`None` 是尚未见到行，`Some(None)` 是已经见到首行且该行是 SQL `NULL`，`Some(Some(value))` 是已经保存首个非空值。
- `update` 只观察输入迭代器的第一个元素；一旦外层 `Option` 为 `Some`，后续更新不再改变状态。因此“首行为 NULL”也会锁定结果，不能跳到后面的非空行。
- `merge` 是左偏且有顺序的：目标已经有首行时保持目标；目标为空时才克隆来源的完整三态状态。这与并行聚合“按既定合并顺序保留先到达部分结果的首行”一致，但操作并不交换律等价。
- `reset` 恢复未见行状态，`into_result` 消费状态并将嵌套 `Option` 原样交给上层。

## 主要符号

- `pub struct FirstRow<T>`：唯一生产类型；派生 `Clone`、`Debug`、`Default`、`PartialEq`。字段 `state` 是 `pub(crate)`，crate 外只能通过方法观察或变更。
- `FirstRow::reset(&mut self)`：清除已有值和 NULL 标志，回到 `None`。
- `FirstRow::got_first_row(&self) -> bool`：只检查外层 `Option`，所以首行 NULL 也返回 `true`。
- `FirstRow::is_null(&self) -> bool`：仅对 `Some(None)` 返回 `true`；未见行返回 `false`，调用者若需区分两者必须同时读取 `got_first_row`。
- `FirstRow::value(&self) -> Option<&T>`：借用非空首值；未见行和首行 NULL 都返回 `None`。
- `FirstRow::update(&mut self, values: impl IntoIterator<Item = Option<T>>)`：状态为空时取迭代器第一项并保存，空迭代器保持未见行；不要求 `T: Clone`。
- `FirstRow::merge(&mut self, source: &Self) where T: Clone`：仅在目标未见行时克隆来源状态。
- `FirstRow::into_result(self) -> Option<Option<T>>`：消费聚合器并返回完整三态结果。
- 公开别名 `FirstRow4Int`、`FirstRow4Float32`、`FirstRow4Float64`、`FirstRow4Decimal`、`FirstRow4String`、`FirstRow4Time`、`FirstRow4Duration`、`FirstRow4Json`、`FirstRow4VectorFloat32`、`FirstRow4Enum`、`FirstRow4Set`：将 Go 的按类型结构统一映射到同一泛型实现。`Enum` 与 `Set` 当前都使用 `NamedValue`，但仍保留两个语义别名。

## 执行流程

1. 构造或 `Default::default` 后，`state` 为 `None`。
2. 上层调用 `update(values)`。若状态已锁定，方法立即返回，甚至不会调用 `values.into_iter()`；否则只调用一次 `next()`。没有元素时仍为 `None`，第一个元素为 `None` 时变为 `Some(None)`，为 `Some(v)` 时变为 `Some(Some(v))`。
3. 分片结果需要合并时，`destination.merge(source)` 先检查目标。目标为空则复制来源；目标已捕获首行（包括 NULL）则忽略来源。空来源合并到空目标仍为空。
4. 普通观察通过 `got_first_row`、`is_null` 和 `value` 完成；最终所有权交接通过 `into_result` 完成。
5. spill 路径中，`aggfuncs.rs::BuiltAggFunc::spill_function` 为支持的 `AggImplementation::FirstRow` 类型创建 `StateSerializer<FirstRow<T>>`。`spill_serialize_helper.rs` 中的 `SpillState for FirstRow<T>` 依次写出 `is_null`、`got_first_row` 和必要的值，读回时重建嵌套 `Option`；`aggfuncs.rs::merge_spilled_partial_result` 再调用本文件的 `merge`。

## 数据与状态

核心不变量是“是否见过行”和“首行是否为空”不可压缩成单层 `Option<T>`：单层 `None` 无法区分空分组与 NULL 首行。嵌套状态的合法映射固定为：

| `state` | `got_first_row()` | `is_null()` | `value()` |
| --- | --- | --- | --- |
| `None` | `false` | `false` | `None` |
| `Some(None)` | `true` | `true` | `None` |
| `Some(Some(v))` | `true` | `false` | `Some(&v)` |

`FirstRow<T>` 自身只拥有一个可选值，没有计数器、锁、引用计数或全局状态。固定部分的内存由泛型布局决定；字符串、JSON、向量等值可能拥有堆内存。当前 spill codec 通过 `SpillElement::heap_bytes` 计算读回值的堆内存增量；本文件的 `update` 和 `merge` 不返回内存增量。

## 依赖与调用关系

- 上游选择：`builder.rs::build` 在 `FunctionName::FirstRow` 分支调用 `build_first_row`；后者拒绝 `AggMode::Dedup`，并按返回类型生成 `AggImplementation::FirstRow(ValueKind)`。
- spill 分配：`aggfuncs.rs::BuiltAggFunc::spill_function` 当前为 `VectorFloat32`、`String`、`Int`、`Float64`、`Decimal`、`Time`、`Duration` 绑定 `FirstRow<T>`。该分支没有为 `Float32`、`Json`、`Enum`、`Set` 建立绑定，因此“存在公开类型别名”不等于这些类型已在这条 Rust spill 工厂路径接通。
- spill 编解码：`spill_serialize_helper.rs` 的 `SpillState for FirstRow<T>` 调用 `is_null`、`got_first_row`、`value` 并直接重建 crate 可见的 `state`。向量类型特别处理“未见行/NULL 时不编码默认值”，以区分空向量值。
- spill 合并：`aggfuncs.rs::merge_spilled_partial_result` 对七种已登记 `FirstRow<T>` 类型向下转型后调用 `merge`，并返回零内存增量。
- 下游类型：`Decimal` 来自 `func_sum`；`BinaryJson`、`DurationValue`、`NamedValue`、`TimeValue`、`VectorFloat32` 来自 `func_max_min`。
- 独立测试直接调用所有方法；`spill_helper_test.rs` 和 `aggregate/agg_spill_test.rs` 还覆盖 spill 往返及执行器级 spill 场景。仓库搜索没有发现本文件直接实现 `AggFunc`，普通标量执行另有 `physical_plan_runtime.rs::ScalarAggregateState::FirstRow`，不应与本泛型状态误认为同一对象。

## 错误处理与边界

本文件所有方法均为无错误返回；它不求值表达式、不解析 SQL 值，也不验证合并顺序。类型安全由泛型和 `merge(&Self)` 保证，spill 路径的类型向下转型错误由相邻的 `merge_spilled_partial_result` 转换为 `AggError`，不在本文件处理。

边界行为包括：空输入不会锁定状态；NULL 首行会锁定；后续批次不会覆盖已有状态；目标为空时可以采纳 NULL 或非空来源；目标已有 NULL 时不会被非空来源覆盖；`value()` 无法单独区分未见行和 NULL，必须结合状态查询。由于 `merge` 左偏，调换 source/destination 可能改变最终值；调用者必须维持能代表 SQL 首行语义的确定合并顺序。泛型 `T` 只有在合并时要求 `Clone`，大对象的首次合并可能发生深拷贝。

## 并发与资源生命周期

`FirstRow<T>` 没有内部同步，所有变更都要求 `&mut self`；文件本身不声明 `Send`/`Sync`，线程可传递性随 `T` 自动推导。预期生命周期是每个分组或分片拥有独立状态，先更新，再按上层确定的顺序合并，最后读取或消费。`reset` 会释放当前持有的值（其容量不会由本类型保留），`into_result` 将值所有权移出；`merge` 克隆来源但不消费来源。

spill 时 `StateSerializer` 克隆模板/状态并把编码字节交给 chunk；反序列化创建新状态，堆内存记账由相邻 codec 返回。这里没有异步任务、通道、锁、事务或 I/O；磁盘生命周期由聚合执行器和序列化层负责。

## 与 Go 版本的对应关系

Go 文件 `pkg/executor/aggfuncs/func_first_row.go` 使用 `basePartialResult4FirstRow { isNull, gotFirstRow }` 加每种具体类型的 `val`；Rust 以 `Option<Option<T>>` 等价表达两个标志和一个值。Go 的 `UpdatePartialResult` 在未锁定且批次非空时只求值 `rowsInGroup[0]`，Rust `update` 同样只取迭代器第一项。Go 的 `MergePartialResult` 仅在目标 `gotFirstRow == false` 时复制来源，Rust `merge` 保持相同左偏规则。Go 输出时把“未见行”和“首行 NULL”都追加为 SQL NULL；Rust 本文件只通过 `into_result` 暴露状态，实际 chunk 输出不在此处。

Go 为 int、float32、float64、decimal、string、time、duration、JSON、vector、enum、set 分别实现分配、求值、合并、输出及 spill，并对可变长类型报告内存变化。Rust 本文件用别名覆盖相同类型集合，但仅提供通用状态操作；表达式求值、最终输出和完整内存记账没有在本文件复刻。特别是当前 `spill_function` 的类型分支少于别名集合，因此文档只把完整集合称为“类型模型/公开别名对齐”，不声称所有 Go 执行路径均由本文件接通。

## 扩展指南

- 新增值类型时，在本文件增加语义清晰的别名，并确保值类型能表示 SQL 值；若要支持 spill，还需在 `SpillElement`、`BuiltAggFunc::spill_function` 和 `merge_spilled_partial_result` 中成套登记，不能只添加别名。
- 改动状态表示时，必须保持未见行、NULL 首行、非空首行三态，并同步修改 `SpillState::write_spill/read_spill` 的标志协议；已有落盘格式和 Go 的 `isNull/gotFirstRow` 顺序是兼容性风险点。
- 改动 `merge` 时要明确 FIRST_ROW 的顺序语义。把它改成任意非空优先或可交换合并会改变 NULL 首行及并行聚合结果。
- 测试必须放在独立文件。状态机行为同步更新 `pkg/executor/aggfuncs/func_first_row_test.rs`；spill 三态与向量空值同步更新 `spill_helper_test.rs`；执行器级恢复行为可更新 `pkg/executor/aggregate/agg_spill_test.rs`。Go 对齐判断参考 `func_first_row_test.go` 的类型矩阵、合并期望和内存增量测试。
- 当前最明显的扩展审查点是 spill 工厂未绑定 `Float32`、`Json`、`Enum`、`Set`。若未来补齐，应同时验证 codec、恢复合并、内存记账和类型构建，而不是假设已有别名即可工作。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`node --file pkg/executor/aggfuncs/func_first_row.rs` 核对了完整 102 行、全部方法和 11 个别名；`query/node build_first_row` 确认调用边 `builder.rs::build -> build_first_row -> value_kind`；`query/node FirstRowState` 用于区分旧式公共 partial 结构与本文件泛型状态。
- RustCodeGraph 读取：`pkg/executor/aggfuncs/lib.rs`、`builder.rs`、`aggfuncs.rs`、`spill_serialize_helper.rs`、`spill_deserialize_helper.rs`、`func_first_row_test.rs`、`spill_helper_test.rs`、`physical_plan_runtime.rs`。关键生产边为 `BuiltAggFunc::spill_function -> StateSerializer<FirstRow<T>>`、`SpillState for FirstRow<T>` 以及 `merge_spilled_partial_result -> FirstRow::merge`。
- 直接读取的未索引/对照材料：`pkg/executor/aggfuncs/Cargo.toml`、`pkg/executor/aggfuncs/func_first_row.go`、`pkg/executor/aggfuncs/func_first_row_test.go`；另以 `rg` 复核 `FirstRow` 的 Rust 引用集合和当前工厂绑定类型。
- Rust 独立测试证据：`func_first_row_test.rs` 覆盖无行与 NULL 首行区分、NULL 锁定、reset 后采纳来源、只保留首值、空批次及全部公开别名；`spill_helper_test.rs::spill_first_vector_preserves_unseen_null_empty_and_nonempty_flags` 覆盖向量的未见、NULL、空向量、非空向量四态往返。
- 本任务是纯文档分析，按计划未运行 Cargo；交付校验仅执行任务指定的 11 章节结构命令，并人工复核本文没有把未接线类型描述为已支持。
