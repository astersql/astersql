# `pkg/store/mockstore/mockcopr/topn.rs`

## 文件定位

本文件是 `astersql-store-mockstore-mockcopr` crate 内部的 TopN 数据结构实现，源码入口由 [`lib.rs`](lib.rs) 的私有 `mod topn;` 声明装配。它不解析请求、不计算 ORDER BY 表达式，也不直接读取 KV；上游 [`cop_handler_dag.rs`](cop_handler_dag.rs) 把 `ExecutorSpec::TopN` 构造成 [`executor.rs`](executor.rs) 中的 `topNExec`，后者计算排序键后调用本文件维护容量受限的候选集合。

[`Cargo.toml`](Cargo.toml) 将该目录定义为独立库 crate，并以 `package.metadata.porting.go-package = "pkg/store/mockstore/mockcopr"` 指向 Go 对照包。本文件自身只使用标准库 `std::cmp::Ordering`，以及同 crate `copr_handler` 模块的 `ByItem`、`Datum`、`Row` 和 `CopError`；Cargo 中列出的可选 AsterSQL 子 crate 并非本文件的直接依赖。

## 核心职责

该文件把 `ORDER BY + LIMIT N` 拆成两个阶段：扫描输入时，`topNHeap` 最多保留当前排序意义上最优的 N 行；输入耗尽后，`topNSorter::sort` 再将候选行按最终 ORDER BY 顺序排列。这样，面对 M 行输入时，无须保存并全量排序所有行，候选存储量受 `totalCount` 限制，维护成本约为 `O(M log N)`，最终候选排序约为 `O(N log N)`。

比较规则集中在 `compare_rows`：依照 `orderByItems` 的顺序逐键比较，第一个不相等的键决定顺序，`descending` 会反转该键的比较结果；所有键相等则返回 `Ordering::Equal`。堆使用反向比较，使下标 0 始终是已接纳候选中的“最差行”，从而可用更优的新行替换它。

## 主要符号

- `sortRow { key: Vec<Datum>, data: Row }`：将已求值的 ORDER BY 键与原始行绑定。`key[index]` 应对应 `orderByItems[index]`；构造发生在 `executor.rs::topNExec::evalTopN`。
- `topNSorter { orderByItems, rows, err }`：持有比较配置、候选行和可选比较错误。`new` 建立空缓冲，`Len`/`Swap`/`Less` 提供 Go `sort.Interface` 风格的兼容方法，私有 `compare` 统一转调 `compare_rows`，私有 `sort` 使用稳定的 `Vec::sort_by` 做最终排序。
- `compare_rows(order_by, left, right)`：无副作用的词典序比较函数。实现通过 `left.key.get(index).cmp(&right.key.get(index))` 比较键；正常入口保证键数与 ORDER BY 项数一致。若被其他 crate 内代码错误构造为短键，`Option` 的顺序会参与比较，而不是返回错误。
- `topNHeap { topNSorter, totalCount, heapSize }`：有界最大堆；`totalCount` 是容量，`heapSize` 是当前参与堆运算的元素数，正常情况下等于 `topNSorter.rows.len()`。
- `topNHeap::Push`：追加行、递增 `heapSize` 并调用 `sift_up`。这不是标准库 `BinaryHeap` 的接口，而是本文件自己的堆操作。
- `topNHeap::Less`：将普通 ORDER BY 比较反转；返回 true 表示左行比右行更差，因此最差行向堆顶移动。
- `topNHeap::tryToAddRow`：容量为零时拒绝；未满时直接入堆；已满时仅在新行严格优于堆顶时替换并 `sift_down`，返回值表示该行是否被接纳。
- `topNHeap::intoSortedRows`：消费堆；先传播 `topNSorter.err`，再按正常 ORDER BY 方向排序，丢弃排序键并返回原始 `Vec<Row>`。
- `sift_up` / `sift_down`：分别维护插入后和替换堆顶后的堆序不变量。
- `topNHeap::Pop`：当前固定返回 `None` 的兼容占位。当前 Rust 调用链不依赖它，不能把它当作可用的删除堆顶操作。

这些类型和多数方法声明为 `pub`，但 `lib.rs` 中 `topn` 模块本身是私有且没有再导出，因此它们当前主要是 crate 内部 API，而不是该 crate 的外部公开接口。

## 执行流程

