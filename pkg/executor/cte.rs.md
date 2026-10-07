# `pkg/executor/cte.rs`

## 文件定位

本文件属于 `astersql-executor` crate：`pkg/executor/Cargo.toml` 将 `lib.rs` 声明为库入口，而 `pkg/executor/lib.rs:88` 以 `pub mod cte;` 公开本模块。它把 CTE（Common Table Expression）物化执行的核心状态机写成泛型 Rust 逻辑，覆盖非递归 CTE、递归 CTE、`UNION DISTINCT` 去重、结果端 `LIMIT`、相关列失效、内存/磁盘统计及 spill 测试钩子。

模块采用 `CTEBackend` 反转所有 TiDB 具体类型与副作用。因此，`cte.rs` 本身只直接依赖 Rust 标准库的 `Any`、`HashMap`、`Arc` 和 `Mutex`，具体 executor、chunk、storage、tracker、错误和日志能力全部由后端提供（`pkg/executor/cte.rs:23-25,52-196`）。仓库搜索未找到本文件之外的 `CTEBackend` 实现或 `CTEExec<B>` 构造点；当前可确认的状态是“公开且可编译的通用移植核心”，不能据此声称它已接入真实 Rust SQL 执行主链。

## 核心职责

1. `CTEExec<B>` 提供消费者生命周期：`Open` 初始化游标并协调共享 producer，`Next` 惰性生成完整 CTE 结果后按 chunk 返回，`Close` 关闭生产侧和基类子树（`cte.rs:205-296`）。
2. `cteProducer<B>` 维护 seed/recursive executor、三张物化表、递归轮次、去重哈希、LIMIT、资源 tracker 与相关列快照，并实现 CTE 的同步物化（`cte.rs:314-943`）。
3. `resTbl` 保存所有轮次的最终结果；`iterInTbl` 是本轮递归输入；`iterOutTbl` 暂存本轮递归输出。轮次结束时 `setupTblsForNewIteration` 把输出并入结果并准备下一轮输入（`cte.rs:689-742`）。
4. DISTINCT 以“哈希桶定位 + 全列真实相等比较”处理哈希冲突，先去除当前 chunk 内重复，再去除相对历史存储的重复（`cte.rs:783-906`）。
5. 资源管理通过 `setupCTEStorageTracker`、producer tracker 及 storage 引用计数交给后端；本文件只规定调用时序和错误传播（`cte.rs:338-422,512-575,945-958`）。

## 主要符号

- `RowPointer { chunk_index, row_index }`：定位 storage 或当前 chunk 中的一行，是 `HashTable = HashMap<u64, Vec<RowPointer>>` 的桶元素；同一哈希允许多个指针以处理冲突（`cte.rs:27-50`）。
- `CTELogLevel::{Debug,Info}` 与 `StorageStats`：后端日志/测试断言所需的抽象级别和内存、磁盘、行数快照（`cte.rs:34-47`）。
- `CTEBackend`：本模块的集成契约。其关联类型覆盖上下文、错误、chunk/row、executor、storage、字段类型、相关列、tracker 与 spill action；方法分为生命周期、存储、chunk、比较/哈希、日志以及 failpoint 等能力（`cte.rs:52-196`）。
- `HashContext<F>`：DISTINCT 的字段类型、全列键下标和当前 chunk 哈希缓存，仅在 `isDistinct` 时初始化（`cte.rs:198-203,372-379`）。
- `CTEExec<B>`：每个消费者自己的 `chkIdx`、`cursor`、`meetFirstBatch` 与共享的 `Arc<Mutex<cteProducer<B>>>`；消费者游标互相独立，producer 和物化结果共享（`cte.rs:205-212`）。
- `cteProducer<B>`：核心生产状态。公开字段用于装配 executor/storage/配置，`hashTbl` 和 `hCtx` 保持内部去重不变量（`cte.rs:314-336`）。
- `setFirstErr`：关闭阶段记录每个错误但只返回最先出现的错误，保证后续清理仍继续（`cte.rs:298-312`）。
- `setupCTEStorageTracker`：把 storage 的 tracker 挂到 producer tracker，并让后端同时接入 session memory tracker/spill action（`cte.rs:945-958`）。
- `getCorColHashCode`：把相关列值编码为后端定义的字节哈希，供物化缓存失效检测使用（`cte.rs:960-963`）。

