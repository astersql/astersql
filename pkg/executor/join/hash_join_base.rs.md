# `pkg/executor/join/hash_join_base.rs`

## 文件定位

该文件是 `astersql-executor-join` crate 的 Hash Join 公共运行时基座，由 [`lib.rs`](./lib.rs) 以公开模块 `hash_join_base` 装配。它不实现连接键编码、哈希表或具体 Join 语义，而是给 [`hash_join_v1.rs`](./hash_join_v1.rs) 与 [`hash_join_v2.rs`](./hash_join_v2.rs) 提供四类共用能力：build/probe 共享状态、probe chunk 顺序取用与回收、build 输入行展平和内存阈值 spill 触发、worker panic 到 `Result` 错误的转换。

[`Cargo.toml`](./Cargo.toml) 将该目录声明为 `astersql-executor-join`，库入口是 `lib.rs`。本文件自身只使用标准库的 `Arc`、`Mutex`、`Condvar`、`VecDeque` 与 panic API，并通过 crate 内部 `joiner::Row`、`row_table_builder::Chunk` 交换行和数据块；它没有直接依赖 Cargo 清单中的其他 workspace crate。当前清单的大部分 Join 依赖仅在 Windows target 下启用，这一构建边界由 Cargo 配置决定，而不是本文件的条件编译逻辑；本文件没有 `#[cfg]` 项。

## 核心职责

1. `HashJoinContextBase` 把 build 生命周期、失败原因、取消标志和 spill 标志封装为可克隆的共享上下文，使 v1/v2 worker 能观察同一份状态。
2. `ProbeSideTupleFetcherBase` 从预置 `VecDeque<Chunk>` 单调取出 probe 输入，并把处理完的 chunk 清空后放入回收池。
3. `BuildWorkerBase` 在开始读取时检查取消，把多个 build chunk 展平为 `Vec<Row>`，并在严格超过可选内存上限时执行调用方提供的 spill 回调。
4. `ProbeWorkerBase::run_guarded` 与 `BuildWorkerBase::run_guarded` 捕获 unwind panic，将载荷转换为字符串，同时把共享 build 状态置为失败并唤醒等待者。
5. `HashJoinWorkerResult` 是执行器 `next` 的批量输出载体；空 `rows` 被 v1/v2 用作耗尽信号，正常输出的 `error` 当前为 `None`，运行错误主要沿外层 `Result<_, String>` 返回。

该文件的职责是协作原语而非独立执行器：真正的 open/build/probe/next 流程分别位于 `HashJoinV1Exec` 和 `HashJoinV2Exec`。

## 主要符号

- `HashJoinWorkerResult { rows: Vec<Row>, error: Option<String> }`：公开结果批次。它派生 `Default`，因此默认值是空行和无错误；v1/v2 的 `next` 以此表示结果耗尽。
- `BuildState::{Building, Finished, Failed}`：私有 build 状态机，默认是 `Building`。`Finished` 只能由 `finish_build` 设置，`Failed` 由 `fail` 设置。
- `SharedState`：私有锁内状态，除 `BuildState` 外保存 `error`、`cancelled`、`spilled`。这些字段由同一把互斥锁保护。
- `HashJoinContextBase { shared: Arc<(Mutex<SharedState>, Condvar)> }`：公开、可克隆的同步句柄。`reset`、`finish_build`、`fail`、`cancel`、`set_spilled`、`is_spilled`、`is_cancelled` 和 `wait_for_build_side` 构成它的 API。
- `ProbeChunkResource { chunk, source_index }`：probe 输入资源。当前取数器总把 `source_index` 设为 `0`，回收逻辑也不读取该字段；它是为来源路由保留的接口形状。
- `ProbeSideTupleFetcherBase { source, recycled, finished }`：私有字段组成的单线程可变取数器。`new` 接收全部 chunk；`fetch_next` 检查取消、返回下一块或耗尽；`recycle` 清空 chunk 后保存；`is_finished` 暴露耗尽状态。
- `ProbeWorkerBase { id, context }`：公开 probe worker 公共字段与 `run_guarded` 包装。
- `BuildWorkerBase { id, context, memory_limit }`：公开 build worker 公共字段；提供 `fetch_build_side_rows`、`check_and_spill_row_table_if_needed` 与 `run_guarded`。
- `panic_message`：私有载荷转换函数，依次识别 `&str`、`String`，否则回退为 `"hash join worker panicked"`。

