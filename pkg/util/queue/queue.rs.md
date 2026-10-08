# `pkg/util/queue/queue.rs`

## 文件定位

源码链接：[queue.rs](./queue.rs)。`queue.rs` 是 `astersql-util-queue` crate 的业务实现文件，提供一个泛型、可扩容的环形 FIFO 队列。crate 边界由同目录 `Cargo.toml` 定义，`[lib] path = "lib.rs"`；`lib.rs` 公开 `queue` 模块并以 `pub use queue::*` 再导出本文件的公开符号。仓库根 `pkg/lib.rs` 又通过 `facade_util_queue` 将该 crate 暴露为 `crate::util::queue`，`pkg/lib_test.rs::independent_util_modules_are_wired` 验证该门面可以构造队列。

该实现移植自同目录 `queue.go`。Go 版本当前由 `pkg/executor/join/base_semi_join.go::resetProbeState` 用于保存仍需处理的 probe 行下标；对应 Rust 文件 `pkg/executor/join/base_semi_join.rs` 中的队列接线仍位于注释代码内。虽然 `pkg/executor/join/Cargo.toml` 已声明 `astersql-util-queue` 依赖，现有可检索 Rust 生产源码没有对 `NewQueue` 的有效调用，因此不能把它描述为已经进入 Rust 执行器运行主链。

## 核心职责

- `Queue<T>` 用固定长度槽位、头尾游标和元素计数实现 FIFO 环形缓冲，避免每次出队移动其余元素。
- `Push` 在非零容量队列满时把容量翻倍，并按从 `head` 开始的逻辑顺序重排元素；这保证绕回后的扩容仍保持 FIFO。
- `Pop` 从 `head` 取走所有权并推进队头；`Len`、`IsEmpty` 和 `Cap` 分别暴露逻辑长度、空状态和底层槽位数。
- `Default` 保留 Go 泛型结构体“零值可用”的性质：底层缓冲尚未分配，但首次 `Push` 会分配一个槽位。
- `Clear` 复位逻辑状态且保留容量；`ClearAndExpandIfNeed` 在清空后只按需增大容量，不缩容。

本文件不实现线程安全、阻塞等待、容量上限或错误返回；它是由调用者独占可变访问的内存容器。

## 主要符号

- `pub struct Queue<T>`：公开类型，但四个字段均为私有。
  - `elements: Option<Vec<Option<T>>>`：外层 `Option` 区分 Go 的 `nil` slice 与已分配 slice；内层 `Option<T>` 让 Rust 能在不要求 `T: Default + Clone` 的情况下建立定长空槽，并允许 `Pop` 移出值。
  - `head: usize`：下一次 `Pop` 的物理下标。
  - `tail: usize`：下一次 `Push` 的物理下标。
  - `size: usize`：当前有效元素数，也是判断满队列的依据。
- `impl<T> Default for Queue<T>`：创建 `elements = None`、游标和长度全为零的 Go 式零值。
- `pub fn NewQueue<T>(capacity: isize) -> Box<Queue<T>>`：检查容量可转换为 `usize`，分配恰好 `capacity` 个空槽，并返回堆所有权。负数在转换时 panic。
- `Queue::Push(&mut self, element: T)`：入队；处理零值初始化、满队列扩容、绕回下标和长度递增。
- `Queue::Pop(&mut self) -> T`：出队；空队列 panic，否则从队头槽位 `take()` 元素。
- `Queue::Len(&self) -> usize`、`Queue::IsEmpty(&self) -> bool`、`Queue::Cap(&self) -> usize`：常数时间观察方法。
- `Queue::Clear(&mut self)`：把 `head`、`tail`、`size` 归零，不替换 `elements`。
- `Queue::ClearAndExpandIfNeed(&mut self, size: isize)`：先调用 `Clear`；仅当目标为正且大于当前容量时重新分配目标大小的槽位。
- `fn make_slots<T>(capacity: usize) -> Vec<Option<T>>`：私有分配辅助函数，以 `resize_with` 创建指定数量的 `None`，不构造 `T`。

