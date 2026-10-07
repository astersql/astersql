# `pkg/executor/expand.rs`

## 文件定位

`expand.rs` 属于 `astersql-executor` crate；crate 根 `pkg/executor/lib.rs:102` 以 `pub mod expand;` 公开该模块，`pkg/executor/Cargo.toml` 的 `[lib] path = "lib.rs"` 确认其装配入口。文件只直接导入 `astersql_util_chunk::Chunk`，对应 manifest 中指向 `../util/chunk` 的 `astersql-util-chunk` 路径依赖。

该文件移植的是 root/TiDB 侧 Expand 执行状态机：同一批子执行器输入要依次经过多组 grouping-set 投影，从而为 `GROUPING SETS`、`ROLLUP`、`CUBE` 的后续聚合生成多份输入。Go 主链由 `pkg/executor/builder.go:2517-2546` 的 `buildExpand` 构造 `ExpandExec`；Rust 仓库搜索只找到本文件定义和 `lib.rs` 的模块导出，未找到 `ExpandBackend` 实现、`ExpandExec` 构造或方法调用，因此当前 Rust 模块尚未接入 Rust executor builder/runtime。这里描述的是已实现的状态机能力，不等同于已端到端启用。

## 核心职责

- `ExpandBackend`（`expand.rs:27-46`）把子节点拉取、逐 level 投影、chunk 分配、内存记账、并发统计和生命周期操作抽象为后端契约，使状态机不依赖尚未统一的 Rust executor 上下文类型。
- `ExpandExec`（`expand.rs:49-56`）持有后端、并发配置、缓存输入 chunk、当前 level 游标和 level 总数。
- `Open`/`open`（`expand.rs:60-79`）打开后端、重置资源状态，并建立单线程执行所需的缓存 chunk。
- `Next`/`unParallelExecute`（`expand.rs:82-123`）复用一批子输入，按 level 一次产出一个完整输出 chunk；所有 level 消费完后才拉取下一批子输入。
- `parallelExecute`（`expand.rs:125-128`）明确拒绝尚未实现的并行求值，而不是静默降级。
- `Close`（`expand.rs:130-139`）撤销缓存 chunk 的内存记账、登记并发信息并关闭后端。

## 主要符号

`pub trait ExpandBackend` 定义关联错误类型 `Error` 和十个操作：`open`/`close` 管生命周期；`max_chunk_size`、`new_child_chunk`、`next_child` 管输入；`run_level(level, input, output)` 承担真正的 grouping-set 投影；`consume_memory`、`reset_memory_tracker` 管内存统计；`register_concurrency` 管运行统计；`parallel_not_implemented` 构造并行路径错误。trait 本身没有提供默认实现，仓库中也尚无实现者。

`pub struct ExpandExec<B: ExpandBackend>` 的字段均为公开字段：`backend` 是所有运行时能力的委托对象；`num_workers` 是并行度；`child_result: Option<Chunk>` 是可释放的输入缓存；`level_iter_offset` 是状态游标，`-1` 表示尚未缓存当前输入；`level_count` 必须与后端可执行的 level 集合一致。

`Open<C>`、`Next<C>` 使用泛型上下文 `C`，但文件没有约束其类型。`Open` 先把上下文交给 `backend.open()` 所代表的外层生命周期，随后内部 `open` 不使用上下文；`Next` 则把上下文按值传给一次 `next_child`。`isUnparalleled`、`unParallelExecute`、`parallelExecute` 和 `Close` 构成内部状态机及收尾 API。方法保留 Go 风格命名，文件级 `#![allow(non_snake_case)]`（`expand.rs:22`）专门允许这些名称。

## 执行流程

1. 调用 `Open(ctx)` 时先执行 `backend.open()`；失败立即传播，不创建缓存。成功后进入 `open`，重置内存追踪并把 `num_workers` 强制设为 `0`（`expand.rs:66-72`）。
2. 因为 `isUnparalleled()` 对 `num_workers <= 0` 返回真，初始化把 `level_iter_offset` 设为 `-1`，向后端申请一个子 chunk，登记其初始 `MemoryUsage()`，再存入 `child_result`（`expand.rs:72-77`）。
3. 每次 `Next(ctx, req)` 先按 `max_chunk_size()` 对输出执行 `GrowAndReset`，然后依据并行度分派；当前正常初始化后总是进入 `unParallelExecute`（`expand.rs:82-88`）。
4. 游标为 `-1` 或已经达到 `level_count` 时，执行器在缓存 chunk 上设置与输出一致的 `RequiredRows`，记录拉取前内存，占用 `ctx` 调用 `next_child`，再只登记拉取前后的内存差值（`expand.rs:97-108`）。
5. 子 chunk 行数为零表示上游耗尽，本次返回空输出且不调用 `run_level`。否则游标归零（`expand.rs:109-113`）。
6. 执行器以当前游标作为 `level`，把同一个只读 `child_result` 和可写输出交给 `backend.run_level`；成功后游标加一（`expand.rs:115-122`）。因此一批含 N 行的输入会在连续 `level_count` 次 `Next` 中产生 `level_count` 批投影结果，而不是在一次调用中合并所有 level。
7. `Close` 取走缓存 chunk 并用负的 `MemoryUsage()` 撤销记账，登记非负并发度，最后执行 `backend.close()`（`expand.rs:131-139`）。

