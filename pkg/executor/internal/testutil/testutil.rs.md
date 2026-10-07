# `pkg/executor/internal/testutil/testutil.rs`

## 文件定位

[源文件 `testutil.rs`](./testutil.rs) 是 `astersql-executor-internal-testutil` crate 的通用测试数据层，位于执行器内部测试工具中。`lib.rs` 将它声明为 `pub mod testutil` 并重导出全部公开符号；根门面 `pkg/lib.rs` 又在 `executor::internal::testutil` 下重导出该 crate。它不是 SQL 请求主链上的生产数据源，而是 Agg、Limit、Sort、Window 等执行器测试用的 mock 基础设施。

`Cargo.toml` 把 crate 根定为 `lib.rs`，并通过 `package.metadata.porting.go-package = "pkg/executor/internal/testutil"` 记录 Go 对照包。当前文件的直接 Rust 依赖只是同级 `../exec` crate 中的 `Chunk`、`ExecContext`、`Executor`、`FieldType`、`Result` 和 `Schema`；manifest 另为 Windows 目标声明表达式、规划器、会话、类型、chunk 和内存等 Go 迁移边界依赖，但本文件没有直接引用它们。

## 核心职责

1. 用 `FieldKind`、`ColumnDef` 和 `Datum` 表示测试所需的 MySQL/TiDB 字段种类、列属性和单元格值。
2. 用 `MockDataSourceParameters` 描述行数、NDV、顺序、预置值、NULL 位图、selection 与会话 chunk 大小，再由 `BuildMockDataSource` 生成分块数据。
3. 使 `MockDataSource` 实现 `Executor` 火山模型接口，为上层算子测试提供可重置、逐块拉取且有 schema/返回类型元数据的叶子执行器。
4. 用 `MockDataPhysicalPlan` 包装指定执行器，支持需要物理计划形状的测试路径。
5. 用 `GenRandomChunks` 覆盖更广的字段值域，用 `MockActionOnExceed` 记录内存超限动作的触发次数。

## 主要符号

- `DEF_INIT_CHUNK_SIZE` / `DEF_MAX_CHUNK_SIZE`：默认初始容量 32 和单块最大行数 1024，被 `SessionVars::default`、`resetChunkSizes` 和 `Executor::{InitCap, MaxChunkSize}` 串起来。
- `FieldKind`：覆盖整数、浮点、Decimal、字符串/Blob、时间、Enum/Set、Bit、JSON 和 Null；`Display` 输出 Go/MySQL 风格类型名。
- `ColumnDef { index, kind, nullable, unsigned }`：列的生成规则。`ColumnDef::new` 默认可空、有符号。`index` 保留 schema 中的位置，当前生成器按 slice 枚举位置写列，不使用该字段重排。
- `Datum`：测试值的枚举容器；`Display` 委托 `Debug` 文本，也被 NDV 去重逻辑当作唯一键。
- `MemoryTracker`、`SessionVars`、`SessionContext`：只保留测试要用的 chunk 大小、限额和挂接状态，不是完整生产会话/内存跟踪器。
- `GenDataFunc`：`Arc<dyn Fn(usize, &FieldKind) -> Datum + Send + Sync>`，允许测试按行号和类型注入确定性值。
- `MockDataSourceParameters`：数据源配置集。`Ndvs` 中 `0` 表示逐行回调/随机，`-1` 表示以预置值为候选池再抽样，`-2` 表示直接使用预置列，正数表示先生成指定数量的不同值再回填 `Rows` 行。
- `DataChunk`：文件内生成阶段的真实 `Vec<Vec<Datum>>` 列存储，可带 selection；`NumRows` 在有 selection 时返回选中行数。
- `MockDataSource`：保留不变的 `GenData`、每次 `Open` 可重置的 `Chunks`、配置 `P`、读取指针 `ChunkPtr` 及 `FieldType`/`Schema` 元数据。关键方法是 `GenColDatums`、`RandDatum` 和 `PrepareChunks`。
- `BuildMockDataSource`、`BuildMockDataSourceWithIndex`：核心构建器；后者先把指定列的 `Orders` 置为 `true`。
- `MockDataPhysicalPlan` / `BuildMockDataPhysicalPlan`：保存 schema 和 `Box<dyn Executor>`；`GetExecutor` 借用内部执行器，`TakeExecutor` 则消费包装并转移所有权。
- `MockActionOnExceed`：以 `AtomicI32` 累计 `Action` 次数，优先级固定为 1。
- `GenRandomChunks` / `randomDatumForField`：按 schema 生成指定行数的多类型列数据。`fieldCode` 是 `FieldKind` 到 MySQL type code 的完整映射，`nextRandom` 是全进程原子 LCG。