## 执行流程

1. `CTEExec::Open` 复位当前消费者的 chunk/LIMIT 游标，执行 `base_open`，随后取得共享 producer 的互斥锁。若 `checkAndUpdateCorColHashCode` 发现外层绑定变化，先 `producer.reset()` 清空旧物化状态；历史 `openErr` 会直接复用，未生成结果且生产侧未打开时才调用 `openProducerExecutor`（`cte.rs:214-235`）。
2. `openProducerExecutor` 要求 seed 存在并先打开它，重置并创建 statement 子 tracker；有 recursive executor 时还会打开它并创建、引用 `iterOutTbl`；DISTINCT 模式则初始化全列哈希上下文。无论成功失败，`openErr` 和 `executorOpened` 都记录本次尝试，防止共享 producer 被重复打开而泄漏资源（`cte.rs:338-386`）。
3. 首次 `CTEExec::Next` 在锁内调用 `genCTEResult`。该函数先检查 `resTbl` 的持久错误，为三张 storage 配置 tracker，然后同步执行 `computeSeedPart` 与 `computeRecursivePart`；任一步失败都把错误写入 `resTbl`，全部成功才设置 done 标记（`cte.rs:237-251,512-575`）。
4. `computeSeedPart` 将轮次设为 0，反复从 seed 拉 chunk；每批可选去重后写入 `iterInTbl`，同时暂存在局部 `chunks`，循环结束再全部写入初始为空的 `resTbl`，最后把迭代编号推进到 1。LIMIT 达到 `limitEnd` 或空 chunk 都会终止 seed 拉取（`cte.rs:577-617`）。
5. `computeRecursivePart` 若无 recursive executor、初始输入为空、已达到 LIMIT 则直接结束；超过 `max_recursion_depth` 返回后端构造的递归深度错误。它持续拉取 recursive executor：非空 chunk 写入 `iterOutTbl`；空 chunk 表示本轮结束，于是记录/断言统计、调用 `setupTblsForNewIteration`、检查终止条件、增加轮次，并在下一轮前关闭再重新打开 recursive executor（`cte.rs:619-687`）。
6. `setupTblsForNewIteration` 逐 chunk 读取 `iterOutTbl`，DISTINCT 时先完整复制以免选择向量修改与并发 spill 冲突，再对 `resTbl` 去重并加入结果。随后 reopen `iterInTbl`：DISTINCT 模式逐批写入已去重行，`UNION ALL` 则直接与 `iterOutTbl` 交换数据；最后 reopen 清空输出表并重新挂 tracker（`cte.rs:689-742`）。
7. 物化完成后 `getChunk` 从 `resTbl` 返回选中行的拷贝，避免上游修改缓存。启用 LIMIT 时 `nextChunkLimit` 用每个消费者自己的 `cursor` 跨 chunk 跳过 `[0, limitBeg)`，并把输出截断到 `limitEnd`（`cte.rs:424-505`）。
8. `CTEExec::Close` 在锁内关闭 seed/recursive 和 `iterOutTbl`；若结果尚未完成还会 reset producer，然后无论前面是否出错都调用 `base_close`。`setFirstErr` 保留首错并记录后续清理错误（`cte.rs:253-288,388-422`）。

## 数据与状态

