# `pkg/executor/typed_limit.rs`

## 文件定位

`typed_limit.rs` 位于 `astersql-executor` crate 中；该 crate 由 [`pkg/executor/Cargo.toml`](Cargo.toml) 的 `[package] name = "astersql-executor"` 与 `[lib] path = "lib.rs"` 定义。模块在 [`pkg/executor/lib.rs`](lib.rs) 中以 `pub mod typed_limit` 公开，因此 [`TypedLimit`](typed_limit.rs) 是 crate 对外可见的 typed executor 实现，而不是未接线的占位文件。

它处于物理计划到执行器的转换链中：[`builder.rs`](builder.rs) 会把 canonical `PhysicalLimit`、索引下推树中的 `PhysicalLimit`，以及 `PhysicalIndexLookUpReader.PushedLimit` 包装为 `TypedLimit`。运行时它位于任意实现 [`ExecExecutor`](adapter.rs) 的子执行器之上，按 `OFFSET` 跳过输入行，再至多输出 `COUNT` 行。

## 核心职责

- [`TypedLimit::new`](typed_limit.rs) 接收一个拥有所有权的 typed 子执行器以及 `offset`、`count`，并预先向子节点申请复用的输入 `Chunk`。
- [`TypedLimit::next_inner`](typed_limit.rs) 在分页拉取模式下跨子 `Chunk` 维护全局的已见行数和已返回行数，保证 `OFFSET` 不会在每个批次重新计算，`COUNT` 也不会因多次 `Next` 被突破。
- `ExecExecutor::Next` 与 `NextWithContext` 复用同一状态机；后者把 `ExecutionContext` 原样传给子节点。
- 只为实际越过 `OFFSET` 且被追加到输出页的行发布锁键。被跳过行的锁键不会通过 `TakeLockKeys` 暴露。
- 将 schema、chunk 配置、扫描计数和外键级联相关操作委托给子执行器；自身只负责 LIMIT/OFFSET 过滤和对应的页级锁键。

## 主要符号

- `pub struct TypedLimit`：唯一公开类型。`child: Box<dyn ExecExecutor>` 表示独占的下游执行器；`offset`、`count` 是不可变限制；`seen`、`returned` 是跨页游标。
- `source` / `source_index`：复用的子节点输入块及下一待处理行位置。`source_index >= source.NumRows()` 表示必须再次拉取子节点。
- `source_keys`：与当前 `source` 对齐的锁键。如果非空，其长度必须等于输入行数。
- `page_keys`：当前一次 `Next` 输出行对应的锁键；由 `TakeLockKeys` 通过 `mem::take` 一次性转移给调用者。
- `opened` / `closed`：本地生命周期门禁，阻止未打开或已关闭实例继续读取。
- `TypedLimit::new(child, offset, count) -> Self`：构造并调用 `child.NewChunk()` 创建输入块，但不打开也不读取子节点。
- `next_inner(context, output) -> AdapterResult`：`Next` 和 `NextWithContext` 的共享实现，负责拉取、跳过、复制、限量及锁键对齐检查。
- `impl ExecExecutor for TypedLimit`：实现 `Open`、`Close`、`Next`、`NextWithContext`、元数据委托、外键委托、锁键转移、扫描计数与 `Detach`。

文件没有模块级常量、枚举、独立 trait、条件编译项或后台任务。

## 执行流程

1. [`builder.rs`](builder.rs) 递归构造 typed 子树，然后以计划节点的 `Offset` 和 `Count` 调用 `TypedLimit::new`。构造只创建缓存 `source`，不会触发实际扫描。
2. `Open` 先调用 `child.Open()`；只有成功后才把 `seen`、`returned`、输入块、输入位置和两组锁键清零，并设置 `opened = true`、`closed = false`。因此重复打开会从 LIMIT 起点重新开始。
3. `Next` 或 `NextWithContext` 进入 `next_inner`。若实例未打开或已经关闭，立即返回错误；否则先 `Reset` 调用者的输出块并清空上一页锁键。
4. 当输出块尚未满且 `returned < count` 时循环。缓存输入已经消费完便调用子节点的相应 `Next*`，重置 `source_index`，并立即用 `child.TakeLockKeys()` 取得该输入页锁键。
5. 子节点返回空块代表 EOF，循环结束。若锁键非空但数量不等于输入行数，则返回对齐错误，避免错误地把某行的锁键关联给另一行。
6. 每消费一行，先递增 `source_index` 与 `seen`。当 `seen <= offset` 时丢弃该行；否则用 `output.Append(&source, index, index + 1)` 复制单行，并在存在锁键时复制相同下标的键到 `page_keys`，最后递增 `returned`。
7. 达到输出块容量、全局 `count` 或子节点 EOF 后返回 `Ok(())`。一旦 `returned == count`，以后调用仍会先重置输出，但不会再读取子节点。
8. `Close` 首次调用时先标记 `closed`，再关闭子节点；后续调用幂等返回成功。`Detach` 则要求子节点可 detach，复制当前游标和缓存，使新实例从同一逻辑位置独立继续。

