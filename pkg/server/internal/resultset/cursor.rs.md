# `pkg/server/internal/resultset/cursor.rs`

[查看对应 Rust 源文件](./cursor.rs)

## 文件定位

本文件属于 `astersql-server-internal-resultset` crate。crate 根模块 `pkg/server/internal/resultset/lib.rs` 将本文件的公开项全部再导出；`pkg/server/internal/resultset/Cargo.toml` 的 `package.metadata.porting.go-package` 指向同路径 Go 包，表明它是 `pkg/server/internal/resultset/cursor.go` 的 Rust 移植。它位于服务端结果集和逐行游标协议之间：底层 `ResultSet` 按 chunk 产出记录，本文件把它适配成 `RowIterator`，供服务端游标按行执行 `Current`、`Next`、`End`、`Error` 和 `Close`。

当前 Rust 生产入口见 `pkg/server/protocol_result.rs::WorkerResult::cursor`：第一次需要逐行访问时用 `WrapWithLazyCursor` 包装普通结果集；`WorkerResults::operate` 随后把 `Operation::Current`、`Operation::Advance`、`Operation::Close` 和 `Operation::FetchReturned` 映射到游标或结果集方法。`WrapWithRowContainerCursor` 也是公开 API，但仓库 Rust 生产代码中未检索到该函数的调用者；它目前是给已经物化到 `RowContainerReader` 的结果提供相同接口的适配入口。

## 核心职责

- `RowIterator` 定义协议侧所需的逐行迭代契约，同时允许 `Next`、`Current` 接收 `sqlexec::context::Context`，这正是它与底层 `RowContainerReader` 接口的主要差异。
- `WrapWithRowContainerCursor` / `TidbCursorResultSet` / `RowContainerReaderIter` 将已经物化的行容器与原始 `ResultSet` 组合为一个 `CursorResultSet`；逐行读取来自行容器，其余结果集操作仍落到原始结果集。
- `WrapWithLazyCursor` / `TidbLazyCursorResultSet` / `LazyRowIterator` 在需要下一行时才调用底层 `ResultSet::Next` 填充可复用 chunk，避免预先物化全部结果。
- `impl_result_set_forwarder!` 为两种包装器完整转发 `ResultSet` 接口，保留列信息、chunk 分配、关闭、分离、预编译语句、fetch 通知和 RU v2 追踪语义。
- `FetchNotifier` 描述 fetch 返回后的通知形状；本文件没有为其提供实现或直接调用者。实际生产链使用 `ResultSet::OnFetchReturned`，并由转发宏交给底层结果集。

## 主要符号

- `RowIteratorError = Arc<dyn Error + Send + Sync>`：迭代错误的共享所有权类型。`Arc` 让 `Error()` 可以克隆返回错误句柄；错误对象本身要求 `Send + Sync`。
- `SharedResultSet = Rc<RefCell<Box<dyn ResultSet>>>`：包装器和迭代器共享同一个动态结果集。`Rc`/`RefCell` 明确把共享范围限制在单线程，并将独占可变借用检查放到运行时。
- `RowIterator`：公开 trait。`Next` 前进一步，`Current` 返回当前位置，`End` 构造 EOF 哨兵，`Error` 暴露已捕获错误，`Close` 释放底层资源。
- `CursorResultSet: ResultSet`：在完整 `ResultSet` 能力上增加 `GetRowIterator`；协议层因此可以用同一对象做普通 chunk 操作或逐行游标操作。
- `TidbCursorResultSet`：持有 `result_set` 和独立的 `RowContainerReaderIter`。其逐行读取不调用 `result_set.Next`，但宏转发的 `ResultSet` 方法仍作用于 `result_set`。
- `RowContainerReaderIter`：对 `chunk::row_container_reader::RowContainerReader` 的薄适配器；`Next`、`Current`、`End`、`Close` 直接委托，`Error` 把 reader 错误装入 `Arc`。
- `TidbLazyCursorResultSet`：同时持有共享结果集和 `LazyRowIterator`；`GetRowIterator` 返回内部迭代器的可变 trait 引用。
- `LazyRowIterator`：保存 `error`、复用的 `RecordChunk`、`index_in_chunk` 和 `started`。这是 lazy 路径的状态机主体。
- `WrapWithRowContainerCursor` 与 `WrapWithLazyCursor`：两个公开构造入口，均返回 `Box<dyn CursorResultSet>`；后者先读取 `FieldTypes()`，再以 `capacity` 和 `max_chunk_size` 创建缓冲 chunk。
- `impl_result_set_forwarder!`：内部宏，仅实例化给 `TidbCursorResultSet` 与 `TidbLazyCursorResultSet`，避免两套包装器的 `ResultSet` 行为漂移。

