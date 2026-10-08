# `pkg/util/zeropool/pool.rs`

## 文件定位

该文件是独立 crate `astersql-util-zeropool` 的对象池实现。crate 边界由 `pkg/util/zeropool/Cargo.toml` 定义，入口 `pkg/util/zeropool/lib.rs` 以 `pub mod pool` 声明本模块并通过 `pub use pool::*` 再导出 `Pool` 和 `New`。根 crate 又在 `pkg/lib.rs` 的 `util::zeropool` 门面中再导出该 crate；`benches/zeropool_standalone.rs::canonical_zeropool_is_available_through_the_root_facade` 验证了这条兼容入口。

它对应 Go 包 `pkg/util/zeropool`，目标是为 `sync.Pool` 的常见“存值会引入指针分配”问题提供类型安全、热路径尽量不分配的替代实现。当前检索到的 Rust 直接使用者是本 crate 的独立测试和根门面测试；未检索到生产 Rust 代码将表达式等业务缓冲区接到这里。尤其 `pkg/expression/core_support.rs` 仍使用会丢弃归还值的 `DropPool` 占位，因此不能把 Go 侧的广泛接线描述成 Rust 已完成的生产接线。

## 核心职责

- `Pool<T>` 维护两个受互斥锁保护的栈：`items` 保存可借出的 `Box<T>`，`pointers` 保存内容已被取走、但分配外壳可复用的 `Box<T>`（`Pool`，第 36 行）。
- `New` 保存一个线程安全的构造闭包，供 `items` 为空时创建新值（`New`，第 62 行）。
- `Get` 把值从 `Box<T>` 中移出，同时将清空后的外壳转入 `pointers`，避免外壳仍持有业务对象（`Get`，第 78 行）。
- `Put` 优先从 `pointers` 取出外壳写入归还值，再将其放入 `items`；没有可复用外壳时才分配新 `Box`（`Put`，第 102 行）。
- `Default` 保证池的零值语义：没有工厂且没有库存时，`Get` 返回 `T::default()`（`Default::default`，第 50 行；`Get`，第 81—87 行）。

这里的“零分配”是热路径倾向而不是绝对保证：首次构造、库存耗尽、`Put` 时无空外壳，以及两个 `Vec` 扩容时都可能分配。源码注释和测试均以预热后平均分配次数小于 1 为验收条件，而不是要求任何时刻都严格为 0。

## 主要符号

- `pub struct Pool<T>`：公开泛型池类型；私有字段为 `Mutex<Vec<Box<T>>>` 类型的 `items`、`pointers`，以及 `Option<Arc<dyn Fn() -> T + Send + Sync + 'static>>` 类型的 `new`。
- `impl<T> Default for Pool<T>`：公开类型的标准零值构造入口。它创建两个空向量且不安装工厂，对 `T` 本身没有约束。
- `pub fn New<T, F>(item: F) -> Pool<T> where F: Fn() -> T + Send + Sync + 'static`：公开自由函数；用 `Arc` 保存工厂，使同一个池被多线程共享时能够安全调用工厂。
- `impl<T: Default> Pool<T>`：`Get` 和 `Put` 所在实现块。Rust 版本要求 `T: Default`，因为移出值时使用 `std::mem::take`，零值池和新外壳也需要默认值。
- `pub fn Get(&self) -> T`：公开借用入口；从库存弹出一个对象，或通过工厂/`Default` 产生对象。
- `pub fn Put(&self, item: T)`：公开归还入口；外壳复用优先于重新装箱。
- `fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T>`：文件私有锁辅助函数；发生互斥锁中毒时通过 `PoisonError::into_inner` 继续取得数据。
- `#![allow(non_snake_case)]`：允许 `New/Get/Put` 保留 Go API 命名，以方便逐项对照。

文件没有模块级常量、trait、枚举或条件编译项；测试条件编译位于 `lib.rs`，不是本文件内部。

## 执行流程

`New` 的流程如下：创建空 `items` 和 `pointers`；将调用方闭包包装为 `Arc<dyn Fn() -> T + Send + Sync>`；返回完整 `Pool<T>`。此时尚未创建任何 `T` 或 `Box<T>`。

`Get` 的流程如下：

1. 短暂锁住 `items` 并执行 `pop`，随后释放该锁。
2. 若取到 `Box<T>`，继续处理；若没有库存但存在 `new`，调用工厂并以 `Box::new` 创建外壳；若既没有库存也没有工厂，则立即返回 `T::default()`，不会向 `pointers` 添加外壳。
3. 使用 `std::mem::take(&mut *ptr)` 将业务值移出，并以 `T::default()` 清空外壳。该步骤保证 `pointers` 不会因为保留旧值而延长其引用或堆资源的生命周期。
4. 短暂锁住 `pointers`，把空外壳压入栈，然后把业务值返回调用方。

`Put` 的流程如下：