1. `cop_handler_dag.rs::buildExecutor` 遇到 `ExecutorSpec::TopN { order_by, limit }`，调用 `buildTopN` 创建 `executor.rs::topNExec`，并把前一个执行器接为其 `src`。
2. `topNExec::Next` 首次执行时调用 `topNHeap::new(limit, order_by.clone())`。构造结果的 `rows` 为空，`heapSize` 为 0。
3. `topNExec::innerNext` 反复从上游 `Next` 取行；`evalTopN` 对每个 `ByItem.expr` 求值，生成与 ORDER BY 项一一对应的 `Vec<Datum>`，再构造 `sortRow` 调用 `tryToAddRow`。
4. 堆未满时，`Push` 把新行放到末尾并 `sift_up`。由于 `topNHeap::Less` 使用反向比较，更差的行会逐层上浮，维持堆顶为当前最差候选。
5. 堆已满时，`tryToAddRow` 用正常比较检查新行是否严格优于根。若是，则覆盖根并 `sift_down`，在两个子节点中选择更差者上移；否则丢弃新行。相等行不会替换既有候选。
6. 上游耗尽后，`topNExec::Next` 取走工作堆并调用 `intoSortedRows`。候选按正常 ORDER BY 方向排序、去掉键后转为 `VecDeque<Row>`，随后每次 `Next` 从队首返回一行。
7. `limit == 0` 时每行都会在 `tryToAddRow` 的首个分支被拒绝，最终输出空集合，也不会访问空堆的根节点。

## 数据与状态

`sortRow.key` 是比较数据，`sortRow.data` 是最终返回的数据。键在进入堆之前一次性求值，避免堆调整和最终排序期间重复执行表达式。`Datum::Ord`（定义在 `copr_handler.rs`）决定基础值顺序：NULL 最小；整数、无符号整数和浮点数支持跨数值类型比较；字节串按字节序比较；不同类型族依 `datum_tag` 排序。当前简化类型没有携带 Go 版本比较所需的 StatementContext 或 collation。

核心不变量是：`heapSize <= totalCount`、`heapSize == topNSorter.rows.len()`，并且非空堆的 `rows[0]` 是当前最差候选。`Push` 同步增加向量长度和 `heapSize`；满堆替换只覆盖元素，不改变二者；`intoSortedRows` 消费整个对象，避免排序完成后继续按堆使用同一缓冲。

相同排序键没有额外的行句柄或输入序号作为决胜键。最终 `sort_by` 是稳定排序，但此前的堆调整可能改变候选在缓冲中的相对位置，因此当前实现不承诺全局稳定的同键输出次序。调用者若需要确定性的并列顺序，应在 `order_by` 中提供显式的后续键。

## 依赖与调用关系

直接上游调用链为：

`copr_handler.rs::ExecutorSpec::TopN` → `cop_handler_dag.rs::buildExecutor/buildTopN` → `executor.rs::topNExec::Next/evalTopN` → `topn.rs::topNHeap::{new, tryToAddRow, intoSortedRows}`。

`topNExec::evalTopN` 是 `sortRow` 的正常构造点，保证每个 ORDER BY 项都求得一个键；表达式求值失败会在进入本文件前返回。`topNExec::Next` 是 `intoSortedRows` 的直接调用者，并负责把结果转为输出队列。本文件下游依赖 `ByItem.descending` 和 `Datum::Ord`，不调用存储、RPC、线程或异步运行时。

RustCodeGraph 已索引目标文件及其 20 个符号，但其对本文件 Rust 方法的 callers 查询没有给出方法级边；直接引用搜索确认实际 Rust 使用点仅位于 `executor.rs`，而模块装配点位于 `lib.rs`。RustCodeGraph 对 `compare_rows` 的 callees 查询也显示该目标定义没有进一步调用边，这与其仅执行本地值比较的实现一致。

## 错误处理与边界

- 正常可返回的错误主要产生在上游：`topNExec::innerNext` 传播源执行器错误，`evalTopN` 传播 ORDER BY 表达式求值错误，以及未初始化堆时返回 `CopError::InvalidRequest`。
- `intoSortedRows` 会传播 `topNSorter.err`，但当前 Rust 的 `compare_rows` 使用全序 `Datum::cmp`，没有写入该字段的路径；因此该错误分支目前是从 Go 结构保留下来的休眠兼容形状。
- 容量零由 `tryToAddRow` 显式处理，不会发生 `rows[0]` 越界。
- 键长度不足不会返回 `CopError`；`get(index)` 产生的 `None` 会按 `Option` 的次序参加比较。正常执行器入口不会产生这种形状，但直接构造这些 crate 内公开字段时必须维护键数不变量。
- `Pop` 始终返回 `None`。新增代码若需要弹出元素，必须先实现并验证向量长度、`heapSize` 与堆序的同步，不能依赖现状。
- `Datum::Real` 使用 `f64::total_cmp`，所以包括 NaN 在内也有确定的全序；它并不等同于 Go `types.Datum.Compare` 的完整 SQL 类型与上下文语义。

## 并发与资源生命周期

本文件没有锁、原子变量、通道、任务或线程，也没有 `unsafe`。所有变更都要求 `&mut self`，对象由单个 `topNExec` 独占；代码本身没有提供并发共享协议。

候选行及其键的所有权保存在堆的 `Vec<sortRow>` 中。被拒绝或被替换的行在调用结束时释放；被接纳的行一直保留到上游耗尽。`intoSortedRows(self)` 消费堆，排序后消费每个 `sortRow`，释放键并把 `Row` 所有权交给执行器输出队列。峰值候选内存与 N 及行/键大小成正比；与 Go 实现临时追加第 N+1 行再裁剪不同，Rust 满堆路径直接比较并覆盖根，不会为候选向量临时扩容一个元素。