## 执行流程

lazy 路径的完整流程如下：

1. `pkg/server/protocol_result.rs::WorkerResults::register` 保存一个普通 `ResultSet` 和初始/最大 chunk 大小，尚不建立游标。
2. 首次处理 `Operation::Current` 或 `Operation::Advance` 时，`WorkerResult::cursor` 调用 `WrapWithLazyCursor`。构造函数从底层 `FieldTypes()` 创建 `RecordChunk`，把同一个 `Rc<RefCell<_>>` 分别交给包装器和 `LazyRowIterator`，并初始化 `error = None`、`index_in_chunk = 0`、`started = false`。
3. 首次 `Current(ctx)` 发现 `started == false`，转调 `Next(ctx)`。`Next` 标记已启动并增加索引；初始 chunk 为空，因此进入补充路径，调用底层 `ResultSet::Next(ctx, &mut chunk)`。
4. 若补充成功且 chunk 非空，索引重置为 0，并返回该 chunk 的第一行。后续 `Next` 在 chunk 内递增索引；索引到达 `NumRows()` 后才再次向底层取下一批。
5. 底层成功返回空 chunk 表示 EOF，迭代器返回 `End()`，即 `chunk::Row::default()`。`protocol_result.rs::WorkerResults::operate` 用 `row == iter.End()` 把该哨兵转换成 `None`。
6. 底层 `Next` 失败时，错误被保存到 `error` 并返回 EOF 哨兵；协议层紧接着调用 `Error()`，若存在错误则转换成 `ConnError::Session`，从而区分正常 EOF 与失败。
7. `RowIterator::Close` 关闭共享的底层结果集。协议层的显式 `Operation::Close` 则通过包装器的 `ResultSet::Close` 转发到同一个底层对象。

row-container 路径更直接：`WrapWithRowContainerCursor` 保存原始结果集用于通用 `ResultSet` 方法，同时用 `RowContainerReaderIter` 对外提供逐行读取；上下文参数在该适配器中不使用，因为行已由 reader 管理。

## 数据与状态

`LazyRowIterator` 的核心不变量是：当存在可返回行时，`index_in_chunk < chunk.NumRows()`；跨批次时，成功填充后必须把索引重置为 0。`started` 只区分“尚未定位”与“已经调用过 Next”，保证第一次 `Current` 取得第一行而不是读取尚未初始化的位置。索引使用 `saturating_add(1)`，避免 `usize` 溢出回绕。

`RecordChunk` 在整个迭代生命周期内复用；本文件不把所有行收集进自身容器，因此 lazy 路径的额外行存储上界由 `capacity`、`max_chunk_size` 和底层 `Next` 的填充行为决定。返回的 `chunk::Row` 是从当前 chunk 取得的行视图；调用方不应假定它在后续 refill 后仍拥有独立存储。

`error` 初始为空，仅在底层 `ResultSet::Next` 返回错误时写入；`Error()` 克隆 `Arc`，不会清除错误。实现没有单独的 `exhausted` 标志，因此 EOF 后再次调用 `Next` 仍会尝试向底层请求空批；调用方应在收到 `End()` 或错误后停止推进。

两种包装器都把原始结果集放进 `SharedResultSet`。lazy 迭代器与包装器确实共享该对象；row-container 迭代器则拥有独立 reader，原始结果集只服务于包装器的转发方法。

## 依赖与调用关系

上游直接证据：

- `pkg/server/protocol_result.rs::WorkerResult::cursor` 调用 `WrapWithLazyCursor`；`WorkerResults::operate` 调用 `GetRowIterator`，再根据操作选择 `Current` 或 `Next`，并读取 `Error`/`End`。
- `pkg/server/internal/resultset/resultset_aster_unit_test.rs::lazy_cursor_iterates_across_real_chunks_and_closes_result_set` 直接构造 lazy cursor，验证跨批次迭代和关闭。
- `pkg/server/tests/commontest/cursor_test.rs::lazy_cursor_fetches_across_real_chunk_boundaries` 用四组 chunk 大小验证 0..1000 的逐行顺序。

下游直接依赖：

- `astersql-util-chunk` 提供 `Row`、`Allocator`、`FieldType`、`New` 和 `RowContainerReader`。
- `astersql-util-sqlexec` 提供 `Context`、`RecordChunk`、`GoError`；lazy refill 通过 `ResultSet::Next` 使用它们。
- `astersql-server-internal-column::Info`、`PreparedStmtRef` 和 `CursorRUV2Tracker` 只出现在完整 `ResultSet` 转发签名中。
- 同 crate 的 `resultset.rs::ResultSet` 是包装器的父契约；`lib.rs` 再导出本文件符号。Cargo 清单没有 feature 条件，本文件也没有条件编译项。

