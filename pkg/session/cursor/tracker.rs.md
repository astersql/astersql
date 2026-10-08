# `pkg/session/cursor/tracker.rs`

## 文件定位

本文件属于独立 crate `astersql-session-cursor`；其 crate 根 `pkg/session/cursor/lib.rs` 公开 `state`、`tracker` 两个模块并再导出其中符号，`pkg/session/cursor/Cargo.toml` 用 `go-package = "pkg/session/cursor"` 标明对应的 Go 包。文件提供会话内服务端游标的注册表和句柄生命周期基础能力，不负责 SQL 执行、结果集读取或 MySQL 协议编码。

当前 Rust 生产接线是不完整的：`pkg/session/sessmgr/processinfo.rs::ProcessInfo::CursorTracker` 可以保存 `Arc<CursorTracker>`，`pkg/sessionctx/context.rs::Context::GetCursorTracker` 也声明了关联类型返回边界，但仓库内 Rust 生产代码尚未调用本文件的 `Tracker::NewCursor`、`GetCursor` 或 `RangeCursor`。完整的上层用途目前由 Go `pkg/session/session.go::execStmtResult.TryDetach` 展示：为可分离结果集创建游标、保存事务 `StartTS`，再交给 `staticrecordset.WrapRecordSetWithCursor` 管理。

## 核心职责

- `Tracker` 定义创建、按 ID 查询、遍历游标的协议。
- `CursorTracker` 用线程安全映射保存活动句柄，并用原子计数器分配 ID。
- `CursorHandle` 保存不可变的 ID 和 `State`，通过弱引用在 `Close` 时从所属跟踪器注销。
- `Handle = Arc<CursorHandle>` 让创建者、注册表和调用者共享同一个句柄实例；查询不会复制游标状态对象。

它刻意只做“登记与注销”。`Close` 不关闭底层结果集，也不修改 `State`；这些资源动作必须由持有句柄的更上层组件协调。

## 主要符号

- `pub trait Tracker`（`tracker.rs:29`）：公开 `NewCursor(self: &Arc<Self>, state: State) -> Handle`、`GetCursor(&self, id: i64) -> Option<Handle>` 和泛型 `RangeCursor<F>`。由于 `RangeCursor` 是泛型方法，该 trait 不适合作为 `dyn Tracker` 使用；当前构造器直接返回具体类型。
- `pub struct CursorTracker`（`tracker.rs:44`）：包含 `cursors: Mutex<HashMap<i64, Handle>>` 与 `id_alloc: AtomicI64`。两个字段均私有，调用者只能经接口维护不变量。
- `pub fn NewTracker() -> Arc<CursorTracker>`（`tracker.rs:55`）：构造空映射，ID 分配器从 0 开始。
- `impl Tracker for CursorTracker`（`tracker.rs:62`）：给出创建、查询和遍历的实际实现。
- `CursorTracker::remove`（`tracker.rs:110`）：私有注销入口，仅供同文件的 `CursorHandle::Close` 使用。
- `CursorTracker::set_id_alloc_for_test`（`tracker.rs:118`）：仅在 `cfg(test)` 下存在，用于验证 `i64` 回绕，不属于生产 API。
- `pub type Handle = Arc<CursorHandle>`（`tracker.rs:126`）：共享句柄别名。
- `pub struct CursorHandle`（`tracker.rs:132`）：保存 `id`、按值保存的 `State` 以及 `Weak<CursorTracker>`。
- `CursorHandle::{ID, Close, GetState}`（`tracker.rs:145-163`）：分别读取 ID、注销自身和按值返回状态。

## 执行流程

1. `NewTracker` 创建 `Arc<CursorTracker>`，映射为空、`id_alloc` 为 0。
2. `Tracker::NewCursor` 以 `SeqCst` 执行 `fetch_add(1)`；由于返回旧值，再通过 `wrapping_add(1)` 得到新 ID，因此正常序列从 1 开始，越过 `i64::MAX` 时回绕到 `i64::MIN`。
3. 创建 `Arc<CursorHandle>`。句柄按值保存传入的 `State`，并用 `Arc::downgrade(self)` 记录所属跟踪器，避免跟踪器与句柄形成强引用环。
4. 锁定 `cursors`，以 ID 为键插入句柄克隆，然后把原句柄返回调用者。
5. `GetCursor` 锁定映射，命中时克隆 `Arc`，缺失时返回 `None`。
6. `RangeCursor` 在锁内克隆当前全部句柄到 `Vec`，随即释放锁；随后按该快照逐个调用回调，回调返回 `false` 即停止。
7. `CursorHandle::Close` 尝试升级弱引用；跟踪器仍存在时调用 `remove(id)`，否则直接结束。删除不存在的键不会报错，所以重复关闭幂等。