- `resTbl.done/error` 是跨消费者共享的物化终态。成功后 `done=true`，失败后保存错误，使后续消费者返回一致结果而不是重新计算（`cte.rs:508-560`）。
- `executorOpened` 表示 seed/recursive 当前是否处于打开生命周期；`openErr` 缓存首次打开结果。二者与 producer 一起受同一 `Mutex` 保护（`cte.rs:220-233,314-336,383-385`）。
- `curIter` 在 seed 阶段由 0 推到 1，之后每完成一个递归轮次加一，并同步写入 `iterInTbl`；深度检查发生在进入递归前和每次准备下一轮后（`cte.rs:581-610,629-669`）。
- `hashTbl` 跨 seed 和所有递归轮次保存已接受行的哈希桶，保证 `UNION DISTINCT` 是全局去重，不仅是单轮去重。`hCtx.key_column_indexes` 覆盖所有输出列（`cte.rs:372-379,769-872`）。
- `sel` 是无原始 selection 时可复用的逻辑行号缓冲；若 chunk 行数超过既有长度会扩展，避免假设 chunk 永不超过初始 max chunk size（`cte.rs:783-805`）。
- `chkIdx`、`cursor`、`meetFirstBatch` 属于消费者，因此多个 `CTEExec` 可独立扫描同一 `resTbl`；producer、storage 和去重状态则共享（`cte.rs:205-212,290-295`）。
- `corColHashCodes` 必须与 `corCols` 按下标对应；`checkAndUpdateCorColHashCode` 直接索引旧哈希，构造方需保证长度一致（`cte.rs:908-919`）。

## 依赖与调用关系

模块入口为 `pkg/executor/lib.rs:88 -> pub mod cte`。RustCodeGraph 对本文件确认的主要内部调用边包括：`Next -> openProducerExecutor/genCTEResult/getChunk`、`genCTEResult -> setupCTEStorageTracker/computeSeedPart/computeRecursivePart`、`computeRecursivePart -> setupTblsForNewIteration/limitDone/logTbls`、`tryDedupAndAdd -> deduplicate -> computeChunkHash/checkHasDup`。

所有实际下游均经 `CTEBackend`：executor 生命周期对应 `open_executor/next_executor/close_executor`；物化表对应 `storage_*`；chunk 操作对应 `chunk_*`；语义比较对应 `hash_chunk_all_columns/rows_equal`；资源链对应 `new_*_tracker`、`attach_*_tracker` 和 `configure_storage_trackers`。因此后端实现是把本状态机接到 executor、chunk、cteutil storage、session variables 与错误体系的必要桥梁。

静态仓库搜索只找到 `pkg/executor/lib.rs` 的模块声明，没有找到 `impl CTEBackend`、本文件外的 `CTEExec<...>` 或 `cteProducer<...>` 使用点。RustCodeGraph 的同名查询同时命中 Go 实现，但没有给出 Rust 泛型类型的外部调用者。故当前上游只能确认到 crate 导出边界，真实 Rust 构造/运行入口为“未接线或未验证”，不能把 Go 的 builder 接线当作 Rust 调用边。

`pkg/executor/Cargo.toml` 声明 crate 名为 `astersql-executor`，无 CTE 专属 feature；`nextgen` feature 仅转发到 import-into。虽然该 manifest 含 `astersql-util-cteutil`、chunk、memory 等大量执行依赖，本文件当前通过后端抽象而未直接引用这些 crate。

## 错误处理与边界

- producer 互斥锁中毒在 `Open`、`Next`、`Close` 转为后端错误 `"CTE storage lock poisoned"`，不会 panic（`cte.rs:220-223,239-242,257-260`）。
- seed 缺失在打开阶段返回显式错误；recursive 缺失代表非递归 CTE并正常结束。相反，进入 recursive 专用路径后 `iterOutTbl` 缺失、DISTINCT 时 `hCtx` 未初始化、物化前 tracker 缺失属于装配不变量，使用 `expect` 触发 panic（`cte.rs:345-350,362-370,521-547,623-625,691-695,796-799`）。
- seed/recursive 的执行体都由 `catch_unwind` 包裹，panic 经 `recovered_panic_error` 转成查询错误；但 `Open`、结果扫描、关闭和辅助方法中的不变量 panic 不在这两个恢复边界内（`cte.rs:577-617,619-687`）。
- storage/executor/chunk/哈希/比较错误均通过 `Result` 原样向上传播。物化计算错误还会写入 `resTbl.error`，之后的 `genCTEResult` 首先返回该错误（`cte.rs:513-559`）。
- LIMIT 使用半开区间 `[limitBeg, limitEnd)`；构造方需保证边界关系有效。`limitDone` 可让 seed 或 recursive 提前停止，但最终读取仍由 `nextChunkLimit` 精确裁剪（`cte.rs:445-505,764-767`）。
- DISTINCT 不仅比较 64 位哈希；同桶候选还经 `rows_equal` 按所有键列比较，避免哈希冲突导致误删（`cte.rs:875-905`）。

