# `pkg/statistics/handle/autoanalyze/priorityqueue/heap.rs`

## 文件定位

本文件是 `astersql-statistics-handle-autoanalyze-priorityqueue` crate 的底层最大堆实现。crate 入口 `pkg/statistics/handle/autoanalyze/priorityqueue/lib.rs` 将 `heap` 声明为内部模块，再通过 `pub use heap::*` 重导出其公开符号；`Cargo.toml` 用 `package.metadata.porting.go-package` 明确它对应 Go 包 `pkg/statistics/handle/autoanalyze/priorityqueue`。

它位于自动 ANALYZE 调度链的“作业已经构造和打分、等待按优先级消费”这一层：`queue.rs::QueueState` 持有 `PqHeapImpl`，`AnalysisPriorityQueue::push_locked` 先用 `PriorityCalculator::CalculateWeight` 计算权重并写入作业，再调用 `PqHeapImpl::AddOrUpdate`；调度端通过 `AnalysisPriorityQueue::Pop` 取出权重最高的作业。堆不负责生成作业、计算权重、执行 ANALYZE、管理运行中作业或启动后台线程。

## 核心职责

- 以 `AnalysisJob::GetTableID()` 返回的 `i64` 作为唯一 key，保证同一表/分区在堆中至多有一个待调度作业；再次加入相同 key 时替换整个 trait 对象并修复堆序。
- 以 `AnalysisJob::GetWeight()` 为比较依据维护最大堆，根节点始终应是当前权重最大的作业，使 `Peek` 和 `Pop` 能在常数时间定位最高优先级对象。
- 同时维护 `HashMap<i64, HeapItem>` 与 `Vec<i64>`：map 支持按 key 常数期望时间查询，向量支持二叉堆调整；每个 `HeapItem.index` 把两种表示连接起来。
- 提供插入/更新、任意 key 删除、堆顶查看/弹出、列表快照、按 key 查询、长度和空状态接口，供 `queue.rs` 组合成带锁和后台刷新能力的上层队列。

本文件只维护内存数据结构。它不持有锁、不做 I/O、不记录日志，也不校验权重是否为有限值；正确性依赖调用者提供满足排序假设的权重，并且所有修改都通过本文件公开方法进行。

## 主要符号

- `ERR_HEAP_IS_EMPTY: &str = "heap is empty"`：`Peek` 和 `Pop` 在空堆时使用的稳定错误文案，与 Go 的 `ErrHeapIsEmpty` 文本一致。
- `HeapItem`：私有条目，拥有一个 `Box<dyn AnalysisJob>`，并保存该作业 key 在 `queue` 中的当前位置 `index`。它没有独立公开 API。
- `PqHeapImpl`：公开堆类型，派生 `Default`。`items` 保存 key 到条目的映射，`queue` 保存满足最大堆性质的 key 序列；两个字段均为私有。
- `NewHeap() -> PqHeapImpl`：构造两个容器均为空的堆。它返回值而非共享句柄，锁和共享所有权由 `queue.rs` 提供。
- `less(left, right)`：比较两个堆下标对应作业的权重。虽然名称沿用常见堆接口习惯，实际返回 `left.weight > right.weight`，因此构建的是最大堆。
- `swap`：交换 `queue` 中两个 key，并同步改写两个 `HeapItem.index`；这是维持双重表示一致性的关键操作。
- `sift_up`、`sift_down`、`fix`：私有堆序修复函数。`fix` 根据节点是否比父节点更优先选择上浮，否则下沉。
- `AddOrUpdate`、`Update`：拥有式接收 `Box<dyn AnalysisJob>`。新 key 追加到末尾并上浮；已有 key 替换对象后从原下标双向修复。`Update` 是兼容 Go 命名的别名，并不要求 key 预先存在。
- `DeleteByKey`、`Delete`：删除任意条目并返回其作业所有权；`Delete` 从传入作业读取表 ID 后委托 `DeleteByKey`。
- `Peek`、`Pop`：分别借用堆顶对象和移交堆顶对象所有权；空堆均返回 `ERR_HEAP_IS_EMPTY`。
- `List`、`ListKeys`、`GetByKey`、`Get`、`Len`、`IsEmpty`：只读观察接口。文件中没有 trait、泛型、宏、条件编译项或可变全局状态。

## 执行流程

插入或更新由 `AddOrUpdate` 完成：

1. 从新作业读取 `GetTableID()` 作为 key。
2. 若 `items` 已包含 key，保留原 `index`，用新 `Box<dyn AnalysisJob>` 替换旧对象；旧对象在替换时被释放。随后调用 `fix(index)`，权重升高可上浮，权重降低或不高于父节点则尝试下沉。
3. 若 key 不存在，把 key 追加到 `queue` 尾部，在 `items` 中插入记录了尾部下标的新 `HeapItem`，再调用 `sift_up`，直到父节点权重不小于它或到达根。