RustCodeGraph 文件索引报告本文件被 `protocol_result.rs`、`resultset_aster_unit_test.rs` 等文件使用；精确函数 callers 查询未产生边，因此上述生产调用边又以仓库文本引用核验。未发现 Rust 生产代码调用 `WrapWithRowContainerCursor`，也未发现 `FetchNotifier` 的实现或使用。

## 错误处理与边界

- `LazyRowIterator::Next` 不直接返回 `Result`。底层 `GoError` 被转换为 `RowIteratorError` 保存，同时方法返回与 EOF 相同的空行；调用方必须在空行附近查询 `Error()`。当前协议实现遵守这一顺序。
- `RowContainerReaderIter::Error` 将 reader 当前错误转换为共享 trait object；其他方法保持 reader 原有行为。
- 正常结束由默认空 `Row` 表示，而非独立枚举。业务若可能合法产生“空行对象”，仍必须沿用 chunk 行的有效性约定，并用 `End()` 比较，不应自行发明第二种 EOF 表示。
- `Current` 在迭代开始前有推进副作用；开始后不推进。EOF 后当前 chunk 为空时它稳定返回 `End()`。
- `capacity`、`max_chunk_size` 的合法性由 `chunk::New` 及底层 chunk 实现负责，本文件不额外校验或改写参数。
- `RefCell::borrow_mut` 可能在同线程发生重入可变借用时 panic。现有接口通过 `&mut self` 和短生命周期借用避免正常路径重叠；扩展时不要在持有底层借用期间回调包装器。
- `Close` 没有在本文件增加幂等状态；它依赖底层 `ResultSet::Close` 的契约。当前 `TidbResultSet` 的实现是幂等的，但新的 `ResultSet` 实现也应维持这一行为。

## 并发与资源生命周期

`SharedResultSet` 使用 `Rc<RefCell<_>>` 而不是 `Arc<Mutex<_>>`，所以这些游标包装器不是跨线程共享的数据结构。并发边界由协议层把操作串行发送给持有 `WorkerResults` 的工作线程来保证；`RowIteratorError` 使用 `Arc` 只是在错误句柄层面允许安全共享，不会使游标本身变成线程安全。

资源所有权从 `Box<dyn ResultSet>` 转移给包装器。lazy 路径中，包装器和迭代器各持有一个 `Rc`，但最终都指向同一结果集；关闭任一接口都会调用同一底层 `Close`。本文件没有为包装器实现 `Drop`，因此确定性释放依赖调用方执行 `Close`，或依赖具体底层对象自身的析构行为。`protocol_result.rs` 在 `Operation::Close` 时从结果表移除对象并调用 `Close`；其 `ResultHandle` 的关闭请求也覆盖正常生命周期结束。

chunk 被 `LazyRowIterator` 独占并跨 fetch 复用；没有后台任务、通道、锁或事务在本文件中创建。RU v2 tracker 由底层结果集拥有，本文件只通过转发宏传递 `SetCursorRUV2Tracker` 和 `ReportCursorRUV2Delta`，不改变其锁或计量生命周期。

## 与 Go 版本的对应关系

`pkg/server/internal/resultset/cursor.go` 是逐项对照来源。Rust 保留了 Go 的两类包装器、`RowIterator`/`CursorResultSet` 接口、lazy 状态字段、首次 `Current -> Next`、chunk 耗尽时 refill、空 chunk 表示 EOF、错误旁路查询以及 `Close` 委托。

主要语言映射和差异如下：

- Go 通过匿名嵌入 `ResultSet` 自动转发方法；Rust 用 `impl_result_set_forwarder!` 显式覆盖全部 `ResultSet` 方法。新增 `ResultSet` 方法时必须同步宏，否则两个包装器会在编译期缺失实现。
- Go 的 `cursorRUV2Trackable` 是可选类型断言；Rust 的 `ResultSet` trait 已把 RU v2 方法纳入必需接口，因此包装器无条件转发，底层不需要功能时提供空实现。
- Go 用接口引用共享底层结果集；Rust 用 `Rc<RefCell<Box<dyn ResultSet>>>` 表达共享所有权和内部可变性，因此多了一项运行时借用约束。
- Go 保存普通 `error`；Rust 保存 `Arc<dyn Error + Send + Sync>`，以适配 `Error()` 的借用外返回和共享需求。
- Rust 索引递增使用 `saturating_add`，Go 使用普通 `int` 加一；对正常 chunk 尺寸行为一致，Rust 额外避免整数回绕。
- Go 的 `rowContainerReaderIter` 通过嵌入 reader 继承 `End`、`Error`、`Close`；Rust 明确逐方法委托并转换错误类型。