## 数据与状态

`cursors` 是活动游标的唯一注册表，键和 `CursorHandle::id` 必须一致。映射持有每个活动句柄的一份强引用，因此调用者丢弃自己的 `Arc` 并不会自动注销游标；必须显式调用 `Close`，或整体释放跟踪器。

`id_alloc` 与映射锁相互独立：ID 分配无需占用映射锁，插入和删除则由同一个互斥锁串行化。`State` 定义在 `pkg/session/cursor/state.rs`，当前只有 `StartTS: u64`，并实现 `Copy`；`GetState` 因而返回创建时快照的值副本。

ID 回绕是有意对齐 Go `atomic.Int64.Add` 后转整数的行为。极端情况下如果计数器绕回一个仍在使用的 ID，`HashMap::insert` 会替换旧注册项；旧句柄随后 `Close` 也可能删除同 ID 的新项。现有实现和测试只验证算术回绕，不提供跨完整 `i64` 周期的碰撞保护。

## 依赖与调用关系

下游依赖只有标准库和同 crate 类型：`super::state::State`、`HashMap`、`AtomicI64`、`Arc`、`Mutex`、`Weak`。`pkg/session/cursor/Cargo.toml` 没有声明第三方依赖，crate 根 `lib.rs` 公开再导出 `tracker` 的符号。

RustCodeGraph 对 `tracker.rs::NewTracker` 的调用边显示，直接调用主要来自 `pkg/session/cursor/tracker_test.rs` 和 `migration_aster_unit_test.rs`。图中同名 `NewTracker` 还大量存在于内存跟踪器等模块，判断调用边时必须使用文件限定，不能把它们当作游标调用者。

当前可确认的 Rust 上游边界是：`pkg/session/sessmgr/processinfo.rs::ProcessInfo` 暴露可选的 `Arc<cursor::CursorTracker>`，其 `Clone` 会浅克隆该 `Arc`；`pkg/sessionctx/context.rs::Context` 通过关联类型声明 `GetCursorTracker`。仓库搜索未发现 Rust 生产代码实际调用本文件的三个 `Tracker` 方法，因此不能声称 Rust 服务端协议主链已经使用该注册表。Go 对应主链则是 `execStmtResult.TryDetach -> GetCursorTracker -> NewCursor -> WrapRecordSetWithCursor`。

## 错误处理与边界

公开操作不返回 `Result`。查无游标通过 `Option::None` 表示；关闭已经关闭的句柄、或在跟踪器已经释放后关闭句柄，均静默成功。

所有映射加锁都使用 `expect("cursor map poisoned")`，因此持锁线程 panic 导致锁中毒后，后续创建、查询、遍历或删除会继续 panic，而不是恢复或返回错误。回调在锁释放之后运行，回调自身 panic 不会毒化 `cursors` 的互斥锁。

`RangeCursor` 遍历的是取样时的 `Arc` 快照，不保证与随后发生的创建/关闭同步；快照中的句柄即使已被别的线程注销，仍可能收到本次回调。`HashMap` 迭代顺序未定义，调用者不得依赖游标顺序。回调返回 `false` 只终止本次遍历，不关闭剩余游标。

## 并发与资源生命周期

`AtomicI64` 以 `SeqCst` 保证跨线程 ID 分配具有单一全序；`Mutex<HashMap<...>>` 保护映射的所有读写。`RangeCursor` 先复制 `Arc` 再释放锁，使回调可以安全调用 `cursor.Close()`，也避免用户回调长期占锁或发生同锁重入死锁。

引用关系为“跟踪器的映射强持有句柄，句柄弱引用跟踪器”。这避免引用环：释放最后一个跟踪器 `Arc` 会连同映射一起释放注册项；外部仍持有的句柄可以继续读取 `ID`/`State`，但其 `Close` 无法再升级弱引用，因而成为无操作。相反，只释放外部句柄不会释放仍登记在映射中的游标。

`pkg/session/cursor/tracker_test.rs::TestCursorTrackerConcurrentCreateDelete` 用 100 个创建线程和 100 个遍历关闭线程覆盖并发路径；`migration_aster_unit_test.rs::concurrent_create_and_range_delete_is_safe` 提供同类迁移回归。测试证明该同步设计在压力下不 panic，但不证明公平性、吞吐量或固定遍历顺序。