删除由 `DeleteByKey` 完成：

1. 从 `items` 查找 key；不存在时立即返回 `"object not found"`，两个容器保持不变。
2. 读取目标的 `index` 和最后下标。若目标不在末尾，用 `swap` 把目标换到末尾，同时更新两个条目的 `index`。
3. 弹出 `queue` 末尾 key，再从 `items` 移除目标并取得其 `Box<dyn AnalysisJob>`。
4. 若原位置仍在新向量范围内，对换入该位置的条目调用 `fix`，恢复最大堆性质，然后把已删除作业返回调用者。

`Pop` 先读取 `queue[0]`，再复用 `DeleteByKey`，所以删除根节点后的修复与任意删除共用同一条路径；`Peek` 只借用根节点，不改变容器。`List` 按 `queue` 的当前堆数组顺序解析对象，`ListKeys` 克隆同一序列。该顺序只保证父节点不低于子节点，不是全量降序排序。

## 数据与状态

核心不变量是：

- `items.len() == queue.len()`，且 `queue` 中每个 key 唯一并恰好存在于 `items`。
- 对任意有效下标 `i`，`items[queue[i]].index == i`。
- 对任意非根节点，父节点的权重不小于该节点的权重；比较由 `less(child, parent)` 的严格 `>` 实现。
- key 完全由 `AnalysisJob::GetTableID()` 决定；更新相同 key 不改变长度，而是替换该 key 对应的整个对象。

`items` 拥有实际作业，`queue` 只重复保存轻量的 `i64` key。`Peek`、`Get`、`GetByKey` 和 `List` 返回受 `&self` 生命周期约束的借用，不能在可变更新期间继续使用；`Delete`、`DeleteByKey` 和 `Pop` 返回 `Box<dyn AnalysisJob>`，把对象所有权交给调用者。`ListKeys` 返回独立的 key 副本，可供 `queue.rs::RefreshLastAnalysisDuration` 在后续逐项加锁和更新时遍历。

复杂度上，`Peek`、`Len`、`IsEmpty` 和按 key 查询为 O(1) 或 HashMap 的 O(1) 期望时间；插入、更新、删除和 `Pop` 的堆调整为 O(log n)；`List`、`ListKeys` 为 O(n)，其中 `ListKeys` 还会分配并复制整个 key 向量。

## 依赖与调用关系

直接依赖只有同 crate 的 `job.rs::AnalysisJob` 和标准库 `HashMap`、`Vec`、`Box`。`AnalysisJob` 的 `GetTableID` 提供唯一 key，`GetWeight` 提供堆比较值；trait 本身要求实现者满足 `Display + Send + Sync`。虽然 crate 的 `Cargo.toml` 声明了工作区依赖 `astersql-statistics-handle-logutil`，`heap.rs` 没有直接使用它，也没有自己的 feature 开关。

主要生产调用边为：

- `queue.rs::QueueState::default`、`RebuildWithoutLock`、`Close`、`ResetSyncFields` → `NewHeap`：创建或重置堆。
- `queue.rs::AnalysisPriorityQueue::push_locked` → `PqHeapImpl::AddOrUpdate`：计算并设置权重、注册完成钩子后入堆；这条路径被初始化重建、普通 Push、DML 刷新和 must-retry 重入复用。
- `queue.rs::RefreshLastAnalysisDuration` → `ListKeys` → `DeleteByKey` → `Update`：先取得 key 快照，再取出旧作业、只更新分析时长和权重，最后重新入堆。
- `queue.rs::AnalysisPriorityQueue::Pop` → `PqHeapImpl::Pop`：取出最高权重作业，并由上层把其 ID 加入 `running_jobs`。
- `queue.rs::{PeekForTest, IsEmptyForTest, Len, Snapshot, DeleteByTableID}` 分别消费 `Peek`、`IsEmpty`、`Len`、`List`、`GetByKey`/`DeleteByKey`。
- 堆内部 `Update` → `AddOrUpdate`，`Delete`/`Pop` → `DeleteByKey`，而 `AddOrUpdate` 与 `DeleteByKey` 最终通过 `fix`、`sift_up`、`sift_down` 和 `swap` 维护结构。

RustCodeGraph 对目标文件内调用边识别完整，例如 `AddOrUpdate` 指向 `fix`/`sift_up`，`DeleteByKey` 指向 `swap`/`fix`，`Pop` 指向 `DeleteByKey`；其方法解析没有列出全部跨文件 `queue.rs` 调用，因此上述生产边同时由已索引的 `queue.rs` 源码节点逐行核对。