## 数据与状态

核心状态转换是 `level_iter_offset: -1 -> 0 -> ... -> level_count`。`-1` 与“已消费完所有 level”都会触发一次上游拉取；成功拉取非空数据后归零。`child_result` 在同一批输入的全部 level 之间保持不变，并以共享引用传入 `run_level`，这保证后端不能通过该接口修改缓存输入。

输出 `req` 每次 `Next` 开始都会重置，输出容量上界来自 `backend.max_chunk_size()`。输入请求行数则从 `output.RequiredRows()` 传给缓存 chunk，并同样受最大 chunk 大小限制。内存只追踪缓存 chunk：初始化登记一次，拉取时登记容量变化差值，关闭时撤销当前总量；输出 chunk 的所有权和记账不在本文件负责范围内。

`level_count` 与后端 level 数据分离保存，文件无法自行验证二者一致。正常调用的不变量是：`Open` 成功后才调用 `Next`；非空输入时 `level_count >= 1`；每个 `0..level_count` 都能由 `run_level` 处理。若非空输入配合 `level_count == 0`，当前代码仍会以 level 0 调用 `run_level`，因此构造端必须阻止这一组合。

## 依赖与调用关系

RustCodeGraph 对 `expand.rs` 的下游边显示：`Open -> backend.open + open`；`open -> reset_memory_tracker + isUnparalleled + new_child_chunk + consume_memory`；`Next -> max_chunk_size + isUnparalleled + unParallelExecute/parallelExecute`；`unParallelExecute -> max_chunk_size + next_child + consume_memory + run_level`；`parallelExecute -> parallel_not_implemented`；`Close -> isUnparalleled + consume_memory + register_concurrency + close`。

文件的唯一具体数据依赖是 `astersql_util_chunk::Chunk`。后端抽象承担了原 Go 实现中 `BaseExecutor`、`expression.EvaluatorSuite`、`memory.Tracker`、runtime stats 和 `exec.Next` 的角色。`pkg/executor/Cargo.toml` 没有为 Expand 声明专属 feature；crate 唯一列出的 `nextgen` feature 与本文件没有条件编译关系，本文件本身也没有 `cfg` 项。

上游架构证据来自 Go：`pkg/executor/builder.go:355-356` 把 `PhysicalExpand` 分派给 `buildExpand`，`builder.go:2517-2535` 把每组 `LevelExprs` 编成 evaluator suite 并构造执行器。Rust planner 侧已有 `PlanKind::Expand` 附着逻辑（`pkg/planner/core/task.rs:1050-1062,1272`），但没有发现连接到本 Rust `ExpandExec` 的 executor builder 边；不能据此推断 Rust 执行器已接线。

## 错误处理与边界

所有可恢复错误统一使用 `B::Error`。`backend.open`、`next_child`、`run_level`、并行未实现错误以及 `backend.close` 都通过 `?` 或直接返回传播；本文件不包装错误，也不重试。`next_child` 失败时不会登记本次内存差值，游标保持原值；调用者仍需执行 `Close` 才能释放已登记缓存。

未调用或未成功完成 `Open` 就调用 `unParallelExecute` 会在 `child_result.expect(...)` 处 panic；这是生命周期违约而非 `B::Error`。与此相对，`Close` 使用 `Option::take`，没有缓存时不会 panic。`run_level` 失败时游标不会递增，若调用方选择在错误后重试，会再次执行同一 level。

空子 chunk 被解释为输入结束，函数以空输出成功返回。文件没有独立的 finished 标志，因此后续再次调用 `Next` 会再次请求上游；正确的上游必须保持 EOF 幂等。`level_count` 与实际可用 level 不一致、过大或为零时的错误完全由构造者/后端契约承担。

## 并发与资源生命周期

当前实现没有线程、任务、锁或通道。`open` 无条件把 `num_workers` 设为 `0`，所以由标准入口打开后只运行单线程路径；这是与 Go `expand.go:60-68` 一致的刻意限制。即使调用方构造时配置了正并发度，打开时也会清零。只有绕过 `Open` 手工保留正值时才可能进入 `parallelExecute`，该路径立即返回 `parallel_not_implemented()`。