## 数据与状态

核心不变量是 `seen` 表示已经从子流消费的总行数，`returned` 表示已经向上游交付的总行数，且正常运行时 `returned <= count`。判断 `seen <= offset` 使前 `offset` 行恰好被忽略；例如 `offset = 1` 时第一行被跳过，第二行才可输出。

`source`、`source_index` 和 `source_keys` 构成同一输入页的缓存状态。只有从子节点取得新页时才替换 `source_keys`；消费行时以相同 `index` 取键。空 `source_keys` 表示子节点没有提供逐行锁键，而不是零行输入。`page_keys` 只代表最近一次成功构造的输出页，下一次读取前会清空，取走后也为空。

输出容量由子执行器提供的 `NewChunk`/`ChunkConfig` 决定。实现逐行 `Append`，因而可以跨多个子页填充一个输出页，也可以在一个子页中途停止并于下次继续。`count = 0` 时循环条件一开始即为假，完全不会调用子节点的 `Next`。

## 依赖与调用关系

上游生产调用均在 [`pkg/executor/builder.rs`](builder.rs)：

- `wrap_typed_pushdown_plan` 在索引或表扫描的 canonical 下推子树遇到 `PhysicalLimit` 时构造本类型。
- `build_typed_physical_plan` 在 `PhysicalIndexLookUpReader.PushedLimit` 存在时，于 lookup pipeline 外再包一层本类型。
- 同一构建函数处理普通 `PhysicalLimit` 时，要求恰有一个子节点，递归构造子执行器后再包装本类型。

直接下游接口为 [`adapter.rs`](adapter.rs) 的 `ExecExecutor`。`Open`、`Close`、`Next`/`NextWithContext`、`NewChunk`、`Schema`、外键操作、`ScannedRows` 和 `Detach` 都调用该 trait；数据搬运依赖 `astersql-util-chunk` 的 `Chunk::Reset`、`NumRows`、`IsFull` 与 `Append`，错误依赖 `astersql-errors::New`。这两个 workspace crate 均由 [`Cargo.toml`](Cargo.toml) 直接声明。

RustCodeGraph 将 `TypedLimit` 定位在本文件第 14 行，并显示 [`typed_limit_test.rs`](typed_limit_test.rs) 对它的导入；图索引没有为其方法建立可查询的独立调用边，因此 builder 的三处构造关系使用 `rg` 与源码位置补证。

## 错误处理与边界

- 未成功 `Open` 或已 `Close` 后调用 `Next*`，返回 `astersql_errors::New("limit executor is not open")`。
- `Open` 首先传播子节点打开错误；失败时不会把本地实例标为已打开。
- 子节点 `Next`/`NextWithContext`、`Close`、`CheckForeignKeys` 的错误均原样通过 `?` 或返回值向上传播。
- 非空锁键数组与输入行数不一致时返回 `lock keys do not match typed child rows`；此时输出可能已被重置，但调用者不得将本次读取视为成功页。
- `Close` 在调用子节点前设置 `closed = true`。即使子节点关闭失败，后续 `Close` 也会幂等返回成功，且不能再次读取；扩展错误恢复逻辑时必须注意这一既有语义。
- `Detach` 在子节点不支持 detach 时返回 `None`。成功时复制尚未消费的 `source` 与 `source_keys`、游标和生命周期标记，但故意让新实例的 `page_keys` 为空，避免重复发布已交付页的锁键。
- 大 `offset`、子流提前 EOF、`count = 0` 都通过自然循环条件处理。计数使用 `u64`；当前实现逐行递增，未对理论上的 `u64` 溢出做单独防护。

## 并发与资源生命周期

该类型没有锁、原子变量、通道、线程或异步任务；所有可变状态都通过 `&mut self` 串行访问。它不提供多调用者并发读取保证，调度者必须遵循 `ExecExecutor` 的独占可变调用方式。

资源生命周期为 `new -> Open -> 多次 Next* / TakeLockKeys -> Close`。`Open` 负责重置执行状态，`Close` 负责向下关闭唯一拥有的子执行器。`source` 是跨调用复用的内存块，减少重复分配；`page_keys` 通过转移而非复制交给调用者。`Detach` 通过 child detach 和本地状态克隆产生独立所有权快照，测试验证原实例与 detached 实例随后可以分别继续读取和关闭。

## 与 Go 版本的对应关系