## 与 Go 版本的对应关系

Go 原实现位于 `pkg/session/cursor/tracker.go`。符号一一对应：Go `Tracker`/`cursorTracker`/`Handle`/`cursorHandle` 对应 Rust `Tracker`/`CursorTracker`/`Handle`/`CursorHandle`；`NewCursor`、`GetCursor`、`RangeCursor`、`remove`、`ID`、`Close`、`GetState` 保持同名语义。

主要表示差异如下：

- Go 用 `sync.Map`，Rust 用单个 `Mutex<HashMap<...>>`；两者均支持并发创建、查询和删除，但 Rust 遍历显式形成快照，Go `sync.Map.Range` 不承诺一致快照，故并发修改时可见集合不必完全相同。
- Go `GetCursor` 缺失时返回 `nil`，Rust 返回 `None`；Go 通过接口和类型断言返回句柄，Rust 返回克隆的 `Arc`。
- Go 句柄强引用 `*cursorTracker`，Rust 使用 `Weak<CursorTracker>` 避免环；因此跟踪器先销毁时，Rust `Close` 是无操作，而 Go 的句柄会延长跟踪器生命。
- Go ID 类型是 `int`，由 `atomic.Int64` 结果转换；Rust固定为 `i64`。`TestNewCursorIDWrapsLikeGoAtomicInt64` 只对齐 64 位平台上的补码回绕语义。
- Go 测试 `tracker_test.go` 覆盖递增、同一句柄、遍历中断、关闭与并发；Rust `tracker_test.rs` 对齐这些用例并额外覆盖 ID 回绕，迁移测试还明确覆盖状态保存、缺失查询和重复关闭。

## 扩展指南

新增游标元数据时，先扩展 `pkg/session/cursor/state.rs::State`，再调整 `CursorHandle` 的保存/读取接口，并同步独立测试 `tracker_test.rs`；若是 Go 逐提交对齐，还应核对 `state.go`、`tracker.go` 及其测试，避免改变值快照语义。

新增批量关闭、统计或过滤遍历时，应优先沿用“锁内取得最小快照、锁外执行用户逻辑”的模式，不能在持有 `cursors` 锁时调用可能关闭游标或进入外部代码的回调。若改变 ID 策略，需要同时评估回绕碰撞、已有协议使用的整数宽度以及 `HashMap` 替换语义。

把该实现接入 Rust 服务端主链时，最可能修改会话的构造/关闭、`GetCursorTracker` 实现、可分离结果集包装以及 `ProcessInfo` 填充点；必须新增位于独立 `*_test.rs` 文件中的集成测试，证明创建、FETCH/关闭、会话退出清理和错误回滚都会注销游标。不要把测试内嵌到 `tracker.rs`。

性能风险主要来自单把互斥锁：高并发创建和查询会竞争同一临界区，`RangeCursor` 还会按当前游标数分配并克隆一个 `Vec`。若改成分片结构或并发映射，必须保留提前终止、回调可关闭自身和无固定遍历顺序这些契约。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `pkg/session/cursor/{lib.rs,state.rs,tracker.rs,tracker_test.rs}`；`node --file pkg/session/cursor/tracker.rs` 核对了 164 行完整源码和 21 个符号；`node NewTracker` 核对了文件限定的构造、实例化及测试调用边。
- Rust 源与模块：`pkg/session/cursor/tracker.rs`、`state.rs`、`lib.rs`；crate 元数据：`pkg/session/cursor/Cargo.toml`。
- Rust 上游边界：`pkg/session/sessmgr/processinfo.rs::ProcessInfo::CursorTracker`、`pkg/sessionctx/context.rs::Context::GetCursorTracker`。通过仓库搜索确认 Rust 生产调用尚未覆盖 `Tracker` 三个方法。
- Go 对照与真实主链：`pkg/session/cursor/tracker.go`、`tracker_test.go`、`pkg/session/session.go::execStmtResult.TryDetach` 与 `session::GetCursorTracker`。
- 独立 Rust 测试：`pkg/session/cursor/tracker_test.rs`、`pkg/session/cursor/migration_aster_unit_test.rs`，覆盖 ID、状态、同一性、缺失、遍历中断、关闭幂等和并发创建/删除。
- 按任务约束未运行 Cargo；交付只执行文档的固定十一章节结构检查，并人工复核实现能力与当前接线状态没有混写。
