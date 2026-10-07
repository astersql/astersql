# `pkg/infoschema/issyncer/deferfn.rs`

## 文件定位

本文件属于 `astersql-infoschema-issyncer` crate；crate 入口是同目录的 `lib.rs`，其中以 `mod deferfn;` 装入模块并通过 `pub use deferfn::*;` 重新导出。文件只依赖标准库的 `std::sync::Mutex` 与单调时钟 `std::time::Instant`，没有使用 `Cargo.toml` 中列出的其他内部或外部 crate。

它实现一个由调用者主动轮询的延迟回调队列。当前 Rust 应用接线中，`syncer.rs::newSyncer` 创建共享的 `Arc<DeferFn>`，`Syncer::SyncLoop` 周期性调用 `check`。但是 `loader.rs::newLoader` 的 `_deferFn` 参数当前未被保存，生产 Rust 代码也没有调用 `add`；所以现状是“调度器和轮询端存在、测试覆盖其行为，但 Go 的 InfoSchema V1/V2 切换延迟释放入队链尚未接通”，不能把它描述成已经完成生产清理。

## 核心职责

- `deferFn::add` 把一个只能执行一次、可在线程间转移且为 `'static` 的闭包，连同绝对触发时刻追加到队尾。
- `deferFn::check` 在一次检查开始时获取当前 `Instant`，随后在持锁状态下按登记顺序执行所有已严格到期的回调，并把未到期记录按原相对顺序放回队列。
- `deferFn::len` 与 `deferFn::is_empty` 提供只读队列状态，主要用于测试和诊断。
- 该类型不创建计时器或后台任务；回调是否及时运行完全取决于外部是否调用 `check`。当前生产轮询入口是 `syncer.rs::Syncer::SyncLoop`。

## 主要符号

- `deferFnRecord { fire: Instant, callback: Box<dyn FnOnce() + Send> }`：私有单条记录。`FnOnce` 表示回调消费后不可再次调用，`Send` 允许记录随队列跨线程共享；记录本身不公开。
- `deferFn { records: Mutex<Vec<deferFnRecord>> }`：实际队列类型。`Default` 创建空向量和未锁定互斥量；小写命名是为了贴近 Go 的 `deferFn`。
- `DeferFn = deferFn`：公开别名，供 `lib.rs` 重导出以及 `syncer.rs`、`loader.rs`、`deferfn_test.rs` 使用。
- `add<F: FnOnce() + Send + 'static>(&self, callback: F, fire: Instant)`：公开登记入口。`&self` 配合内部互斥允许多个线程共享登记。
- `check(&self)`：公开消费入口。一次检查只使用函数开头捕获的 `now`，不会在每条记录前重新读时钟。
- `len(&self) -> usize`、`is_empty(&self) -> bool`：分别返回持锁时的待处理数和空状态；两次独立查询之间队列仍可能被其他线程改变。

## 执行流程

1. 调用者用 `add` 登记回调；该方法取得 `records` 锁并将记录追加到 `Vec` 末尾，因此单个临界区内保持追加顺序。
2. 外部轮询器调用 `check`。函数先记录 `Instant::now()`，再建立空的 `pending` 向量并取得队列锁。
3. `records.drain(..)` 暂时取走全部记录，按原向量顺序逐条处理。
4. 若 `record.fire < now`，立即在锁内消费并调用 `FnOnce`；若不满足，则把记录推入 `pending`。因此 `fire == now` 不算到期，要等后续检查。
5. 遍历成功结束后，用 `pending` 替换已排空的原向量。已执行记录被删除，未到期记录保持原相对顺序。
6. `syncer.rs::SyncLoop` 在其周期点调用 `self.deferFn.check()`；不过当前 Rust Loader 不会向同一对象登记生产回调，所以除非未来补齐接线，该周期检查通常处理空队列。

## 数据与状态

唯一持久状态是 `Mutex<Vec<deferFnRecord>>`。没有按时间排序、堆结构、ID、取消句柄或重复执行标记；每次 `check` 都线性扫描当时的全部记录，时间复杂度为 O(n)，并另建最多 O(n) 的 `pending` 向量。

`Instant` 表示进程内单调时间点，适合计算延迟，不表示墙上时钟，也不能序列化或跨进程恢复。队列只存在于内存中，进程退出即丢失。回调由 `Box` 独占；到期时执行一次，未到期时继续由队列独占。

## 依赖与调用关系

- 模块边界：`lib.rs` 声明并重导出本模块；`Cargo.toml` 指定该 crate 的入口为 `lib.rs`，本文件没有 feature 或条件编译分支。
- 上游构造：`syncer.rs::newSyncer` 调用 `DeferFn::default()` 并放入 `Arc`，把同一对象保存到 `Syncer.deferFn`，普通 Loader 构造时也传入一个克隆。
- 上游消费：`syncer.rs::Syncer::SyncLoop` 周期性调用 `self.deferFn.check()`。
- 当前缺失的生产登记边：`loader.rs::newLoader` 接收 `_deferFn: Option<Arc<crate::DeferFn>>`，但 `Loader` 没有对应字段，也没有 `add` 调用。RustCodeGraph/仓库检索到的 `add` 调用均位于 `deferfn_test.rs`。
- Go 完整链路：`loader.go::Loader` 保存 `*deferFn`；完整加载发生 V1/V2 切换且 `schemaTs > 0` 时，`LoadWithTS` 获取 `infoCache.Upsert` 返回的释放闭包并登记为十分钟后执行；`syncer.go::SyncLoop` 在 lease ticker 分支调用 `check`。

## 错误处理与边界

