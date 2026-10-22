# `pkg/executor/aggfuncs/row_number.rs`

## 文件定位

`next_value` 负责真正推进并产出序号；相同排序值不会并列，也不会跳号。

## 核心职责

- `RowNumber` 只保存当前分区已经产出的序号，保持“首行返回 1、后续逐行加一”的行为。
- `reset` 在开始新分区或复用部分结果时清零状态。
- `update` 和 `slide` 保留 Go 聚合/滑动窗口协议的无操作语义：行号不依赖输入值或帧增删。
- `DEF_PARTIAL_RESULT_ROW_NUMBER_SIZE` 提供固定状态体积，语义对应 Go 的 `DefPartialResult4RowNumberSize`，供内存核算契约使用。
- `next_value` 负责真正推进并产出序号；相同排序值不会并列，也不会跳号。

## 主要符号

- `pub const DEF_PARTIAL_RESULT_ROW_NUMBER_SIZE: i64`：通过 `size_of::<RowNumber>()` 计算部分结果的固定字节数。它描述结构本身，不包含堆分配；当前结构只有一个 `i64`。
- `pub struct RowNumber { cur_idx: i64 }`：公开类型、私有字段。`Default` 得到 `cur_idx == 0`；`Clone`、`Copy`、`Debug`、`Eq` 和 `PartialEq` 便于值语义使用与测试，但外部代码不能直接破坏计数器不变量。
- `pub fn RowNumber::reset(&mut self)`：把 `cur_idx` 置零；下一次 `next_value` 返回 1。
- `pub const fn RowNumber::update(&self) -> i64`：固定返回 0，表示本次更新没有额外内存增量，也不读取或改变状态。
- `pub fn RowNumber::next_value(&mut self) -> i64`：使用 `wrapping_add(1)` 更新计数器后返回新值。
- `pub const fn RowNumber::slide(&self)`：无操作的滑动钩子，不改变计数器。
- `pub(crate) const fn RowNumber::from_index_for_test(i64) -> Self`：仅在 `cfg(test)` 下存在，用于构造整数上界状态并验证回绕行为；它不是生产 API。

本文件没有 trait 定义或实现、泛型、异步函数、条件 feature，也不负责把结果写入 chunk。唯一条件编译项是测试构造器 `from_index_for_test`。

## 执行流程

1. 调用方以 `RowNumber::default()` 创建部分状态，此时 `cur_idx` 为 0；其固定内存体积由 `DEF_PARTIAL_RESULT_ROW_NUMBER_SIZE` 表示。
2. 输入行进入窗口协议时可以调用 `update`。该函数恒为 0，既不检查行值，也不推进序号，这与 Go `UpdatePartialResult` 返回 `(0, nil)` 对齐。
3. 每需要产出一行结果时调用 `next_value`。函数先对 `cur_idx` 做带回绕的加一，再返回，因此正常范围内依次得到 `1, 2, 3, ...`。
4. 帧滑动时调用 `slide` 不会改变状态，因为 `ROW_NUMBER()` 按分区中的输出位置编号，而不是根据当前 frame 的进入/离开行重新聚合。
5. 分区切换或状态复用时调用 `reset`，恢复到 0；下一次产出重新从 1 开始。

完整 SQL 路径不应由以上局部流程推断。当前 `pkg/executor/windows/window.rs::RowNumber::result` 在实际窗口执行接口中独立完成相同的“先递增、再返回”行为，并通过 `ignores_frame() == true` 表示忽略 frame；目标文件本身没有实现该 trait。

## 数据与状态

状态只有私有的 `cur_idx: i64`，其核心不变量是：在未溢出的普通执行范围内，`cur_idx` 等于本分区已经调用 `next_value` 的次数；`update` 与 `slide` 不影响它；`reset` 将其恢复为零。因为没有保存输入行、排序键、分区键或 frame 边界，本类型无法自行判断何时切换分区，也无法决定调用顺序，这些职责属于上层窗口执行器。

`RowNumber` 没有堆拥有数据，固定体积等于 `size_of::<RowNumber>()`。类型为 `Copy`，复制会产生独立计数器快照；对副本的递增不会回写原值。虽然 SQL 正常执行不可能现实地输出超过 `i64::MAX` 行，代码仍显式使用 `wrapping_add`，使 debug/release 构建都具有确定的二进制补码回绕语义：`i64::MAX` 的下一值是 `i64::MIN`。

## 依赖与调用关系

本文件唯一直接导入是标准库 `std::mem::size_of`，没有使用 `Cargo.toml` 中声明的其他 crate 依赖。crate 边界由 `pkg/executor/aggfuncs/Cargo.toml` 的 `[package] name = "astersql-executor-aggfuncs"` 与 `[lib] path = "lib.rs"` 确定，`pkg/executor/aggfuncs/lib.rs` 公开 `row_number` 模块并在测试构建中装入 `row_number_test`。