文件通过 `#![allow(non_snake_case)]` 保留 Go API 的 `NewQueue`、`Push`、`Pop` 等命名，便于逐符号对照移植。

## 执行流程

构造流程如下：

1. `NewQueue` 用 `usize::try_from` 检查有符号容量；负数无法转换并以 `"queue capacity must not be negative"` panic。
2. `make_slots(capacity)` 建立定长空槽；即使容量为零，`elements` 仍为 `Some(Vec::new())`，区别于 `Default` 的 `None`。
3. `head`、`tail`、`size` 均从零开始。

`Push` 的关键流程如下：

1. 若 `elements` 为 `None`，说明来自 `Queue::default()`，先分配容量 1。
2. 若 `size == elements.len()`，队列已满。创建两倍容量的新槽位，按 `old_index = (head + i) % old_capacity` 遍历全部有效元素，并通过 `take()` 搬到新缓冲区的 `[0, size)`。
3. 扩容完成后将 `head` 设为 0、`tail` 设为 `size`，使逻辑顺序变成连续物理顺序。
4. 在 `tail` 写入 `Some(element)`，令 `tail = (tail + 1) % capacity`，最后递增 `size`。

`Pop` 先拒绝 `size == 0`；非空时从 `elements[head]` 取走值，按容量取模推进 `head`，再递减 `size`。只要结构不变量未被破坏，非空队列的队头槽必为 `Some`。

`ClearAndExpandIfNeed` 总是先逻辑清空。目标容量不大于当前容量（包括负数和零）时复用原缓冲；目标为更大的正数时用全新空槽替换旧缓冲。它不会保留清空前的元素。

## 数据与状态

核心不变量是 `0 <= size <= capacity`。容量非零时，`head` 和 `tail` 总在 `[0, capacity)`；空队列可有任意历史 `head/tail`，但 `Clear` 会把二者规范化为零。满队列中 `head == tail`，空队列也可能满足该等式，因此必须由 `size` 区分空与满。

有效元素的逻辑顺序为 `elements[(head + i) % capacity]`，其中 `i` 属于 `[0, size)`。扩容正是按此顺序重排，随后令有效区间连续。`tail` 指向下一个写槽；每次成功 `Push` 增加 `size`，每次成功 `Pop` 减少 `size`。

`Pop` 会将对应槽位变回 `None`。`Clear` 只重置计数器，不逐槽 `take()`，因此清空前尚未弹出的 `T` 仍由底层 `Option` 持有，直到这些槽位被后续 `Push` 覆盖、缓冲被扩容替换或整个队列析构；对持有大量资源的 `T`，这意味着 `Clear` 保容量的同时也可能延迟资源释放。这与 Go `Clear` 不清零 slice 槽位的行为相符。

## 依赖与调用关系

本文件只依赖 Rust 标准库：`Box`、`Vec`、`Option`、`usize::try_from` 及其 panic/索引语义；`pkg/util/queue/Cargo.toml` 没有声明第三方运行依赖或 feature。

RustCodeGraph 的精确边显示：

- `NewQueue -> make_slots`；
- `Push -> make_slots`；
- `ClearAndExpandIfNeed -> Clear` 且 `ClearAndExpandIfNeed -> make_slots`；
- `lib.rs` 通过 `pub mod queue` 和 `pub use queue::*` 提供 crate API。

上游方面，`pkg/lib.rs` 的 facade 再导出该 crate，`pkg/lib_test.rs::independent_util_modules_are_wired` 经 `crate::util::queue::NewQueue` 验证公开路径。同目录的两组 Rust 测试直接调用所有公开行为。仓库中的 Rust 执行器虽已在 Cargo 清单声明该 crate，但 `base_semi_join.rs` 中对应 `NewQueue`、`ClearAndExpandIfNeed`、`Push` 调用仍为注释，不构成运行时调用边。

Go 侧真实调用链为 `baseSemiJoin.resetProbeState -> queue.NewQueue`，随后根据 probe 行状态调用 `ClearAndExpandIfNeed` 和 `Push`；消费路径再从该队列弹出未完成行下标。该 Go 链路解释了本通用容器在完整数据库中的预期用途，但不能作为 Rust 已接线的证据。