## 错误处理与边界

公开错误使用 `Result<_, String>`：空堆 `Peek`/`Pop` 返回 `"heap is empty"`；删除不存在的 key 返回 `"object not found"`。`AddOrUpdate` 和 `Update` 当前所有正常路径都返回 `Ok(())`，保留 `Result` 主要用于与上层及 Go API 形状对齐。`Get`/`GetByKey` 用 `Option` 表达缺失，不产生错误。

`less`、`swap`、`Peek` 和删除完成阶段使用 `expect("heap index")`、`expect("heap key")` 或 `expect("heap item")`。这些 panic 不是普通外部输入错误，而是检测 `items`、`queue`、`HeapItem.index` 已经失配的内部不变量破坏。由于字段私有且公开修改入口同步维护两种表示，正常调用不应触发；未来新增修改方法时必须同时更新 map、向量和反向下标。

权重是 `f64`，比较直接使用严格 `>`：等权重节点彼此不会交换，因此没有承诺稳定的全局先后次序；NaN 与任何值比较均为 false，可能使含 NaN 的作业停留在不符合业务直觉的位置。无穷值按 IEEE 754 正常比较。该文件不会拒绝、钳制或规范化异常权重，这一边界与上游 `calculator.rs` 对浮点输入不做合法性校验相连。

`List` 使用 `filter_map`，理论上会静默跳过 `queue` 中无法在 `items` 找到的 key；这种情况同样代表内部状态已经损坏，而不是受支持的缺项语义。调用者也不应把 `List` 或 `ListKeys` 当成全排序结果，只能依赖首元素为最大值和父子堆序。

## 并发与资源生命周期

`PqHeapImpl` 没有内部互斥、原子变量、任务、线程或通道；所有写方法都要求独占的 `&mut self`。生产上 `queue.rs::QueueState` 把它放在 `Arc<Mutex<QueueState>>` 内，`AnalysisPriorityQueue` 在持有状态锁时调用堆方法。因此线程安全边界在上层队列，不在本文件；单独共享堆的调用者必须自行提供同步。

作业由 `Box<dyn AnalysisJob>` 拥有。新插入把所有权移入堆；同 key 更新会立即丢弃旧盒子；删除和弹出把盒子交还调用者；堆整体被替换或释放时，剩余所有作业随 `HashMap` 一起释放。只读接口返回临时借用，不克隆作业，也不延长其生命周期。

`AnalysisJob: Send + Sync` 允许这些 trait 对象随上层受锁状态跨线程使用，但本文件不会调用分析执行方法、成功/失败钩子或释放外部线程资源。上层 `AnalysisPriorityQueue::Close` 先停止并 join 后台 worker，再以 `NewHeap` 替换旧堆；具体作业持有的钩子和其他资源随后按 Rust 所有权规则释放。

## 与 Go 版本的对应关系

Rust `heap.rs` 是同路径 `heap.go` 的直接移植，最大堆算法和错误契约保持一致：两边都用表 ID 作为 key，用严格的 `GetWeight() >` 判定更高优先级；同 key 的 add/update 替换对象后修复堆；任意删除通过末尾元素填位再修复；空 `peek`/`pop` 的错误文本均为 `heap is empty`，缺失删除的错误文本均为 `object not found`。

实现和接口上的已验证差异如下：

- Go 用 `container/heap` 驱动 `heapData::{Less, Swap, Push, Pop}`；Rust 在 `PqHeapImpl` 内显式实现 `sift_up`、`sift_down` 和 `fix`，避免额外 trait 适配层。
- Go `pqHeapImpl` 间接持有 `*heapData`，`newHeap` 返回指针；Rust 把 map 和向量直接放进 `PqHeapImpl`，`NewHeap` 返回拥有值。
- Go `delete` 只返回 `error`；Rust `Delete`/`DeleteByKey` 同时返回被移除的 `Box<dyn AnalysisJob>`，供 `Pop` 和刷新权重流程复用。
- Go `Get`/`getByKey` 返回 `(AnalysisJob, bool, error)`；Rust 用 `Option<&dyn AnalysisJob>` 表达命中或缺失，当前没有查询错误分支。
- Go `list` 和 `ListKeys` 遍历 map，顺序未定义；Rust `List` 和 `ListKeys` 遍历/克隆堆数组，顺序可反映当前堆布局，但仍不是完整排序，也不应作为跨语言稳定顺序。
- Go 对异常内部下标或缺失 map 项部分采用返回 false/nil；Rust 私有算法在不变量失配时使用 `expect` panic。公开合法操作下两者应到达相同业务结果，但损坏状态下的防御行为并不相同。