## 并发与资源生命周期

`CTEExec` 通过 `Arc<Mutex<cteProducer<B>>>` 共享生产者。`Open`、整个同步物化的 `Next` 以及 producer 关闭均持锁，因此同一 CTE 的多个消费者不会同时生成或破坏共享结果；代价是首次生成期间其他消费者阻塞。`B: Send + Sync + 'static` 保证后端可跨线程共享，但本文件没有创建线程、任务或通道（`cte.rs:52-53,205-212,214-288`）。

生产侧打开顺序为 seed → tracker → recursive → `iterOutTbl.OpenAndRef`，关闭时 seed/recursive 均尝试关闭，随后 `iterOutTbl.DerefAndClose`，再丢弃 producer tracker。`resTbl` 和 `iterInTbl` 是共享表，不在 `closeProducerExecutor` 中关闭，而是在结果失效/未完成时 reopen；其最终所有权由外部装配和后端管理（`cte.rs:338-422,744-762`）。

每次物化或轮次表 reopen 后都会重新调用 `setupCTEStorageTracker`。后端负责把 storage 内存/磁盘 tracker 挂到 statement tracker，并可把 spill action 挂到 session memory tracker；测试模式会等待 spill action，以便断言异步落盘结束（`cte.rs:512-575,711-740,945-958`）。

