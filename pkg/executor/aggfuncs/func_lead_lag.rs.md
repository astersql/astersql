# `pkg/executor/aggfuncs/func_lead_lag.rs`

[对应 Rust 源文件](./func_lead_lag.rs)

## 文件定位

本文件位于 `astersql-executor-aggfuncs` crate，crate 根由 `pkg/executor/aggfuncs/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，`lib.rs` 通过 `pub mod func_lead_lag` 暴露该模块。它提供 LEAD/LAG 窗口函数的泛型、预求值状态机：调用方先把目标表达式和当前行默认表达式求值为 `LeadLagRow<T>`，再按分区内行序逐行取结果。

该文件本身不解析 SQL、不排序或划分窗口分区，也不操作 `chunk::Row`。`builder.rs::build_window_function` 会把 `FunctionName::Lead`/`Lag` 路由到 `build_lead_lag`，生成带值类型、偏移和默认常量的 `BuiltAggFunc` 元数据；但仓库内对本文件 `Lead<T>`、`Lag<T>`、`LeadLagRow<T>` 的直接引用目前只见于 `func_lead_lag_test.rs` 和 `go_scenario_coverage_test.rs`。因此，源码证据能确认状态机与构建元数据分别存在，不能据此断言这两个泛型状态对象已经由完整 SQL 执行器实例化。

## 核心职责

- `LeadLagRow<T>` 保存一行已经求值的目标值 `value` 与属于当前行的越界回退值 `default`；二者均以 `Option<T>` 表示 SQL NULL。
- `LeadLagState<T>` 保存固定 `offset`、整个分区的行缓冲和下一个待输出行的游标，并统一实现追加、重置及内存增量估算。
- `Lead<T>` 从当前游标向后取 `offset` 行，`Lag<T>` 向前取 `offset` 行；目标不存在时都使用当前行的 `default`。
- 每次成功调用 `next_value` 恰好推进一个输入行；外层 `Option` 表示是否还有行，内层 `Option<T>` 表示 SQL NULL 或具体值。

## 主要符号

- `LeadLagRow<T> { value: Option<T>, default: Option<T> }`：公开输入载体。`LeadLagRow::new` 只组装两个已求值字段，不做类型转换或表达式计算。
- `LeadLagState<T> { offset, rows, cur_idx }`：共享的私有字段状态。`new(offset)` 固定偏移；`reset()` 清空缓冲并把游标归零，但保留偏移；`update(...)` 追加行并返回 `新增行数 * DEF_ROW_SIZE`；`rows()` 提供只读切片。
- `Lead<T>(LeadLagState<T>)`：公开 LEAD 包装类型。`new`、`reset`、`update` 委托共享状态，`next_value` 使用 `cur_idx + offset` 定位目标。
- `Lag<T>(LeadLagState<T>)`：公开 LAG 包装类型。其接口与 `Lead<T>` 对称，`next_value` 使用 `cur_idx - offset` 定位目标。
- `crate::func_rank::DEF_ROW_SIZE`：本文件唯一导入，作为每个缓存行的固定内存估算值；它不是 `LeadLagRow<T>` 的精确堆内存测量。

## 执行流程

1. 上游按一个已排序窗口分区准备 `LeadLagRow<T>` 序列。每行的 `default` 必须在该当前行上下文中预先求值，这保证越界时使用的不是目标行默认值。
2. 调用 `Lead::new(offset)` 或 `Lag::new(offset)` 创建空状态，随后用一次或多次 `update` 追加分区行。追加保留原迭代顺序。
3. 对每个输入行调用一次 `next_value`。方法先用 `rows.get(cur_idx)?` 判断当前行是否存在；缓冲耗尽时返回外层 `None`，且不推进游标。
4. LEAD 先把 `u64 offset` 转为 `usize`，再用 `checked_add` 计算目标下标；LAG 对应使用 `checked_sub`。转换失败、算术溢出/下溢或 `rows.get` 越界都统一视为目标不存在。
5. 目标存在时克隆目标行的 `value`；否则克隆当前行的 `default`。随后 `cur_idx += 1`，返回 `Some(result)`。
6. 一个分区结束后调用 `reset`；状态可在保留原偏移的前提下复用于下一个分区。

`offset = 0` 时目标就是当前行，因而返回当前行 `value` 而非 `default`。目标行存在但其 `value` 为 SQL NULL 时也返回 NULL，不会再回退到默认值；默认值只用于目标行不存在的情况。

## 数据与状态

状态生命周期以一个窗口分区为单位。`rows: Vec<LeadLagRow<T>>` 会保留所有已追加行，支持 LEAD 读取未来行和 LAG 回看历史行；因此调用方必须在开始输出 LEAD 结果前保证所需未来行已经进入缓冲。`cur_idx` 指向下一次输出对应的当前行，结果数不会超过缓存行数。

`update` 返回的 `i64` 仅按新增元素个数乘 `DEF_ROW_SIZE`，与 Go `UpdatePartialResult` 的逐行 `DefRowSize` 记账意图一致。它不包含 `Vec` 扩容容量、泛型 `T` 拥有的堆对象、克隆成本，也不报告释放量。`reset` 的 `Vec::clear` 会丢弃元素但通常保留容量；文件没有返回负内存增量。

## 依赖与调用关系

- 模块装配：`pkg/executor/aggfuncs/lib.rs` 声明 `pub mod func_lead_lag`，并在 `#[cfg(test)]` 下挂接独立的 `func_lead_lag_test`。
- crate 边界：`pkg/executor/aggfuncs/Cargo.toml` 将本目录定义为 `astersql-executor-aggfuncs`，并用 `package.metadata.porting.go-package = "pkg/executor/aggfuncs"` 标明 Go 对照包。本文件运行时代码只直接依赖同 crate 的 `func_rank::DEF_ROW_SIZE` 和标准库容器/转换能力。
- 构建入口：`builder.rs::build_window_function` 对 `FunctionName::Lead | FunctionName::Lag` 调用 `build_lead_lag`；后者读取常量 offset（缺省为 1）、转换默认常量（缺省为 NULL），再生成 `AggImplementation::Lead { kind, offset }` 或 `Lag { kind, offset }`。
- 直接调用者：RustCodeGraph 与精确引用搜索均显示 `Lead::new`、`Lag::new` 目前由 `func_lead_lag_test.rs` 的 `collect_lead`/`collect_lag` 及 `go_scenario_coverage_test.rs` 驱动。未发现生产 Rust 文件把 `BuiltAggFunc` 的 Lead/Lag 变体实例化成本文件状态对象。
- 下游调用：`Lead::next_value`/`Lag::next_value` 只访问共享状态、标准库的 `usize::try_from`、`checked_add`/`checked_sub`、切片 `get` 和 `T::clone`，没有存储、网络或表达式引擎调用。