1. 短暂锁住 `pointers` 并弹出空外壳。
2. 如果没有空外壳，以 `Box::new(T::default())` 创建一个。
3. 用传入的 `item` 覆盖外壳内默认值。
4. 短暂锁住 `items`，把可借出的外壳压入栈。

两条路径都不会同时持有 `items` 与 `pointers` 两把锁，因此文件内部不存在双锁顺序反转。

## 数据与状态

`items` 和 `pointers` 构成一条外壳生命周期环：`items` 中的外壳拥有有效业务值；`Get` 移出业务值并把清空后的外壳转到 `pointers`；`Put` 把归还值写入外壳并把它转回 `items`。从 API 角度看，借出的 `T` 完全归调用方所有，池内只保留一个默认值外壳，Rust 所有权系统避免调用方和池同时访问同一个 `T`。

`Vec::pop`/`push` 使两个池都表现为 LIFO，但顺序不是公开契约。池没有容量上限、淘汰策略或显式清理方法：只要 `Pool` 存活，已归还的值和空外壳便可能一直保留；销毁 `Pool` 时，两个向量、其中的 `Box<T>`、工厂 `Arc` 及其捕获状态按 Rust RAII 自动释放。

`new` 在构造后不会改变。源码注释沿用 Go 的“首次使用后不得复制”约束，但 Rust `Pool<T>` 没有实现 `Clone`，普通赋值是所有权移动而非复制；跨线程共享应使用 `Arc<Pool<T>>`，测试也采用这一方式。

## 依赖与调用关系

直接标准库依赖仅有 `std::sync::{Arc, Mutex, MutexGuard}`，以及完全限定调用的 `std::mem::take`。`pkg/util/zeropool/Cargo.toml` 没有第三方依赖或 feature；它仅声明包名、工作区版本/edition/publish 配置、`lib.rs` 入口和 Go 迁移元数据。

已验证的 Rust 调用/导出关系为：

- `pkg/util/zeropool/lib.rs` → `pool` 模块 → 再导出 `Pool`/`New`。
- 根 `Cargo.toml` 以别名 `facade_util_zeropool` 依赖该路径 crate，`pkg/lib.rs::util::zeropool` 再导出其公开项。
- `pkg/util/zeropool/pool_test.rs::test_pool` 与 `migration_aster_unit_test.rs` 中五个回归测试直接调用 `New/Get/Put` 或 `Pool::default`。
- `benches/zeropool_standalone.rs::canonical_zeropool_is_available_through_the_root_facade` 经根门面调用这些 API。

RustCodeGraph 能识别本文件的 `Pool/default/New/Get/Put/lock` 七个符号，但其 `callers`/`callees` 查询没有返回边；因此上述关系由模块入口、Cargo 声明和精确文本引用补证。`pkg/dumpformat/parquetfile/Cargo.toml` 虽声明了该 crate，相关 `parser.rs` 使用位置仍是注释，不能作为运行时调用证据。

## 错误处理与边界

公开 API 不返回 `Result`，文件也不主动产生领域错误。工厂闭包或 `T` 的析构/赋值若 panic，panic 会继续向上传播。

`lock` 对中毒的 `Mutex` 不调用 `unwrap` 终止，而是取出内部数据继续工作；这是为了贴近 Go `sync.Pool` 在其他 goroutine panic 后仍可使用的操作语义。该恢复只表示允许继续访问，不验证中毒发生前的数据是否满足更高层业务不变量；新增复合状态时不能把它当作事务回滚。

关键边界包括：

- `T` 必须实现 `Default` 才能调用 `Get/Put`，而 Go 的 `Pool[T any]` 没有这一类型约束。
- 零值池空取每次直接返回 `T::default()`；此分支没有可回收的 `Box`，所以不会为随后 `Put` 预留外壳。
- 工厂池空取会调用工厂且新建 `Box`；工厂返回值是否满足业务不变量完全由调用方负责。
- `Put` 接受任意同类型值，不检查重复归还、来源或容量；逻辑上重复归还会形成多个独立所有权值，Rust 安全代码不能把同一个非 `Copy` 值按值重复传入。
- 无界缓存可能保留大对象；调用方应在归还前清理不应跨请求保留的内容，并在业务层限制归还对象容量。

## 并发与资源生命周期

`Pool<T>` 的共享可变状态全部位于 `Mutex` 内，工厂要求 `Send + Sync`；当 `T` 满足跨线程所需的自动 trait 条件时，可通过 `Arc<Pool<T>>` 并发调用。`pool_test.rs::test_pool` 和 `migration_pool_is_safe_for_concurrent_get_and_put` 使用 255 个线程执行共 1,000,000 次 `Get`/修改/`Put`，验证同一借出对象不会被其他线程同时复用。