## 执行流程

`BuildMockDataSource` 首先检查 `max_chunk_size`：为 0 时通过 `resetChunkSizes` 同时恢复默认初始值和最大值。它再对每个 `ColumnDef` 调用 `fieldCode`，建立与返回字段一致的 `Schema`，并创建尚无 chunk 的 shell 数据源。

随后，构建器对每列调用 `GenColDatums`：

1. `ndv == 0` 时，每行优先调用 `GenDataFunc`，否则调用 `RandDatum`。
2. `ndv == -2` 时，直接取对应预置列。
3. 其他情况先建候选池：`-1` 使用预置值，正数则按 `Datum::to_string()` 去重，直到收集到 NDV 个随机值；然后为每行随机抽取一个候选值。
4. `Orders[col]` 为真时对结果列排序，当前只接受 `Int`、`UInt`、`Double` 和 `Text`。

所有列准备完后，`BuildMockDataSource` 以 `max_chunk_size` 向上取整建立 `DataChunk` 序列，再按行号定位 chunk，对每列优先应用 `Nulls[col][row]`，否则复制已生成的 datum。`HasSel` 为真时，每块从随机的 0 或 1 开始每隔一行建立 selection。最终这些块放入 `GenData`。

拉取生命周期是：`Open` 调用 `PrepareChunks` 克隆 `GenData` 并把 `ChunkPtr` 清零；每次 `Next` 先清空请求 `Chunk`，若尚有数据块，就把请求块的行数设为该 `DataChunk::NumRows()` 并递增指针；耗尽后保持 0 行。`Close` 无操作。重要的当前边界是：`exec::Chunk` 只保存行数、列数和容量元数据，所以 Rust `Next` 只向上层暴露有效行数，不会把 `DataChunk.columns` 交换进请求块。

## 数据与状态

`GenData` 是构建阶段完成后的模板，`Chunks` 是当前一次执行的可消费克隆，`ChunkPtr` 是唯一流式读取状态。因为 `Open` 从模板重建队列，同一实例可重新开启，且不会继承上次的读取位置。

NULL 有两条路径：`BuildMockDataSource` 只根据显式 `Nulls` 位图覆盖已生成值，未提供时创建全 false 位图；`GenRandomChunks` 则由 `ColumnDef::nullable` 控制约 10% 的随机 NULL，`FieldKind::Null` 不受此概率影响，永返回 `Datum::Null`。整数的 `unsigned` 会产生 `Datum::UInt(value * 2)`；有符号整数依随机值分支生成正或负数。

`nextRandom` 的 `STATE` 是进程全局原子状态，初值固定，但消费顺序受同进程其他调用影响；因此它提供可重复的生成算法，不保证单个并发测试拿到固定序列，也不用于密码安全场景。

## 依赖与调用关系

下游依赖集中在 `astersql_executor_internal_exec::executor`：`MockDataSource` 实现其 `Executor: Send` trait，`BuildMockDataSource` 构造其 `FieldType`/`Schema`，`Next` 修改其 `Chunk`，并使用其统一 `Result`。该 trait 定义了 `Open/Next/Close`、schema、返回类型、chunk 容量和子执行器接口；本 mock 是无子节点的叶子，`AllChildren` 返回空 slice，`SetAllChildren` 忽略输入。