独立 Rust 测试 `heap_test.rs` 保留 Go `heap_test.go` 的主要用例：同 key 更新、最大权重弹出、任意删除、Update 提权、查询、列表、Peek、空状态和长度；Rust 还明确测试了缺失删除不改变堆，以及空堆稳定错误文案。

## 扩展指南

- 新增改变容器内容或位置的操作时，必须保持三项同步：`queue` 的 key、`items` 的条目、`HeapItem.index`。优先复用 `swap`、`fix`、`DeleteByKey` 和 `AddOrUpdate`，不要直接分别修改两个容器。
- 若要支持不同优先级策略，修改入口是 `less`，但必须同步评估 `calculator.rs` 的权重语义、等权重决策和 NaN 处理，并更新 `heap_test.rs` 与 Go 对照 `heap_test.go`。从最大堆改成最小堆会改变调度行为，不是局部重命名。
- 若需稳定处理等权重作业，应先定义可跨 Rust/Go 复现的第二排序键，例如表 ID 或入队序号；后者需要在状态中保存单调序号，并评估更新是否保留原顺序。仅依赖当前 `Vec` 布局不能形成稳定契约。
- 若新增批量操作，需明确复杂度和原子性：逐项 `AddOrUpdate` 是 O(k log n)，而线性 heapify 需要新的构建流程；无论哪种方式，都必须在上层 `QueueState` 锁内完成或提供不会暴露半成品状态的替换策略。
- 若改变 `Result<String>`、删除返回值或查询的 `Option` 形状，要同步修改 `queue.rs` 的调用路径及公开重导出使用者；同时核对 Go 兼容层，而不是只让当前单元测试编译。
- 测试必须继续放在独立的 `heap_test.rs`，不要内嵌进生产文件。至少覆盖上浮和下沉更新、删除根/中间/末尾、重复 key、缺失 key、空堆、等权重，以及若引入浮点防御策略时的 NaN/无穷值；涉及上层锁和 running 状态的行为应放在 `queue_test.rs`。

## 验证依据

- RustCodeGraph 状态：本地索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/statistics/handle/autoanalyze/priorityqueue` 确认目标 Rust/Go 源与独立测试均已索引。目标文件节点展示完整 202 行和 24 个符号。
- RustCodeGraph 符号证据：`heap.rs::{PqHeapImpl, NewHeap, AddOrUpdate, DeleteByKey, Peek, Pop, List, ListKeys, GetByKey}`；节点 trail 明确显示 `AddOrUpdate -> {fix, sift_up}`、`DeleteByKey -> {swap, fix}`、`Pop -> DeleteByKey`，并列出 `heap_test.rs` 的直接调用者。精确 `callers`/`callees` 子命令在本地索引上未输出并达到超时，因此跨文件边由同一索引的文件节点补充核对，未把超时当成“无调用者”。
- 生产 Rust 证据：`queue.rs::{QueueState, push_locked, RebuildWithoutLock, RefreshLastAnalysisDuration, Pop, Snapshot, DeleteByTableID, Close, ResetSyncFields}`；`job.rs::AnalysisJob` 的 `GetTableID`、`GetWeight` 及 `Send + Sync` 约束；`lib.rs` 的模块声明和公开重导出。
- crate 证据：`pkg/statistics/handle/autoanalyze/priorityqueue/Cargo.toml` 的包名、`lib.rs` 入口、工作区依赖和 `package.metadata.porting.go-package`；目标文件本身只使用 `std::collections::HashMap` 与 `crate::job::AnalysisJob`。
- Go 对照：`heap.go::{heapItem, heapData, pqHeapImpl, newHeap, addOrUpdate, update, delete, peek, pop, list, ListKeys, Get, getByKey, isEmpty}`，逐项核对了算法、错误文本及接口差异。
- 测试证据：Rust `heap_test.rs::{TestHeap_AddOrUpdate, TestHeapEmptyPop, TestHeap_Delete, delete_missing_object_matches_go_error_and_preserves_heap, TestHeap_Update, TestHeap_Get, TestHeap_GetByKey, TestHeap_List, TestHeap_ListKeys, TestHeap_Peek, TestHeap_IsEmpty, TestHeap_Len, empty_heap_reports_canonical_error_and_stable_state}`；Go `heap_test.go` 中对应的十二组主用例。
- 本任务只新增本说明文档，没有修改 Rust、Go、Cargo 或 `plan.md`，并按任务约束不运行 Cargo。结构完整性使用任务指定的 11 章节命令验证；人工复核覆盖文件存在原因、真实生产调用链、堆不变量、错误和浮点边界、所有权/并发边界、Go 差异与安全扩展点。