## 错误处理与边界

公开方法不返回 `Result`，因为表达式求值已被移出本状态机。空缓冲或结果耗尽返回外层 `None`；有效行上的 SQL NULL 返回 `Some(None)`，两者语义不可混淆。

极大 `u64 offset` 若不能转换为平台 `usize`，LEAD 加法溢出、LAG 减法下溢以及普通分区越界都会安全回退到当前行 `default`，不会 panic。目标位置存在而 `value` 为 NULL 时保留 NULL。唯一未显式保护的算术是成功输出后的 `cur_idx += 1`，但 `cur_idx` 始终来自有效的 `Vec` 下标，所以在 Rust 可表示的 `Vec` 长度约束内不会到达溢出状态。

`update` 的返回值可能在理论上因从 `usize` 转为 `i64` 或乘法而截断/溢出，但实际可分配的分区缓冲通常先受地址空间限制；源码未提供饱和计算或错误返回。新增大分区支持时应把这一点作为记账正确性风险评估。

## 并发与资源生命周期

本文件没有锁、原子、通道、异步任务、I/O 或事务。`update`、`reset` 和 `next_value` 都要求 `&mut self`，Rust 借用规则阻止同一实例被无同步地并发修改；若 `T` 满足相应 auto trait，状态可由外层整体转移或加锁共享，但文件本身不提供并发策略。