直接语义基线是 [`pkg/executor/select.go`](select.go) 的 `LimitExec`：Go 版本同样维护区间起点/终点与跨批次 cursor，忽略 offset 行、最多返回 count 行，遇到 LIMIT 后不再向子节点取数，并在 `Open` 重置状态、在 `Close` 关闭子树。

Rust 的 `offset`/`count` 与 Go 的 `begin`/`end` 表达方式不同：Rust 分别记录已见与已返回数量；Go 以 `cursor` 对照 `[begin, end)`。Rust 每行搬运，Go 会计算首个有效批次的切片边界并通过 `adjustRequiredRows` 限制对子节点的请求量。因此两者结果语义一致，但批次调度和性能策略并非逐字段复刻。

Go `LimitExec` 还拥有 inline projection 的列交换、adaptive-limit controller、tracing span 和慢关闭日志；这些职责当前不在 `TypedLimit` 中。Rust 特有职责是 typed pipeline 的 `ExecutionContext` 转发、逐行 record lock key 过滤、外键接口委托和 `Detach` 快照。文档不把 Go 的附加能力声明为 Rust 已支持；若未来移植，应分别补生产接线与独立 Rust 测试。

相关 Go 回归基线包括 [`executor_required_rows_test.go`](executor_required_rows_test.go) 的 `TestLimitRequiredRows`，它验证 Go 的 required-rows 优化；该优化不能当作当前 Rust 逐行实现已具备的证据。

## 扩展指南

- 修改 LIMIT/OFFSET 边界时，主要接入点是 `next_inner` 的循环条件与 `seen`/`returned` 更新顺序；必须同步 [`typed_limit_test.rs`](typed_limit_test.rs)，覆盖跨 chunk offset、精确 count、提前 EOF、零 count 和后续调用不再扫描。
- 修改锁读取时，保持 `source_keys.len() == source.NumRows()`（非空时）和“仅返回行发布锁键”两个不变量，并增加错配输入和多行输出页测试，避免锁错行或把被 offset 丢弃的键泄露给上游。
- 新增生命周期能力时，应同时检查 `Open` 重置、`Close` 幂等性和 `Detach` 是否需要复制或清空新状态；尤其不能让 detached 实例重复暴露旧 `page_keys`。
- 若增加 Go 已有的 required-rows、自适应停止、inline projection 或 tracing，需先确认 `ExecExecutor`/`Chunk` 接口能表达相应契约，再在独立 `*_test.rs` 中验证，而不要把测试代码放入本生产文件。
- 修改 builder 接线时应覆盖三条现有入口：普通 `PhysicalLimit`、下推计划中的 `PhysicalLimit`、`PhysicalIndexLookUpReader.PushedLimit`。性能风险主要来自逐行 `Append` 与大 offset 扫描；兼容风险主要来自上下文转发、锁键顺序和重复调用行为。

## 验证依据

- RustCodeGraph：`status` 确认仓库索引可用；`query TypedLimit --kind struct` 与 `node TypedLimit` 定位公开结构；`node --file pkg/executor/typed_limit.rs --offset 1 --limit 260` 读取完整 173 行实现；`node ExecExecutor` 核对 trait 契约；`node --file pkg/executor/builder.rs --offset 260 --limit 420` 核对构建链。方法级 callers/callees 未由当前索引解析，未据此作超出源码的推断。
- 源与装配：[`pkg/executor/typed_limit.rs`](typed_limit.rs)、[`pkg/executor/adapter.rs`](adapter.rs)、[`pkg/executor/builder.rs`](builder.rs)、[`pkg/executor/lib.rs`](lib.rs)。
- crate 边界：[`pkg/executor/Cargo.toml`](Cargo.toml)，确认 crate 名、库入口、`astersql-errors` 与 `astersql-util-chunk` 直接依赖；本文件没有 feature gate。
- Rust 独立测试：[`pkg/executor/typed_limit_test.rs`](typed_limit_test.rs) 验证 offset/count 跨页流式执行、只转发返回行锁键、达到 LIMIT 后不继续扫描、detach 独立续读、零 count 不读子节点，以及 canonical `PhysicalLimit` 能构建惰性 typed pipeline。另有 [`typed_index_lookup_test.rs`](typed_index_lookup_test.rs) 覆盖 `PushedLimit` 计划输入的构造场景。
- Go 对照：[`pkg/executor/select.go`](select.go) 的 `LimitExec.Next`、`Open`、`Close`、`adjustRequiredRows`；[`pkg/executor/executor_required_rows_test.go`](executor_required_rows_test.go) 的 `TestLimitRequiredRows`。
- 本任务是纯文档分析，按计划不运行 Cargo；最终使用任务指定命令验证恰好存在 11 个固定二级标题，并人工复核未把 Go 专属功能描述成 Rust 当前能力。