本 API 不返回 `Result`。所有锁获取都使用 `lock().unwrap()`：若某次持锁执行期间发生 panic，互斥量会被毒化，后续 `add`、`check`、`len` 或 `is_empty` 会再次 panic。

回调就在持锁且原队列已被 `drain` 的过程中运行。若回调 panic，本次 `check` 不会执行最后的 `*records = pending`；已移入 `pending` 的未到期记录以及尚未迭代的 drain 项会随栈展开被丢弃，队列还会进入毒化状态。因此回调必须避免 panic。

回调若同步重入同一个实例的 `add`、`check`、`len` 或 `is_empty`，会等待当前线程已经持有的非重入 `Mutex`，造成死锁。耗时或阻塞回调也会阻塞所有并发登记和检查。文件没有取消、去重、容量限制或公平性保证；未来扩展不能默认为这些能力已经存在。

## 并发与资源生命周期

互斥量把登记、检查和状态读取串行化。`deferfn_test.rs::test_defer_fn` 用八个线程并发登记并验证记录没有丢失；编译期 `assert_sync::<DeferFn>()` 证明该类型可共享。`check_holds_the_lock_while_running_callbacks` 进一步验证回调阻塞期间，并发 `add` 不能完成，这正是与 Go 版本一致的锁生命周期。

锁从 `check` 取得 `records` 开始，一直持有到所有到期回调执行完、未到期记录写回为止。这样避免检查中途改变队列，但也把回调行为纳入关键区。多个线程同时调用 `check` 时会串行运行；后取得锁的一次仍使用它在等待锁之前捕获的 `now`，因此可能暂不执行等待期间才到期的记录，须等下一次检查。

## 与 Go 版本的对应关系

Rust 的 `deferFnRecord` 对应 `deferfn.go::deferFnRecord`，`records` 对应 Go 的 `data`，`callback` 对应 `fn`；`add`、`check` 的追加、严格到期判断、原序保留和锁内执行语义保持一致。Go 使用 `now.After(record.fire)`，Rust 使用 `record.fire < now`，两者都排除相等时刻。

Rust 用 `Box<dyn FnOnce() + Send>` 表达一次性回调，并要求捕获值为 `'static`；Go 的 `func()` 可直接闭包引用测试栈变量。因此 Rust 测试用 `Arc<AtomicBool>` 代替 Go 测试中的局部 `bool`。Rust 额外提供 `len`、`is_empty` 及公开别名 `DeferFn`，因为私有 `records` 不能像同包 Go 测试那样直接读取。

重要迁移差异在调用链而非队列算法：Go Loader 在 V1/V2 切换时真实调用 `add`，Rust Loader 当前忽略 `_deferFn`。所以 Rust 文件顶部所说“用于推迟清理或通知”是设计用途，当前生产可达事实仅确认了构造和周期 `check`，未确认清理回调的生产登记。

## 扩展指南

- 补齐 V1/V2 延迟释放时，应优先在 `loader.rs::Loader` 保存与 `Syncer` 相同的 `Arc<DeferFn>`，并在与 `loader.go::LoadWithTS` 对应的成功切换分支登记 `InfoCache::Upsert` 返回的释放动作；同时确认跨 keyspace 构造路径是否需要独立队列。
- 若要改变到期边界、回调执行顺序或锁外执行，必须同步评估 Go 兼容性。尤其锁外执行会改变 `check_holds_the_lock_while_running_callbacks` 明确锁定的并发契约，不能作为无行为变化的优化。
- 若要支持取消、按触发时间高效选择或大量记录，应新增明确的数据结构/API，而不是把当前 `Vec` 的插入顺序误当成按时间排序。性能改变需要覆盖相同触发时刻、乱序登记和大队列。
- 任何逻辑改动都应更新独立文件 `deferfn_test.rs`，不要把测试内嵌回生产源文件；至少覆盖严格相等边界、回调顺序、并发登记、锁持有以及 panic 策略。若补生产接线，还应在 Loader/Syncer 的独立测试中证明真实回调从入队到周期消费的完整链路。
- 不应让回调重入同一个 `DeferFn`，除非实现同时改为显式支持重入并定义 panic 后的恢复/记录保留策略。

## 验证依据

- RustCodeGraph：`query DeferFn` / `query deferFn` 定位 `deferFn`、`deferFnRecord`、`add`、`check`、`len`、`is_empty`、Go 对照及测试；`explore "pkg/infoschema/issyncer/deferfn.rs DeferFn"` 返回目标源码、Go 源码和两侧测试，并显示 Rust 测试调用关系。
- Rust 源与装配：`pkg/infoschema/issyncer/deferfn.rs`、`pkg/infoschema/issyncer/lib.rs`、`pkg/infoschema/issyncer/syncer.rs`、`pkg/infoschema/issyncer/loader.rs`。
- crate 边界：`pkg/infoschema/issyncer/Cargo.toml`，包名为 `astersql-infoschema-issyncer`，`[lib] path = "lib.rs"`。
- Rust 独立测试：`pkg/infoschema/issyncer/deferfn_test.rs`，覆盖短/长延迟筛选、剩余数量、并发登记、`Sync` 性质以及回调期间持锁。
- Go 对照：`pkg/infoschema/issyncer/deferfn.go`、`deferfn_test.go`、`loader.go`、`syncer.go`，分别验证队列算法、原始测试意图、V1/V2 切换登记和周期消费。
- 仓库检索：对 `pkg/infoschema/issyncer` 中 `DeferFn`、`deferFn`、`add`、`check` 的引用核对，确认当前 Rust 生产代码只有构造/传递/消费，没有生产登记调用。