资源由 `Vec` 和 `T` 的所有权管理：追加时取得 `LeadLagRow<T>` 所有权，求值时克隆一个 `Option<T>`，`reset`/析构时自动释放元素。由于每个输出可能克隆 `T`，大对象类型的性能取决于 `T::clone`；分区缓冲的峰值空间为 O(分区行数)，逐行定位为 O(1)。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/executor/aggfuncs/func_lead_lag.go`。两版共享核心不变量：按分区缓存全部行；LEAD 使用 `curIdx + offset`，LAG 使用 `curIdx - offset`；越界时在当前行求值默认表达式；每次产出后游标加一；重置清空逻辑长度并归零游标；追加内存按行数乘固定行大小估算。

Rust 版把 Go 的 `baseLeadLag`、`partialResult4LeadLag` 和 `lead`/`lag` 拆成 `LeadLagRow<T>`、共享 `LeadLagState<T>` 及两个泛型包装器。Go 版缓存 `chunk.Row`，在 `AppendFinalResult2Chunk` 内通过 `valueEvaluator` 求值并可能返回错误；Rust 版要求上游提前得到 `Option<T>`，所以 `next_value` 无表达式错误通道，也不直接向结果 chunk 写列。Go 的 `AllocPartialResult` 还报告状态结构体基线大小，Rust 本文件没有等价分配 API，只报告追加行增量。

`func_lead_lag_test.go::TestLeadLag` 覆盖 offset 0、1、2、3、1,000,000，以及 NULL、常量默认值和当前行列默认值；`TestMemLeadLag` 覆盖基线与逐行内存记账。Rust `func_lead_lag_test.rs` 复刻主要 offset/default 矩阵，并额外直接验证重置、耗尽和目标值为 NULL 时不使用默认值。当前 Rust 状态机的预求值接口与 Go 完整执行接口并非一一同型，后续接线必须保留 Go 的求值错误传播和结果类型转换语义。

## 扩展指南

- 修改 offset、越界或 NULL 语义时，应集中修改 `Lead::next_value`/`Lag::next_value`，并同步 `func_lead_lag_test.rs` 的矩阵；同时对照 Go 的 `AppendFinalResult2Chunk`，避免把“目标值为 NULL”误判成“目标不存在”。
- 修改缓冲或内存记账时，应修改 `LeadLagState::update`/`reset`，保留一次追加多批行的增量性质，并补充独立测试文件中的多次 update、reset 后容量/记账场景。不要把 Rust 单元测试内嵌回生产 `.rs`。
- 将状态机接入生产执行器时，接入点应从 `builder.rs` 的 `AggImplementation::Lead/Lag` 实例化路径建立；需要明确把表达式求值结果转换为 `LeadLagRow<T>`、按分区 reset、把外层耗尽与内层 SQL NULL 分开，并恢复 Go 路径具有的表达式错误传播。
- 支持大对象或流式窗口时要评估 O(分区大小) 缓冲、`T::clone` 和固定 `DEF_ROW_SIZE` 估算偏差。LEAD 需要未来行，若改成增量输出，必须证明缓冲水位至少覆盖 offset；LAG 可只保留历史窗口，但这会改变共享状态设计，应分别验证两条路径。
- 新增值类型时需同步 `builder.rs::value_kind`、`AggImplementation::Lead/Lag` 的实例化逻辑和 builder 测试；仅让构建器接受类型不足以证明运行时状态机已接线。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/executor/aggfuncs` 确认目标源、Go 对照与独立测试均在索引内。
- RustCodeGraph 源码读取：`node --file pkg/executor/aggfuncs/func_lead_lag.rs` 核对全部 155 行及 `LeadLagRow`、`LeadLagState`、`Lead`、`Lag`；`node --file .../func_lead_lag_test.rs` 核对 offset/default、内存、reset、耗尽和 NULL 测试；`node --file .../lib.rs` 核对模块与测试装配。
- RustCodeGraph 调用检索：`explore` 确认 `build_window_function -> build_lead_lag`，并列出 `collect_lead`、`collect_lag` 等测试调用；`node --file .../builder.rs` 核对 `FunctionName`、`AggImplementation` 和 `build_lead_lag` 的 offset/default 构建逻辑。对泛型类型引用图边覆盖不足处，再用精确 `rg` 核对直接引用范围。
- 配置与对照读取：`pkg/executor/aggfuncs/Cargo.toml`、`func_lead_lag.go`、`func_lead_lag_test.go`；这些文件分别证明 crate/移植边界、Go 运行语义与 Go 测试矩阵。
- Rust 测试证据：`pkg/executor/aggfuncs/func_lead_lag_test.rs` 与 `go_scenario_coverage_test.rs`。依任务约束，本次纯文档分析未运行 Cargo，也未执行代码测试。