## 执行流程

典型 v1 流程如下：

1. `HashJoinV1Exec::open` 创建 `BuildWorkerV1`；后者在 `BuildWorkerBase::run_guarded` 内调用 `fetch_build_side_rows`，构建 `HashRowContainer`，再以容器内存量调用 `check_and_spill_row_table_if_needed`。
2. 构建成功后执行器保存哈希表并调用 `HashJoinContextBase::finish_build`；失败则调用 `fail` 并向调用者返回同一错误。
3. `HashJoinV1Exec::produce_all` 首先调用 `wait_for_build_side`。成功后创建 `ProbeSideTupleFetcherBase`，反复 `fetch_next`；每个 chunk 在 `ProbeWorkerBase::run_guarded` 内逐行 probe，完成后调用 `recycle`。
4. `HashJoinV1Exec::next` 按 `max_chunk_size` 切分输出并返回 `HashJoinWorkerResult`，耗尽时返回默认空结果。

v2 复用同一个状态与 worker 防护边界：`BuildWorkerV2::split_partition_and_append` 用 build `run_guarded` 包装分区，`HashJoinV2Exec::fetch_and_probe_hash_table` 在 probe 前等待 build，并用 probe `run_guarded` 包装每块输入。v2 的 probe 输入由自身 fetcher 管理，不使用本文件的 `ProbeSideTupleFetcherBase`；v2 在分区 spill 时直接调用共享上下文的 `set_spilled`。

`wait_for_build_side` 使用 `while state == Building && !cancelled` 防御条件变量伪唤醒。醒来后取消优先于状态判断；否则 `Finished` 成功，`Failed` 返回已保存错误，只有逻辑上不可达的“仍在 Building 且未取消”落入 `unreachable!()`。

## 数据与状态

共享状态的不变量是：所有读取和更新都在 `shared.0` 的 `Mutex` 下完成；`finish_build`、`fail`、`cancel` 在更新后 `notify_all`，因此所有 build waiter 都能重新检查条件。`set_spilled` 只记录事实，不参与等待条件，所以无需唤醒等待者。

状态不是严格的一次性终态：API 没有阻止 `finish_build` 与 `fail` 相互覆盖，`reset` 也能把全部标志恢复为默认值。正确调用顺序由外层执行器保证；新增并发路径若可能重复结束 build，应先定义“首个终态获胜”还是“后写覆盖”的契约。

`ProbeSideTupleFetcherBase` 的 `source` 只从队首弹出，保持输入 chunk 顺序；第一次发现空队列后把 `finished` 置为 `true`，后续调用稳定返回 `Ok(None)`。`recycled` 中的 chunk 已执行 `clear`，但当前实现不会再次从回收池取出并填充，因此“回收”只保留可复用资源，尚未形成循环池。`Chunk` 是 `Vec<Row>` 的别名语义，`fetch_build_side_rows` 通过 `flatten().cloned()` 复制所有行，内存开销与总行数及行宽成正比。

spill 判断采用严格大于：只有 `memory_bytes > memory_limit` 才触发，等于限额不触发；`memory_limit == None` 永不触发。只有 spill 回调成功后才设置共享 `spilled=true`，回调错误会直接返回且不改变标志。

## 依赖与调用关系

上游装配与调用证据如下：

- [`lib.rs`](./lib.rs) 公开 `hash_join_base`，同 crate 的 v1/v2 通过 `crate::hash_join_base` 导入公共类型。
- [`hash_join_v1.rs`](./hash_join_v1.rs) 的 `HashJoinCtxV1` 持有 `HashJoinContextBase`；`BuildWorkerV1::build` 调用 build 基座的三个方法；`HashJoinV1Exec::produce_all` 调用 `wait_for_build_side`、`fetch_next`、probe `run_guarded` 和 `recycle`；`next` 构造 `HashJoinWorkerResult`。
- [`hash_join_v2.rs`](./hash_join_v2.rs) 的 `HashJoinCtxV2`、`BuildWorkerV2`、`ProbeWorkerV2` 分别持有共享上下文或 worker 基座；构建分区与 probe 块由 `run_guarded` 保护，执行器在 probe 前调用 `wait_for_build_side`，spill 路径调用 `set_spilled`，`next` 返回公共结果类型。
- 下游数据类型是 [`joiner.rs`](./joiner.rs) 的 `Row` 和 [`row_table_builder.rs`](./row_table_builder.rs) 的 `Chunk`。本文件不认识连接类型、键列、哈希算法、谓词、分区格式或落盘介质；这些策略由 v1/v2 和 spill 模块负责。