上游方面，RustCodeGraph 将本文件标记为被 82 个文件使用，其中包含执行器、DDL 及多个执行器测试路径；crate 公开面通过 `lib.rs` 和 `pkg/lib.rs` 的两级重导出传播。直接 Rust 回归 `testutil_test.rs` 调用 `BuildMockDataSource`、`BuildMockDataPhysicalPlan`、`GenRandomChunks` 和 `MockActionOnExceed`。RustCodeGraph 的精确 `callers/callees` 在 Go/Rust 同名符号消歧后查询超时，因此本文档不声称已枚举全部 82 个调用点。

## 错误处理与边界

执行器生命周期方法当前不产生可恢复错误：`Open`、`Next`、`Close` 均返回 `Ok(())`。错误配置主要以 panic 暴露：

- `ndv == -1` 或 `-2` 但没有相应 `Datums[col]` 时 panic `"need to provide data"`。
- NDV 候选池为空但仍需生成行时，随机取模/索引无法成立；正 NDV 若大于类型生成器可产生的唯一值数，去重循环可无法结束。
- `Orders` 要求排序但 datum 类型不在 `Int/UInt/Double/Text` 或同列存在 `Null` 时，比较分支 panic `"not implement"`。`Float`、Decimal 和时间类型也没有排序分支。
- `RandDatum` 只覆盖整数、Float/Double、Decimal 和字符串/Blob 类型，对其他 `FieldKind` panic。更广的类型应走 `GenRandomChunks`，或由 `GenDataFunc`/`Datums` 提供。
- `BuildMockDataSource` 直接索引 `nulls[column][row]` 和 `column_data[column][row]`；调用者必须保证位图维度、预置列长度与 `DataSchema`/`Rows` 一致。
- `BuildMockDataSourceWithIndex` 直接写 `Orders[*index]`，越界列号会 panic。`MockDataPhysicalPlan::SetID` 是明确的未实现桩。

这些 panic 是测试工具对错误 fixture 的快速失败，不应被解读为面向用户的错误协议。

## 并发与资源生命周期

`MockDataSource` 通过 `Executor: Send` 可被转移到其他线程，但其 `Vec` 和 `ChunkPtr` 是可变状态，并没有内部锁支持多线程同时调用 `Open/Next`。`GenDataFunc` 要求 `Send + Sync` 并由 `Arc` 共享，使配置的克隆和线程转移不因回调而失效。

`MockActionOnExceed` 以 `fetch_add(AcqRel)` 和 `load(Acquire)` 维护计数，因而多线程触发不会丢计数。`nextRandom` 用 `AtomicU64` 的弱 CAS 循环推进 LCG，采用 `Relaxed` 次序：这足以保证状态原子更新和唯一的推进步，但不提供与其他数据的同步边。