## 错误处理与边界

API 没有 `Result` 或可恢复错误，边界失败均表现为 panic：

- `NewQueue` 的负容量在 `usize::try_from(...).expect(...)` 处 panic；迁移测试只要求发生 panic，未锁定该 Rust 专用文案。
- `Pop` 在空队列上以精确文案 `"Queue is empty"` panic，与 Go 实现一致。
- `NewQueue(0)` 本身成功，但第一次 `Push` 会先执行“满队列扩为两倍”，零的两倍仍为零，随后写 `elements[0]` 触发越界 panic。测试明确把这视为与 Go 零容量 slice 下标写入一致的行为，而不是自动修正为容量 1。
- `Queue::default()` 与 `NewQueue(0)` 不等价：前者首次 `Push` 分配容量 1 并可用，后者保留零容量构造的 panic 边界。
- `ClearAndExpandIfNeed` 接受负数时只清空、不扩容；条件先检查 `size > 0`，因此不会把负数转换为巨大 `usize`。
- `Pop` 内部关于已初始化缓冲和槽位为 `Some` 的 `expect` 是不变量断言。字段私有且所有修改方法受控，正常公开 API 流程不应触发它们。

容量翻倍使用 `current_len * 2`，未显式处理 `usize` 溢出或分配失败；极端容量下依赖 Rust 构建模式和分配器行为。实现也没有容量收缩策略。

## 并发与资源生命周期

所有修改方法都要求 `&mut self`，本文件没有锁、原子变量、通道、后台任务或异步代码；同一实例的并发协调完全由调用者负责。Rust 类型系统会阻止安全代码同时持有多个可变引用。`Queue<T>` 是否可在线程间发送或共享由字段的自动 trait 推导和 `T` 决定，本文件没有额外的 `unsafe impl`。

`NewQueue` 返回 `Box<Queue<T>>`，队列及底层 `Vec` 由调用者拥有并在离开作用域时析构。普通满队列扩容时，全部有效值逐个从旧槽移动到新槽，旧空槽随旧 `Vec` 析构；`ClearAndExpandIfNeed` 确实替换缓冲时，会析构清空前仍滞留在旧槽中的值。`Pop` 将一个 `T` 的所有权交给调用者。覆盖旧槽、按需替换缓冲以及队列析构都会按 Rust 的常规规则析构仍被槽位持有的值。

当前实现没有借用返回值，因而扩容不会给外部留下指向旧缓冲的引用；代价是 `Pop` 必须移动并拥有元素。

## 与 Go 版本的对应关系

Rust 的 `Queue<T>`、`NewQueue`、`Push`、`Pop`、`Len`、`IsEmpty`、`Clear`、`ClearAndExpandIfNeed`、`Cap` 与 `queue.go` 同名符号逐一对应，FIFO、环形绕回、满时容量翻倍、清空复用、仅扩不缩和空 `Pop` panic 的主要语义保持一致。

必要的语言映射差异包括：

- Go 使用 `[]T`；Rust 使用 `Option<Vec<Option<T>>>` 同时表达 nil slice、定长未占用槽以及可移出所有权的元素。
- Go 构造器返回 `*Queue[T]`；Rust 返回拥有所有权的 `Box<Queue<T>>`。
- Go 的 `int` 映射为公开容量参数的 `isize`，内部下标和计数使用 `usize`；负参数由显式分支/转换条件保持相应边界。
- Go 扩容时对元素赋值；Rust 用 `Option::take` 移动元素，因此无需 `T: Clone`。
- Go `Pop` 返回槽位值但不清零旧引用；Rust `Pop` 必须清空槽位才能移出 `T`。对于队列可观察的值序列二者等价，但资源释放时机可能不同。
- Rust 以 `Default` 显式提供 Go 零值语义；`NewQueue(0)` 则保留 Go 构造零长度 slice 后首次写入 panic 的行为。

`queue_test.go::TestQueue` 覆盖基础 FIFO、扩容、清空、空弹出和环形游标；`queue_test.rs` 在此基础上增加非 `Copy` 值、绕回后扩容、零值与零容量边界。`migration_aster_unit_test.rs` 进一步固定负容量和负扩容参数行为。