关闭采用“继续清理、返回首错”策略。相关列变化则在新的 `Open` 中 reopen `resTbl`/`iterInTbl` 并清空 hash/open 状态，确保外层参数改变后不复用旧物化结果（`cte.rs:214-234,298-312,744-752,908-919`）。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/executor/cte.go`。Rust 的 `CTEExec`、`cteProducer`、`Open/Next/Close`、seed/recursive 双阶段物化、三表轮换、全列 DISTINCT、LIMIT、相关列缓存失效、递归深度、panic 恢复、tracker/spill 与首错保留，均沿用 Go 文件的结构和分支顺序。Rust `CTEBackend` 将 Go 中对 `exec.Executor`、`chunk.Chunk`、`cteutil.Storage`、session variables、codec、tracker、日志和 failpoint 的直接调用抽象为关联类型和方法。

需要注意的当前差异：

- Go `CTEExec` 明确实现 `exec.Executor`，并由 Go executor builder 装配；Rust 文件没有实现仓库的具体 executor trait，且未找到 `CTEBackend` 实现或构造点。因此算法对应不等于运行时已接线。
- Go 用 `resTbl.Lock()` 作为共享锁；Rust 把整个 producer 放入 `Arc<Mutex<_>>`，保护范围更显式，但锁中毒新增了 Rust 特有错误路径。
- Go DISTINCT 哈希表是 `join.BaseHashTable`，Rust 使用本地 `HashMap<u64, Vec<RowPointer>>`，仍保留链式冲突候选与真实行比较语义；并发哈希表能力没有直接复刻，因为 producer 已由互斥锁串行保护。
- Go tracker 标签、session fallback action、临时存储开关和具体 failpoint 名称由具体包直接实现；Rust 把这些细节留给 `configure_storage_trackers`、`spill_test_enabled` 等后端方法。没有后端实现时无法验证这些具体副作用。
- Go `Close` 中还有 `mock_cte_exec_panic_avoid_deadlock` failpoint；Rust trait 仅暴露 seed/recursive panic 与 spill 钩子，本文件没有对应的 Close panic 注入点。

相关测试为 `pkg/executor/test/cte/cte_test.go` 和独立 Rust 回归 `pkg/executor/test/cte/cte_test.rs`。Rust 回归保持 Go 测试意图，覆盖 insert-select 无死锁、递归 DISTINCT 与 spill、执行错误、seed/recursive panic 恢复、深度错误后复用、OOM 后清理、相关列共享、迭代 tracker 及计划输出；其中首个 `canonical_recursive_cte_uses_union_distinct_across_iterations` 只是集合模型。由于测试未导入 `astersql_executor::cte`、仓库也没有 `CTEBackend` 实现，它们不能作为本泛型模块直接执行的证据。

## 扩展指南

- 接入真实 Rust 执行器时，首先实现 `CTEBackend` 并在 builder 中构造共享 `cteProducer`/多个 `CTEExec`；必须逐项映射 Go 的 storage 引用计数、statement/session tracker、spill fallback、递归深度错误、行哈希/相等语义和 failpoint。新增实现应放在独立生产文件，测试放在独立 `*_test.rs`，不要把测试内嵌进 `cte.rs`。
- 修改递归协议时重点审查 `computeSeedPart`、`computeRecursivePart` 和 `setupTblsForNewIteration` 的三表不变量：`resTbl` 只累积，`iterInTbl` 只代表下一轮输入，`iterOutTbl` 在轮次交接后必须清空；recursive executor 必须在新输入就绪后再 close/open。
- 修改 DISTINCT 时同步维护 `computeChunkHash`、`deduplicate`、`checkHasDup`，保留“chunk 内 + 历史 storage”两阶段过滤、全列相等复核和 selection 的逻辑/物理下标关系。需要增加直接后端单元测试覆盖空 chunk、已有 selection、哈希冲突、NULL/排序规则及 spill 并发复制。
- 修改 LIMIT 时同步验证 seed 提前停止、recursive 提前停止、跨 chunk offset、`limitBeg == limitEnd`、尾 chunk 截断和多个消费者独立游标；对应符号是 `limitDone` 与 `nextChunkLimit`。
- 修改生命周期或错误处理时验证首次 open 失败被共享、未物化 Close 后可重开、完成结果可在 producer 关闭后继续读、多个关闭错误只返回首错、锁中毒和 panic 恢复边界。Go 的 `TestCTEIssue49096`、`TestCTEPanic`、`TestCTEDelSpillFile` 是兼容意图来源。
- 性能风险集中在同步全量物化、producer 大锁、DISTINCT 的行级比较、`chunks` 临时积累和 DISTINCT 轮次的 `chunk_copy_all`；改动前应保留 spill 安全与结果不可被上游修改的拷贝边界。

## 验证依据

- Rust 源码：`pkg/executor/cte.rs` 全部 963 行；关键符号为 `CTEBackend`、`CTEExec`、`cteProducer`、`genCTEResult`、`computeSeedPart`、`computeRecursivePart`、`setupTblsForNewIteration`、`deduplicate`、`checkHasDup`、`setupCTEStorageTracker`。
- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`query CTEExec/cteProducer` 同时定位 Go/Rust 定义；`callees genCTEResult` 确认 Rust 边到 storage 状态、seed/recursive 和 tracker；`callees computeRecursivePart` 确认递归边到 executor 生命周期、storage、深度、轮换与日志。部分泛型同名方法的 `callers` 无输出，因此外部接线另用仓库搜索核验。
- crate/模块：`pkg/executor/Cargo.toml`、`pkg/executor/lib.rs:88`。仓库搜索 `CTEBackend|CTEExec<|cteProducer<` 未发现本文件外实现/构造；这是“当前未确认接线”结论的依据。
- Go 对照：`pkg/executor/cte.go` 全部 775 行，重点为 `CTEExec`、`cteProducer`、三表说明、生命周期、递归轮换、去重、tracker 与相关列逻辑。
- 测试：`pkg/executor/test/cte/cte_test.go`、`pkg/executor/test/cte/cte_test.rs`、`pkg/executor/test/cte/main_test.go`、`pkg/executor/test/cte/main_test.rs` 及该独立测试 crate 的 `Cargo.toml`。它们提供兼容边界证据，但不是 `cte.rs` 泛型 API 的直接单元测试。
- 本任务为纯文档分析，按计划不运行 Cargo。结构验证要求目标文件存在且恰有十一个规定的二级标题；最终以任务指定命令的退出码为准。