## 与 Go 版本的对应关系

直接对照文件是 [`topn.go`](topn.go)，执行器配合逻辑分别对应 [`executor.go`](executor.go) 的 `topNExec` 和 [`cop_handler_dag.go`](cop_handler_dag.go) 的 `buildTopN`。

- `sortRow`、`topNSorter`、`topNHeap`、`Len`、`Swap`、`Less`、`Push`、`Pop`、`tryToAddRow` 均保留 Go 命名与总体职责。两版都以反向 `Less` 让堆顶成为最差候选，并只让严格更优的新行替换根。
- Go 使用 `container/heap` 的 `Push`/`Fix`；Rust 用 `sift_up`/`sift_down` 内建相同行为。Go 满堆时临时把新行追加到 `rows[heapSize]` 再比较、交换并裁剪；Rust 先与根比较，命中后直接覆盖根。
- Go 的值比较调用 `types.Datum.Compare`，传入 StatementContext 的类型上下文和由字段 collation 构造的 collator；比较失败会写入 `topNSorter.err`，并由 `evalTopN` 返回。Rust 使用简化 `Datum::Ord`，没有 StatementContext/collation 参与，也没有当前可触发的比较错误。这是明确的迁移语义差距，不能把 Rust 实现描述成完整复刻。
- Go 的 `topNExec` 在输入耗尽时直接 `sort.Sort(&heap.topNSorter)` 并用游标输出；Rust 的 `intoSortedRows` 消费堆，返回行向量，再由 `VecDeque::pop_front` 输出。
- Go 仅在新行被接纳后复制原始 `data`；Rust 的 `sortRow` 在尝试接纳前已拥有 `Row`，拒绝时直接丢弃所有权，外部可见结果一致。

## 扩展指南

- 扩展 SQL 排序语义（collation、更多 Datum 类型、NULL 排序选项）时，应优先集中修改 `copr_handler.rs::Datum::cmp` 或为 TopN 引入可返回错误的上下文比较，并同步审视 `compare_rows`、`topNSorter.err` 与 `topNExec::evalTopN` 的错误传播。主要兼容风险是与 Go 的类型上下文/collation 结果不一致。
- 修改堆算法时，必须保持“正常比较判定优劣、反向比较维护最差根”这一方向关系，并覆盖容量 0、容量 1、未满、满堆替换、满堆拒绝、升序、降序、多键和同键场景。方向写反会静默返回 BottomN，是最高正确性风险。
- 若实现 `Pop`，应把测试放在独立 Rust 测试文件（例如同目录现有测试组织方式中的 `topn_test.rs`，并由 `lib.rs` 的 `#[cfg(test)]` 模块声明接入），不要把测试内嵌到 `topn.rs`；还需验证 `heapSize`、`rows.len()` 与堆序同步。
- 当前 `rg` 未在 `pkg/store/mockstore/mockcopr/*test*.rs` 或 `*_test.go` 中发现直接针对 TopN/这些堆符号的测试。扩展本文件时应新增独立回归测试，并通过 `topNExec` 或 DAG 路径补一组集成式断言，覆盖表达式求值错误以及最终逐行输出。
- 性能修改应保留 `O(N)` 候选内存边界，避免比较阶段克隆整行或重复求值表达式；大 N 时还应关注最终稳定排序和 `VecDeque` 转换的额外内存。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/store/mockstore/mockcopr/topn.rs` 确认目标已索引且有 20 个符号；`node --file .../topn.rs` 阅读完整 195 行；对 `topNHeap`、`topNSorter`、`compare_rows` 执行了 `query`/`callers`/`callees`。方法级 callers 缺失后，以精确 `rg` 引用搜索补齐调用证据。
- 目标实现：`topn.rs:27-195`，核对行/键结构、普通与反向比较、有界堆、容量零、最终排序、错误占位和资源所有权。
- Rust 入口与类型：`lib.rs:18-29`（模块装配）；`copr_handler.rs:27-131, 418-470`（`CopError`、`Datum::Ord`、`ByItem`、`ExecutorSpec::TopN`）；`cop_handler_dag.rs:230-267, 322-325`（DAG 构建）；`executor.rs:319-415`（键求值、堆生命周期与输出）。
- crate 边界：`Cargo.toml` 的 `[lib]`、`package.metadata.porting`、dependencies 与 dev-dependencies。
- Go 对照：`topn.go:27-141`；`executor.go:455-553`；`cop_handler_dag.go:405-437`。
- 测试搜索：对本目录 `*test*.rs` 与 `*_test.go` 搜索 `TopN`、`topNHeap`、`topNSorter`、`tryToAddRow`、`ExecutorSpec::TopN` 等符号，未发现直接覆盖；因此本文没有声称已有 TopN 回归测试。
- 本任务是纯文档分析，按计划不运行 Cargo。交付验证使用任务指定的 11 章节结构检查，并补充 `git diff --check` 与目标范围差异复核。