资源完全由 Rust 所有权管理，无后台任务、通道、文件、网络连接或显式释放操作。`Close` 是空实现；`TakeExecutor` 是唯一会消费计划包装的操作，其他查询方法只借用或克隆元数据。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/executor/internal/testutil/testutil.go`。Rust 保留了主要结构与流程：`MockDataSourceParameters`、`MockDataSource`、四种 NDV 规则、可选排序和 selection、`PrepareChunks`、`BuildMockDataSource(WithIndex)`、物理计划包装、内存超限计数，以及覆盖广泛 MySQL 类型的随机 chunk 生成。`testutil_test.rs` 中的类型码、默认值、正负 Duration、Enum/Set 非固定值和超限优先级断言直接锁定这些迁移意图。

已确认的差异包括：

- Go 使用生产 `sessionctx.Context`、`expression.Schema`、`types.FieldType`、`chunk.Chunk` 和完整 `exec.BaseExecutor`；Rust 是自包含的简化类型系统，且 `exec::FieldType` 暂只保留 type code。
- Go `Next` 以 `SwapColumns` 把实际列移入请求 chunk；Rust `Next` 仅传递有效行数，生成的 `Datum` 列留在 `DataChunk`。
- Go 随机值同时使用 `crypto/rand` 和 `math/rand`；Rust 统一使用原子 LCG，字符串长度、浮点分布、JSON map 键数等细节与 Go 不完全相同，但测试覆盖的值类别和关键正负/空值域保留。
- Go 的 `MockDataPhysicalPlan` 嵌入完整物理计划接口，并有 `Stats`/`QueryBlockOffset`；Rust 只保留当前测试需要的 schema、executor、ID、explain ID 和内存用量门面。
- Go `MockActionOnExceed` 继承真实 OOM action 并接收 tracker；Rust `Action` 无参数，只维护原子计数。

因此，Rust 实现是面向当前迁移测试的等价骨架，不应宣称已与 Go 生产类型、真实列传输或随机分布逐位一致。

## 扩展指南

- 增加字段类型时，至少同步 `FieldKind`、其 `Display`、`fieldCode`、`RandDatum` 的可用范围和 `randomDatumForField`；若允许排序，还要扩展 `GenColDatums` 的比较分支。
- 增加 NDV、NULL、selection 或分块策略时，修改 `MockDataSourceParameters`、`GenColDatums` 和 `BuildMockDataSource`，并明确检查预置列/位图长度，避免新的隐式越界条件。
- 若要让 mock 参与真实列数据计算，关键接入点是 `exec::Chunk` 的数据表示和 `MockDataSource::Next`；不应只改 `DataChunk` 而假定上层已经可见 datum。
- 扩展物理计划能力时，优先在 `MockDataPhysicalPlan` 上添加测试确实需要的最小接口，并以 Go 同名方法为语义对照；不要将 `SetID` 的 panic 当作可用实现。
- 并发测试若依赖确定随机序列，应增加显式、测试私有的随机状态，而不是推断全局 `nextRandom` 的调度顺序。
- 所有回归逻辑应保持在独立的 `pkg/executor/internal/testutil/testutil_test.rs`，不内嵌到生产 `.rs` 文件。类型码/生命周期变更扩展 `mock_data_source_preserves_go_type_codes_and_chunk_lifecycle`，随机值域变更扩展两个 random chunk 测试，内存动作变更扩展 `mock_action_counts_triggers_and_keeps_go_priority`。

兼容性风险集中在 MySQL type code、Go 字段值域和 `Open/Next` 契约；性能风险集中在构建时按列生成全量 datum、再按行复制到 chunk，以及 `Open` 克隆全部 `GenData`。大行数 fixture 的扩展应专门评估这两次存储/复制成本。

## 验证依据

- RustCodeGraph `status`：索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/executor/internal/testutil` 确认本 crate 的 Rust/Go 对照文件集。
- RustCodeGraph `node --file pkg/executor/internal/testutil/testutil.rs` 分段读取了 1–663 行，并报告该文件被 82 个文件使用；已核对所有常量、枚举、结构、类型别名、公开函数、`impl Executor` 和私有辅助函数。
- RustCodeGraph `query BuildMockDataSource --kind function --json` 将 Rust 符号定位到 `testutil.rs:427`，同时确认 Go 对照符号在 `testutil.go:269`。同名符号消歧后的 `callers/callees` 命令超时且无输出，因此调用点结论限定为索引的文件级使用统计与已读直接测试。
- RustCodeGraph `node` 读取 `pkg/executor/internal/testutil/lib.rs`，确认模块声明、公开重导出和独立 `testutil_test.rs`；读取 `pkg/lib.rs:420` 附近确认顶层门面重导出。
- RustCodeGraph `node` 读取 `pkg/executor/internal/exec/executor.rs:1–210`，核对 `Chunk`、`Schema`、`Result` 和 `Executor` 火山模型契约；读取 `pkg/executor/internal/testutil/testutil_test.rs:1–213`，核对物理计划身份稳定性、Go 默认值、类型码、chunk 耗尽、无符号/Null、Enum/Set/Duration 值域和原子计数。
- 直接读取 `pkg/executor/internal/testutil/Cargo.toml` 核对 crate 边界、Go 包元数据和目标依赖；读取 `pkg/executor/internal/testutil/testutil.go:1–603` 核对 Go 结构、NDV/分块/selection 流程、`SwapColumns`、随机字段生成和 OOM action 语义。
- 本任务为纯文档分析，按计划不运行 Cargo；运行时行为的本地证据限于已有独立测试源码，本次未执行这些测试。