RustCodeGraph 对 `wait_for_build_side` 与 `fetch_next` 找到了独立测试调用，也识别到 v1/v2 的构造和包装关系；但索引对两个同名 `run_guarded` 的方法消歧有限，精确生产调用边以路径限定的文本搜索和上述调用点复核为准。

## 错误处理与边界

- `wait_for_build_side` 是唯一把锁中毒转换为普通 `Err("hash join state poisoned")` 的共享状态 API；其他状态方法用 `expect`，锁中毒会再次 panic。这意味着 worker panic 被 `catch_unwind` 捕获并不等于所有同步故障都可恢复。
- `fail(error)` 保存字符串并唤醒 waiter；若状态为 `Failed` 但错误为空（只能由内部状态异常构造），等待函数回退为 `"build side failed"`。
- `cancel` 不强制改写 `BuildState`，但 `wait_for_build_side`、build fetch 和 probe fetch 都优先返回 `"hash join cancelled"`。已从队列取出的 chunk 不会被本文件异步中断；调用方需在批次边界继续检查取消。
- `run_guarded` 只处理 unwind panic；在 `panic=abort` 或进程级终止下不能恢复。闭包正常返回的 `Err` 直接透传，不自动调用 `context.fail`；外层执行器必须在适当位置登记失败。
- `check_and_spill_row_table_if_needed` 不验证负限额或负内存计数；调用者应提供有意义的字节数。spill 回调可能产生部分外部副作用，本文件不负责回滚。
- `HashJoinWorkerResult.error` 当前不承载 v1/v2 `next` 的普通失败；调用方必须检查外层 `Result`，不能只检查该字段。

## 并发与资源生命周期

`HashJoinContextBase::clone` 只克隆 `Arc`，所有副本共享同一把锁、条件变量和状态。标准使用周期是：默认/`reset` 进入 `Building`；build worker 完成后调用 `finish_build` 或 `fail`；probe waiter 被唤醒后继续或返回错误；关闭/kill 路径调用 `cancel`。`notify_all` 支持多个等待者，但当前 v1/v2 活动路径主要以单个同步阶段使用它。

互斥锁把四类状态串行化，逻辑简单但 `is_cancelled`/`is_spilled` 每次都获取同一把锁；若未来在逐行热路径高频查询，应评估原子字段或拆锁，不能在没有基准证据时改写。条件变量等待正确地在循环中重检谓词。

`ProbeSideTupleFetcherBase` 需要 `&mut self`，本身不在多个线程间并发取数；共享取消通过 `HashJoinContextBase` 观察。回收时资源所有权按值移入，chunk 被清空并保存在 fetcher 内，随 fetcher 一起释放。`BuildWorkerBase::fetch_build_side_rows` 返回新分配的行集合；spill 资源的实际创建、持久化、恢复和清理由具体哈希表与 spill helper 管理，不在本文件内。

## 与 Go 版本的对应关系

同路径 [`hash_join_base.go`](./hash_join_base.go) 是命名和职责来源，但当前 Rust 是面向内存内执行器的收敛接口，不是逐字段翻译：

- Go `hashJoinCtxBase` 持有 session、chunk allocator、并发度、结果/关闭/build 完成 channel、原子完成标志、Join 类型和内存/磁盘 tracker；Rust `HashJoinContextBase` 只保留 build 完成/失败、取消和 spill 状态。
- Go `hashjoinWorkerResult` 用 `*chunk.Chunk`、`error` 和返回 chunk 的 channel 实现结果缓冲复用；Rust 用 `Vec<Row>` 与 `Option<String>`，没有结果回收 channel。
- Go `probeSideTupleFetcherBase` 从真实 `exec.Executor` 读取，通过每 worker channel 分发并回收 chunk，还处理 required rows、build 空表跳过和 panic；Rust fetcher 只消费构造时给定的队列，`source_index` 当前固定为零，回收池也尚未重新投喂。
- Go `wait4BuildSide` 还根据 build 是否为空、是否 spill、Join 类型是否允许跳过 probe、是否需要 probe 后扫表来设置 finished；Rust `wait_for_build_side` 只同步 build 终态，空表优化留在 v2 执行器等上层。
- Go `fetchBuildSideRows` 循环拉取 executor chunk，协调 `WaitGroup`、channel、done/close 信号、增长 chunk 容量、failpoint 与剩余行 spill；Rust方法只在调用开始检查一次取消，然后克隆并展平已有 chunk。
- 两者都保留“worker panic 可观测”和“内存压力触发 spill”的核心意图。Rust 回归测试把 Go 的 panic/kill 场景映射到 `run_guarded`、`cancel` 和等待/取数 API，但没有复刻 Go 的完整 goroutine/channel 调度。