每次 `Get` 或 `Put` 至多分别进入两个短临界区，业务值的使用发生在锁外。`Get` 会在调用工厂前释放 `items` 锁，`Put` 会在写值前释放 `pointers` 锁，降低了长时间持锁和用户代码重入锁的风险。代价是这里使用两个全局 `Mutex<Vec<_>>`，与 Go `sync.Pool` 的运行时本地缓存、GC 时可清空语义不同；高争用性能不能由 Go 结果直接推导，现有 Rust 测试证明的是安全性和分配趋势，不是吞吐等价。

资源保持规则是：借出值离开池后由调用方拥有；空外壳留在 `pointers`；归还值留在 `items`；少 `Put` 不会让空外壳继续持有旧业务值，因为 `mem::take` 已将其替换为默认值。多 `Put` 或长期不再 `Get` 会使 `items` 持续保留值直到再次借出或整个池析构。

## 与 Go 版本的对应关系

结构与主算法逐项对应 `pkg/util/zeropool/pool.go`：Go 的两个 `sync.Pool` 对应 Rust 的两个 `Mutex<Vec<Box<T>>>`；Go `items.New` 对应 Rust `new` 工厂；Go `*ptr` 取值并写回零值对应 Rust `mem::take`；Go 从 `pointers` 复用 `*T` 对应 Rust 复用 `Box<T>`。

主要差异如下：

- Go `sync.Pool` 可由运行时在 GC 时清空，Rust 向量不会自动淘汰，因此 Rust 的保留时间和内存上界不同。
- Go 对 `T` 使用 `any`，零值由语言提供；Rust 的方法要求 `T: Default`。
- Go 空工厂池的 `items.Get` 会通过 `sync.Pool.New` 得到指针；Rust 显式调用闭包后 `Box::new`。
- Go 用接口值和运行时类型断言，Rust 用泛型与所有权在编译期保证类型安全。
- Go 注释中的可并发调用由 `sync.Pool` 提供；Rust 由两把 `Mutex` 和线程安全工厂约束提供。

测试意图保持对齐：`pool_test.go::TestPool` 的正确取值、并发、预热后平均分配小于 1、零值可用四个子场景，均在 `pool_test.rs::test_pool` 中保留；Rust 的 `migration_aster_unit_test.rs` 又拆分出工厂调用次数、零值、分配和并发回归。Go 生产调用见 expression、planner、dumpformat、lightning 等目录；当前 Rust 生产接线不能据此推定，且 expression 仍明确是 `DropPool` 占位。

## 扩展指南

若修改池算法，应优先在 `Pool::Get`、`Pool::Put` 和私有 `lock` 中保持以下不变量：`items` 只含有效业务值，`pointers` 只含默认/可覆盖值；跨池移动前不同时持有两把锁；借出前移除池所有权，归还后调用方不再拥有该值；中毒恢复策略必须明确。

若增加容量限制、淘汰、统计或清理 API，需要同时决定 `items` 与 `pointers` 的处理方式，并评估与 Go `sync.Pool` 的 GC 淘汰差异、额外原子/锁开销以及大对象保留风险。若改变 `T: Default` 约束或工厂表示，应同步检查零值池语义、`mem::take` 的替代方案和公开 API 兼容性。

测试必须继续放在独立文件中：Go 对齐场景更新 `pkg/util/zeropool/pool_test.rs`，Rust 特有回归更新 `pkg/util/zeropool/migration_aster_unit_test.rs`；根门面变化才需要调整 `benches/zeropool_standalone.rs`。若将生产 Rust 模块从 `DropPool` 接到该实现，应在对应业务 crate 中新增独立测试，确认借还前的清空/容量规则，且不能仅用本 crate 的算法测试代替业务接线验证。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,305 个节点和 1,849,011 条边；`files --filter pkg/util/zeropool` 找到本 crate 的 Rust/Go 源与测试。
- RustCodeGraph 符号证据：`query`/`node` 确认 `Pool`（第 36 行）、`default`（第 50 行）、`New`（第 62 行）、`Get`（第 78 行）、`Put`（第 102 行）和 `lock`（第 113 行）；`callers`/`callees` 未返回边，故未把图中不存在的调用关系写成事实。
- 实现与边界：`pkg/util/zeropool/pool.rs`；crate 声明与导出：`pkg/util/zeropool/Cargo.toml`、`pkg/util/zeropool/lib.rs`、根 `Cargo.toml`、`pkg/lib.rs`。
- Go 对照：`pkg/util/zeropool/pool.go`；Go 测试：`pkg/util/zeropool/pool_test.go`。
- Rust 测试：`pkg/util/zeropool/pool_test.rs`、`pkg/util/zeropool/migration_aster_unit_test.rs`；根门面证据：`benches/zeropool_standalone.rs`。
- 接线现状补证：`rg` 仅发现上述 Rust 测试/门面直接调用；`pkg/expression/core_support.rs` 明确将池声明为 `DropPool` 占位，`pkg/dumpformat/parquetfile/parser.rs` 中 zeropool 使用仍为注释。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前仅执行任务指定的 11 章节结构检查并人工复核结论与证据。