## 扩展指南

- 修改入队、扩容或下标算法时，首先维护 `size <= capacity`、有效逻辑区间均为 `Some`、`tail` 指向下一写槽这三个不变量；尤其要在 `queue_test.rs::wraparound_growth_preserves_logical_order` 中补充跨越缓冲边界的回归场景。
- 修改清空策略时需明确是否仍与 Go 保持“保留容量且不主动清槽”的资源生命周期；若决定立即释放元素，必须同步评估 Go 对照、析构副作用和性能，并更新 `clear_keeps_capacity_and_allows_non_copy_values_to_be_reused` 及迁移测试。
- 若增加 `Peek`、迭代器或借用式访问，应考虑元素可能跨物理尾部、扩容会移动值，以及返回引用与后续 `&mut self` 操作的互斥；测试仍应放在独立的 `queue_test.rs`，不要内嵌到生产文件。
- 若改变容量参数类型或零/负容量策略，必须同时核对 `NewQueue`、`Push`、`ClearAndExpandIfNeed`，并同步 Go 行为测试或明确记录有意差异。零值 `Default` 与显式零容量构造是两个不同契约，不应无意合并。
- 若把该队列接入 Rust 半连接执行器，应修改真正的调用方而非本容器，并用执行器独立测试证明 reset、复用、扩容和消费顺序；当前注释中的 `base_semi_join.rs` 只能作为迁移意图线索。
- 若需要并发队列、阻塞语义、容量上限或无 panic API，宜在调用层封装或设计新的明确接口，不应在不评估现有 Go 兼容性的情况下改变这些公开方法。
- 性能审查应关注满队列扩容的 O(n) 移动、`Clear` 对大对象的延迟释放以及 `Box` 的额外堆分配；普通 `Push`、`Pop` 和观察方法均为摊销 O(1)。

## 验证依据

- RustCodeGraph 索引状态：项目索引覆盖 `pkg/util/queue`，其中 `queue.rs` 识别出 12 个符号；通过 `node --file pkg/util/queue/queue.rs` 阅读了完整 180 行实现。
- RustCodeGraph 符号/调用查询：`query Queue --kind struct`、`query NewQueue --kind function`、`node NewQueue`、`callees NewQueue --file pkg/util/queue/queue.rs --json`、`callees ClearAndExpandIfNeed --file pkg/util/queue/queue.rs --json`、`callers make_slots --file pkg/util/queue/queue.rs --json`。这些查询确认构造器、清空扩容方法和私有分配函数的直接边；通用方法名的 callers/callees 查询存在跨仓库重载歧义，因此外部接线结论另以限定路径的 `rg` 复核，未把歧义结果当作调用事实。
- 已读 Rust 路径：`pkg/util/queue/queue.rs`、`pkg/util/queue/lib.rs`、`pkg/util/queue/queue_test.rs`、`pkg/util/queue/migration_aster_unit_test.rs`、`pkg/lib.rs`、`pkg/lib_test.rs`、`pkg/executor/join/base_semi_join.rs`。
- 已读配置路径：`pkg/util/queue/Cargo.toml`、根 `Cargo.toml`、`pkg/executor/join/Cargo.toml`；据此核对独立 crate、workspace 成员、facade 和执行器依赖声明。目标目录没有 `doc.go`。
- 已读 Go 对照：`pkg/util/queue/queue.go`、`pkg/util/queue/queue_test.go`，并以 `pkg/executor/join/base_semi_join.go` 的导入与 `resetProbeState` 调用确认 Go 生产用途。
- 行为测试证据（本任务按计划不运行 Cargo）：`queue_test.rs` 覆盖 FIFO、2→4 扩容、绕回后 3→6 扩容、`Clear` 保容量、仅扩不缩、默认零值、空 `Pop` 文案和零容量 `Push` panic；`migration_aster_unit_test.rs` 覆盖重用、负构造容量 panic 与负目标容量只清空不扩容。
- 结构验证使用任务指定命令，要求本文档存在且固定二级标题恰好为 11 个；验证结果在任务交付时记录。