因此扩展或修复时应先判断目标是维持当前 Rust 内存执行模型，还是补齐 Go 的运行时契约；不能仅因名称相似就假定字段、阻塞行为或资源复用已经等价。

## 扩展指南

- 新增共享状态时，集中修改 `SharedState` 和 `HashJoinContextBase` API，并明确它是否参与 `wait_for_build_side` 的等待谓词、是否需要 `notify_all`、`reset` 是否清除它。同步补充独立测试，不要把测试写进本生产文件。
- 调整 build 终态规则时，重点检查 v1 `open`/`produce_all`、v2 `fetch_and_build_hash_table`/`fetch_and_probe_hash_table` 与失败路径，尤其避免 waiter 永久停在 `Building`。
- 扩展 probe 资源多来源路由或真正复用回收池时，应修改 `ProbeSideTupleFetcherBase::{fetch_next,recycle}` 与 `ProbeChunkResource::source_index`，并验证顺序、取消时资源归还、耗尽后的幂等行为。
- 修改 spill 阈值或回调顺序时，必须覆盖“低于、等于、超过阈值”和“spill 回调失败”四种情况；同时检查 v1 `HashRowContainer` 与 v2 分区 spill 的统计一致性，关注峰值内存和重复 spill 风险。
- 修改 panic 策略时，应同时维护 build/probe 两个 `run_guarded`，验证 `&str`、`String`、未知载荷以及闭包普通 `Err`；不要把业务错误误标为 build panic。
- 直接回归位置首选 [`pkg/executor/test/jointest/hashjoin/hash_join_test.rs`](../test/jointest/hashjoin/hash_join_test.rs) 的 `injected_build_and_probe_failures_are_observable`。spill 标志可参考 [`pkg/executor/benchmark_test.rs`](../benchmark_test.rs) 的 build hash table 用例；端到端 v1/v2 语义还应同步相应独立测试文件。性能敏感修改需测量锁访问、全量行克隆与 chunk 回收效果。

## 验证依据

- RustCodeGraph 状态：本地索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件可完整读取为 275 行。
- RustCodeGraph 精确符号查询确认了 `HashJoinContextBase`、`ProbeSideTupleFetcherBase`、`ProbeWorkerBase`、`BuildWorkerBase` 及关键方法的定义位置；组合查询确认 `wait_for_build_side`、`fetch_next` 的独立测试调用，并给出 v1/v2 的构造关系。两个同名 `run_guarded` 的 callers/callees 消歧结果不可靠，本文没有采用其噪声边。
- 已读生产文件：[`hash_join_base.rs`](./hash_join_base.rs)、[`lib.rs`](./lib.rs)、[`hash_join_v1.rs`](./hash_join_v1.rs)、[`hash_join_v2.rs`](./hash_join_v2.rs)、[`Cargo.toml`](./Cargo.toml)；已读 Go 对照：[`hash_join_base.go`](./hash_join_base.go)。
- 已读独立 Rust 测试：[`pkg/executor/test/jointest/hashjoin/hash_join_test.rs`](../test/jointest/hashjoin/hash_join_test.rs) 验证 worker panic 转错、失败唤醒、取消时 build/probe/wait 一致报错；[`pkg/executor/benchmark_test.rs`](../benchmark_test.rs) 验证低内存 build 会同时标记哈希表与共享上下文为 spilled；[`hash_join_test_util_test.rs`](./hash_join_test_util_test.rs) 验证取消错误沿执行辅助路径传播并最终关闭执行器。Go 侧 `pkg/executor/test/issuetest/executor_issue_test.go` 与 `pkg/executor/test/seqtest/seq_executor_test.go` 提供 build 错误和 OOM panic failpoint 的原始回归语义线索。
- 本任务是只读行为分析加单文档产出，没有运行 Cargo 或代码测试。交付结构以任务指定命令验证，另用 Git diff 确认只新增本文档且未改动 `plan.md`、Rust、Go 或 Cargo 文件。