RustCodeGraph 对目标文件给出的直接使用文件为 `pkg/executor/aggfuncs/row_number_test.rs`、`pkg/executor/aggfuncs/window_func_test.rs` 和 `pkg/executor/aggfuncs/go_scenario_coverage_test.rs`；其中可见的实际符号引用集中在前两个测试，场景覆盖文件是模块级依赖关系，目标 `RowNumber` 未在所核对片段中被调用。`pkg/executor/aggfuncs/builder.rs::build_window_function` 的 `FunctionName::RowNumber -> AggImplementation::RowNumber` 是相邻的构建元数据关系，而不是对本结构方法的直接调用。

下游方法调用都止于本结构内部：`reset` 写字段，`update` 返回常量，`next_value` 调用整数的 `wrapping_add`，`slide` 无操作。该文件不访问 chunk、表达式、存储、网络或事务。

## 错误处理与边界

所有生产方法都是无错误返回：状态结构没有外部输入、分配、解析或 I/O，因此没有 `Result`/`Option` 分支。调用方必须保证每个输出行恰好调用一次 `next_value`，并在分区边界调用 `reset`；本类型不会检测漏调、重复调用或错误的分区生命周期。

整数上界是显式兼容边界。`next_value` 不 panic，而是从 `i64::MAX` 回绕到 `i64::MIN`，`pkg/executor/aggfuncs/row_number_test.rs::row_number_wraps_like_go_int64_arithmetic` 固化了这一约定。`update` 返回的 0 是内存增量，不是行号。`slide` 的无操作语义也不能单独替代实际执行框架对“忽略 frame”的声明；后者由 `pkg/executor/windows/window.rs::RowNumber::ignores_frame` 提供。

## 并发与资源生命周期

`RowNumber` 不包含锁、原子量、引用计数、任务、通道或外部资源。方法通过 `&mut self` 串行修改计数器，Rust 借用规则阻止同一实例被两个安全调用点同时推进；类型没有自行提供跨线程共享机制。虽然其字段可复制且基础类型通常可在线程间移动，正确用法仍是每个窗口分区/部分结果拥有自己的实例，由上层控制创建、复用、重置和销毁。

生命周期从 `default`/值构造开始，随每次 `next_value` 推进，在 `reset` 时逻辑复用，最终按普通栈值或所有者生命周期释放；没有析构副作用。复制状态后会形成两个独立生命周期，不能把多个副本当作同一分区的共享计数器。

## 与 Go 版本的对应关系

本 Rust 文件只对应后者及其状态变换，没有移植 `baseAggFunc`、ordinal、`PartialResult` 类型擦除或 `chunk.AppendInt64` 写出动作。

对应关系如下：Go `AllocPartialResult` 创建零值并返回 `DefPartialResult4RowNumberSize`，Rust 以 `Default` 和 `DEF_PARTIAL_RESULT_ROW_NUMBER_SIZE` 分别表达；Go `ResetPartialResult` 对应 `reset`；Go `UpdatePartialResult` 返回零内存增量和 nil 错误，对应无错误的 `update() -> 0`；Go `AppendFinalResult2Chunk` 先执行 `curIdx++` 再写 chunk，Rust `next_value` 只完成递增并返回值；Go `Slide` 返回 nil 且不改状态，对应 `slide`。

本 Rust 文件没有实现等价 trait；Rust 相邻构建器仅产出 `AggImplementation::RowNumber`，实际 SQL 窗口执行类型位于 `pkg/executor/windows/window.rs`。这是一项当前接线差异，而不是可以从局部测试推断为完成的迁移。

Rust 独立测试保留这些参数契约，并额外直接验证连续编号、重置和溢出回绕。Go `pkg/executor/aggfuncs/window_func_test.go` 还通过通用窗口测试验证四行输出 `1,2,3,4`；Rust `window_func_test.rs` 对目标状态验证同一序列。

## 扩展指南

若只修改局部状态语义，应优先改 `RowNumber::{reset, update, next_value, slide}`，并同步更新独立文件 `pkg/executor/aggfuncs/row_number_test.rs`；不要把测试内嵌回生产源文件。若结构字段变化，必须同步审查 `DEF_PARTIAL_RESULT_ROW_NUMBER_SIZE` 的含义、内存测试期望及 `Copy`/`Default` 等派生是否仍合理。

若目标是让本类型进入完整 Rust SQL 窗口链，需要先决定它与 `pkg/executor/windows/window.rs::RowNumber` 的唯一所有权边界，再补齐从 `AggImplementation::RowNumber` 到运行时实例的接线；不得同时维护两份悄然漂移的计数逻辑。相关验证至少应覆盖 builder 选择、分区重置、frame 被忽略、输出类型/可空性，以及实际执行器的多分区 SQL 行为。相应测试应放在同目录独立 `*_test.rs` 文件或现有 `pkg/executor/windows/window_executor_test.rs`、`window_sql_test.rs` 中。

兼容风险主要有三类：把推进动作从结果阶段移到 update 阶段会造成重复或错位编号；改变 `i64`、回绕或首值语义会偏离 Go；让 `slide` 改状态会错误地把 frame 变化当成分区位置变化。性能上当前操作为常数时间、零堆分配，新增行缓存或共享同步应有明确执行链需求与内存计量测试。