缓存 chunk 的资源周期为 `open` 创建到 `Close` 中 `take`。类型没有实现 `Drop`，因此跳过 `Close` 会遗漏后端关闭、内存撤销和并发统计。`Close` 先完成本地记账再调用可能失败的 `backend.close`；即便后端关闭失败，缓存已经释放且不会重复扣减。上下文 `C` 没有 `Clone`/`Copy` 约束，在一次 `Next` 中仅当需要拉取新输入时才按值移交给 `next_child`。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/executor/expand.go`。两版都使用 `levelIterOffset == -1` 表示需要拉取输入，都把输出的 required rows 传给缓存 chunk，都按内存差值记账，都让同一输入依次运行所有 level，并都把并行路径保留为明确的“未实现”错误。Rust 的 `Open`、`Next`、`isUnparalleled`、`unParallelExecute`、`parallelExecute`、`Close` 与 Go 同名方法逐项对应。

Rust 通过 `ExpandBackend` 拆出了 Go 的具体基础设施：Go 的 `BaseExecutor.Open/Close` 对应 `backend.open/close`，`exec.Next` 对应 `next_child`，`EvaluatorSuite.Run` 对应 `run_level`，`memTracker` 对应内存方法，runtime concurrency stats 对应 `register_concurrency`。Go 从 `levelEvaluatorSuits` 的长度得到 level 数量，而 Rust 额外保存 `level_count`；移植接线时必须保证它们同源。

仍存在可观察差异。Go `open` 会创建或重置 tracker 并挂到 statement tracker，Rust 只要求后端重置，具体创建/挂载语义未在仓库实现中验证。Go 仅在 runtime stats 存在时登记并发信息，Rust 总是在 `Close` 调用 `register_concurrency(0)`。Go builder 会建立 evaluator suites 并设置初始并发度，Rust executor builder 尚无对应接线。Go `Close` 的 nil 检查覆盖 `BaseExecutor.Open` 失败场景，Rust 以 `Option` 达到相同的无缓存安全性。

## 扩展指南

接入 Rust 运行主链时，应在 executor builder 中从物理 Expand 的 level 表达式构造一个具体 `ExpandBackend`，并以同一集合长度设置 `level_count`；不要在本文件伪造表达式求值。后端需要保证 `run_level` 的表达式顺序、NULL 填充、grouping ID/position 语义与 Go evaluator suites 一致，并实现 statement 级内存 tracker 的挂载和释放。

若实现并行 Expand，修改点是 `open` 对 `num_workers` 的强制清零、`parallelExecute` 以及与 worker 生命周期配套的 `Close`。必须先定义同一输入的 level 输出顺序、错误取消、输出复用和内存归属；否则会破坏当前逐 level、确定性产出的契约。还应保持小结果和写 memdb 场景的串行策略与 `pkg/executor/builder.go:2537-2546` 及其后续分支一致。

测试应放在独立 Rust 测试文件中，而不是内嵌到 `expand.rs`。最接近的新增位置可为 `pkg/executor/expand_test.rs`，并由 crate 测试模块显式装配；至少覆盖：一批输入按多个 level 复用且顺序正确、level 耗尽后才拉下一批、EOF、`RequiredRows`、内存增减平衡、`run_level`/`next_child` 错误保持游标、未 Open 的生命周期约束、零 level 构造拒绝、Close 幂等策略和并行未实现错误。接入 builder 后还需增加 SQL 级 `ROLLUP/GROUPING` 回归，核对 Go 结果和执行计划。

兼容风险主要是 Go/Rust level 表达式数量或次序漂移；正确性风险是输入 chunk 被错误修改或 EOF 后非幂等拉取；性能风险是每批输入会执行 `level_count` 次投影且当前只能单线程；资源风险是遗漏 `Close` 或 tracker 挂载不一致。

## 验证依据

- Rust 源码：`pkg/executor/expand.rs:22-140`；RustCodeGraph `node --file` 确认文件共 140 行及其 20 个符号。
- RustCodeGraph 查询：`query ExpandExec --kind struct`、`query ExpandBackend --kind trait` 确认 Rust/Go 对照定义；`callees pkg/executor/expand.rs::ExpandExec` 和 `callees ...::ExpandBackend` 得到本文件内部调用边。由于同名符号聚合，`callers` 查询未在时限内返回，故上游结论改由全仓精确引用搜索核验。
- crate 与装配：`pkg/executor/Cargo.toml`、`pkg/executor/lib.rs:102`；全仓 `rg` 只发现 `ExpandExec`/`ExpandBackend` 的本文件定义和模块声明，未发现具体后端或 Rust 构造调用。
- Go 对照：`pkg/executor/expand.go:28-133`；Go 构造入口：`pkg/executor/builder.go:355-356,2517-2546`。
- 规划侧关联：`pkg/planner/core/task.rs:1050-1062,1272`；相关但不覆盖本执行器的 Rust 测试包括 `pkg/planner/core/optimizer_logical_entry_aster_unit_test.rs:1142-1173`、`pkg/planner/core/rule_resolve_grouping_expand_test.rs`、`pkg/planner/core/operator/logicalop/logical_relational_aster_unit_test.rs:201-237` 和 `pkg/planner/core/operator/logicalop/logical_expand_test.rs`。`pkg/executor` 中未找到直接命中 Expand/grouping-set 的 Rust 或 Go 执行器单测。
- 本任务是纯文档分析，按计划不运行 Cargo；结构校验只确认目标文档存在且恰有规定的十一个二级标题。