Go 测试 `pkg/server/tests/commontest/cursor_test.go::TestLazyRowIterator` 与 Rust 测试 `pkg/server/tests/commontest/cursor_test.rs::lazy_cursor_fetches_across_real_chunk_boundaries` 使用相同四组 chunk 配置并检查 1000 行顺序和 EOF，构成直接的移植语义证据。Go 的 `pkg/server/conn_stmt_test.go` 还验证 prepared-statement cursor 的 `Current`/`Next`/`End` 与 `COM_STMT_FETCH` 场景；本次只读分析未发现完全同形的 Rust 协议级 prepared-statement 测试。

## 扩展指南

- 修改 lazy 推进或 EOF 语义时，首要修改点是 `LazyRowIterator::{Next, Current, End, Error}`。应同步扩展独立测试 `pkg/server/tests/commontest/cursor_test.rs`，覆盖首次 `Current`、跨 chunk、最后一行、重复 EOF、底层 `Next` 错误和错误后的停止策略；不要把测试内嵌到 `cursor.rs`。
- 修改已物化游标时，应在 `RowContainerReaderIter` 和 `WrapWithRowContainerCursor` 接入，并新增同目录之外的独立测试。当前现有 Rust 测试没有直接覆盖该包装器，这是扩展该路径时的首要测试缺口。
- 给 `ResultSet` 增加方法时，必须同步 `impl_result_set_forwarder!` 的转发、所有底层实现和测试替身；特别检查 `OnFetchReturned`、`SetCursorRUV2Tracker`、`ReportCursorRUV2Delta` 等协议副作用没有被包装器吞掉。
- 若要支持跨线程持有游标，不能只给某个类型增加 `Send`：需要整体重审 `Rc<RefCell<_>>`、动态 trait 边界、协议线程模型和 chunk 行视图生命周期。直接替换为锁还会引入重入、阻塞和性能风险。
- 若要缓存或异步保存 `chunk::Row`，应先转成拥有自身数据的表示；当前 lazy chunk 会复用，长期持有行视图有数据失效风险。
- 保持 Go 行为对齐：生产修改应同时核对 `cursor.go`、Go `TestLazyRowIterator` 和 prepared-statement cursor 测试。兼容风险集中在首次 `Current` 的副作用、EOF/错误区分、关闭次数和 fetch/RU 通知；性能风险集中在 chunk 分配、refill 频率与不必要的行复制。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/server/internal/resultset` 确认 `cursor.rs`、`cursor.go`、crate 入口和独立测试均已索引；`node --file pkg/server/internal/resultset/cursor.rs` 阅读了 1–275 行全部实现；`query` 确认 `CursorResultSet`、`WrapWithLazyCursor`、`WrapWithRowContainerCursor`、`LazyRowIterator` 的 Rust/Go 定义。精确 callers 查询为空这一索引限制已用直接文本引用补证，没有据此虚构调用边。
- 源码与入口：`pkg/server/internal/resultset/cursor.rs`、`pkg/server/internal/resultset/resultset.rs`、`pkg/server/internal/resultset/lib.rs`、`pkg/server/protocol_result.rs`。
- crate 边界：`pkg/server/internal/resultset/Cargo.toml`，确认 crate 名、根文件、Go 包映射和直接依赖；未声明 feature。
- Go 对照：`pkg/server/internal/resultset/cursor.go`、`pkg/server/tests/commontest/cursor_test.go::TestLazyRowIterator`、`pkg/server/conn_stmt_test.go` 的 prepared-statement cursor 片段。
- Rust 测试：`pkg/server/internal/resultset/resultset_aster_unit_test.rs::lazy_cursor_iterates_across_real_chunks_and_closes_result_set`；`pkg/server/tests/commontest/cursor_test.rs::lazy_cursor_fetches_across_real_chunk_boundaries`。前者验证多批次与一次关闭，后者验证四种 chunk 配置、首次/重复 `Current`、顺序、EOF、无错误和关闭。未运行 Cargo，符合本任务的纯文档约束。
- 结构验证使用任务指定命令，要求目标文件存在且恰有 11 个固定二级标题；交付前另行人工复核所有关键结论均可回指以上符号或文件，并确认没有建议把 Rust 测试写入生产源文件。
