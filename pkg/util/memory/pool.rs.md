# `pkg/util/memory/pool.rs`

## 文件定位

本文件属于 `astersql-util-memory` crate。`pkg/util/memory/Cargo.toml` 指定 `lib.rs` 为 crate 根，`pkg/util/memory/lib.rs` 通过 `pub mod pool;` 公开本模块；它没有被 `mem-arbitrator` feature 条件包围，因此模块始终参与该 crate 的编译。文件实现一棵进程内资源配额树：`ResourcePool` 是树节点和配额账户，`Budget` 是从某个 pool 取得、消费并归还容量的句柄。

当前 Rust 仓库中，经限定 `.rs` 路径搜索，生产代码没有直接引用 `pool::ResourcePool`、`pool::Budget` 或 `NewResourcePool*`；可观察行为主要由 `pkg/util/memory/pool_test.rs` 覆盖。因此它是已经公开并具有完整局部行为、但尚未接入 Rust 应用主链的 Go 移植模块，不能把同 crate 的 `arbitrator.rs`/`tracker.rs` 新式预算类型当成其直接调用者。直接 Go 对照是 `pkg/util/memory/pool.go`。

## 核心职责

- 以 `ResourcePool` 维护名称、唯一 ID、硬上限、额外保留额度、对齐粒度、历史峰值，以及父子 pool 链表。
- 以 `Budget` 表示从来源 pool 申请的容量；支持 `Reserve`、`Grow`、`ResizeTo`、`Shrink`、`Empty` 和 `Clear`，并把容量申请/释放递归传播到父 pool。
- 在 `doAlloc` 中同时执行 pool 自身的 `limit` 检查和上游容量检查，分别触发 `OutOfLimitActionCB` 与 `OutOfCapacityActionCB`。
- 通过 `Start`/`Stop` 管理树节点挂载、配额初始化、余额归还和兄弟链表摘除；通过 `Traverse` 生成监控快照。
- 用 `allocAlignSize` 和 `maxUnusedBlocks` 控制批量申请与闲置容量回收，避免每个小额变动都向父 pool 传播。

## 主要符号

- `DefPoolAllocAlignSize`（10 KiB）和 `DefMaxUnusedBlocks`（10）：默认对齐粒度与允许保留的闲置块数。私有 `DefMaxLimit` 当前在本文件内固定为 `5_000_000_000_000_000`，用于替代 Go `arbitrator.go` 中的同名值。
- `ResourcePool`：配额树节点。公开配置包括 `actions`、`name`、`uid`、`reserved`、`limit`、`allocAlignSize` 和 `maxUnusedBlocks`；`mu` 中保存动态状态，`parentMu` 保存由父节点锁保护的兄弟指针。
- `ResourcePoolMu`：`Mutex` 内的复合状态，包括子链表头、上游 `Budget`、当前/峰值分配量、子节点数和 `stopped`。
- `PoolLink = Option<NonNull<ResourcePool>>`：不拥有对象的可空裸指针，模拟 Go 的 `*ResourcePool`；它是本实现所有 `unsafe` 生命周期假设的中心。
- `PoolActions`、`OutOfCapacityActionArgs`、`OutOfCapacityAction`、`OutOfLimitAction`：两个可选回调及其参数。Rust 用 `Arc<dyn Fn + Send + Sync>` 保留可共享的函数值。
- `ResourcePoolState`：`Traverse` 交给回调的快照，包含层级、节点/父节点 ID、已用量、保留量和预算容量。
- `NewResourcePoolDefault`、`NewResourcePool`、`NewResourcePoolInheritWithLimit`：构造入口；只创建对象，不自动执行 `Start`。`newPoolUID` 从 `-1` 原子递减并转换为 `u64`，第一次生成的是 `-2` 的无符号表示。
- `Start`、`Stop`、`Traverse`：节点生命周期和树遍历入口。`StartNoReserved` 是 `Start(parent, 0)` 的便捷包装。
- `Budget`：保存 `pool`、`cap`、`used` 和 `explicitReserved`；`available()` 的定义是 `cap - used`。
- `doAlloc`/`increaseBudget`/`doRelease`/`doAdjustBudget`：pool 的锁内记账核心；`roundSize` 负责向上对齐，`newBudgetExceededError` 统一生成超限文本。

## 执行流程

1. 调用方用 `NewResourcePoolDefault` 或 `NewResourcePool` 构造尚未启动的节点。非正 `allocAlignSize` 回落到 10 KiB，非正 `limit` 回落到本文件的 `DefMaxLimit`。
2. `Start(parent, reserved)` 要求当前 `allocated == 0` 且尚未关联上游 pool，否则 panic。它清零峰值，建立指向父 pool 的内部 `Budget`，记录 `reserved`；有父节点时，在父锁内把自己插到子链表头。
3. 调用方通过 `CreateBudget` 获得以当前 pool 为来源的空预算。`Budget::Grow(request)` 先计算 `request - available()`；有缺口时按来源 pool 的 `allocAlignSize` 向上取整，再调用来源 pool 的 `allocate`，成功后增加 `cap` 和 `used`。
4. `allocate -> doAlloc` 先判断 `allocated > limit - request`。超限时调用 `OutOfLimitActionCB`；没有回调则返回 `out of limit`。随后计算 `request + allocated - budget.used - reserved`，只有正缺口才调用 `increaseBudget`。
5. 非根节点的 `increaseBudget` 调用其内部 `Budget::Grow`，从而递归向父节点申请。根节点没有上游：现有内部预算足够时只增加 `budget.used`；否则调用容量不足回调，或返回 `out of quota`。容量回调必须自行增加根容量，例如测试通过 `forceAddCap` 补足缺口。
6. `Budget::Shrink` 最多把 `used` 减到零；空闲量超过一个对齐块，且释放不破坏 `explicitReserved` 约束时，把多余容量归还来源 pool。`Empty` 清零 used 并保留一个对齐块；`Clear` 清零 cap/used 并全量归还原 cap，但保留 `pool` 指针。
7. pool 的 `doRelease` 饱和扣减 `allocated`，然后 `doAdjustBudget` 根据 `allocated - reserved` 算出对齐后的必要上游用量；当闲置用量至少达到 `allocAlignSize * maxUnusedBlocks` 时，通过内部 `Budget::Shrink` 向父层释放。
8. `Stop` 标记 stopped，释放剩余 allocated 和内部 Budget，按 prev/next 从父节点子链表摘除，最后清空内部 `budget.pool`；返回停止前的 budget cap。调用方应先清理外部 Budget，避免其继续持有已停止节点的裸指针。
9. `Traverse` 递归生成先父后子的深度优先快照。每层在锁内复制当前状态和子指针列表，释放锁后再调用用户回调和递归；回调错误立即停止遍历并向上传播。

## 数据与状态

核心计量关系是：每个外部 `Budget.cap` 都来自其来源 pool 的 `allocated`，测试用所有 Budget 的 `Capacity()` 之和等于 pool 的 `Allocated()` 来验证该不变量。pool 自身向父层申请的量记录在 `mu.budget`；`reserved` 可覆盖本节点一部分分配，只有超出的对齐需求才消耗父层额度。`maxAllocated` 单调记录生命周期峰值，`Start` 会把它重置。

`Budget.cap` 是已从来源 pool 取得的容量，`used` 是其中当前使用量，`explicitReserved` 是不能被一般 shrink 破坏的显式预留目标。`Reserve(request)` 申请按来源 pool 粒度对齐的完整新增容量，并把未对齐的 request 累加到 `explicitReserved`；它不是简单增加 used。`ResizeTo` 以当前 `used` 为旧值，正差调用 `Grow`，负差调用 `Shrink`。

树结构不是拥有关系。`Box<ResourcePool>` 拥有节点内存，但父子边和 Budget 来源都只是 `NonNull`；`ResourcePoolParentMu.prevChildren/nextChildren` 也不会延长兄弟节点生命。`stopped` 控制监控可见性：`traverse` 遇到停止节点就不报告该节点，也不进入其子树。

## 依赖与调用关系

crate 内边界由 `pkg/util/memory/lib.rs` 的 `pub mod pool` 建立。文件的直接代码依赖仅是标准库 `NonNull`、`AtomicI64`、`Arc` 和 `Mutex`；虽然 `pkg/util/memory/Cargo.toml` 声明了 errno、config、cgroup、log、sqlkiller、sysinfo 等 crate 级依赖，本文件没有直接使用它们。`mem-arbitrator` feature 也没有改变本模块的编译或实现。

内部主要调用边为：`NewResourcePoolDefault` 和 `NewResourcePoolInheritWithLimit -> NewResourcePool`；`Budget::Grow/Reserve -> ResourcePool::roundSize -> allocate -> doAlloc -> increaseBudget`；非根 `increaseBudget -> Budget::Grow` 形成沿父链递归申请；`Budget::Shrink/Empty/Clear -> ResourcePool::release -> doRelease -> doAdjustBudget -> Budget::Shrink` 形成沿父链递归归还；`Stop -> doRelease + releaseBudget(Budget::Clear)`；`Traverse -> traverse` 递归访问子链。

RustCodeGraph 的文件节点报告 `pool.rs` 被 13 个文件使用，但列出的候选包含大量测试；对精确 Rust API 做限定搜索后，只确认 `pkg/util/memory/pool_test.rs` 以及同包补充测试引用这些符号，未发现 Rust 生产调用者。图查询对 `Grow`、`Traverse`、`NewResourcePool` 等常见名字还会混入无关 Go 同名符号，故这里不据此宣称跨子系统接线。

## 错误处理与边界

`MemoryResult` 当前只是私有 `Result<(), String>`，并未接入 crate 的结构化 errno；两类内建拒绝文本由 `newBudgetExceededError` 生成。回调返回的 `Err(String)` 原样传播；`Traverse` 也在首个回调错误或子树错误处短路。所有 `Mutex::lock` 中毒都通过带上下文的 `expect` 变成 panic。

`Start` 对残留分配量或重复启动使用 panic。分配超 `limit` 时，若有 `OutOfLimitActionCB` 且回调返回成功，`doAlloc` 会继续执行，而不是自动重新检查 limit；容量不足回调同理必须实际补足 root cap，本文件不验证回调后的容量。回调在 pool 锁持有期间执行，若同步重入需要同一把 `mu` 的普通 API，存在自锁风险；测试专用 `forceAddCap` 使用 `Mutex::get_mut` 正是为了避免再次加锁。

负数请求没有统一拒绝：`Grow`、`Reserve`、`ResizeTo`、`roundSize` 和 `doAlloc` 假定调用方提供合理尺寸；极大正值的 `limit - request`、`sz + alignSize - 1` 在 Rust debug 构建可溢出 panic。`pool_test.rs::TestMemoryAllocationEdgeCases` 因此接受“panic 或错误”两种结果，说明该溢出边界尚未统一成稳定错误契约。

Go 的 `Budget` 方法允许 nil receiver；Rust 不能调用空引用，改用 `Option<Budget>` 或 `PoolLink::None` 表达缺省。`Budget` 的方法在 `pool == None` 时大多空操作，但并不等同于 Go 所有 nil receiver 场景。`Stop` 不自动停止子节点，也没有 `Drop` 自动清理；错误的停止顺序或提前释放 Box 会破坏裸指针不变量。

## 并发与资源生命周期

`ResourcePool.mu` 保护本节点的动态预算、分配量、子链头、子数量和 stopped；父节点的同一把 `mu` 还保护每个子节点的 `parentMu` 链接。`Start` 与 `Stop` 以“先锁自己、再锁父节点”的顺序操作，而预算增长可在持有子锁时递归取得父锁，因此树必须保持无环，且回调/其他操作不能反向获取已持有的子锁。

`Traverse` 的锁策略是先在本节点锁内复制 `ResourcePoolState` 和全部子裸指针，再解锁执行回调和递归，从而避免用户回调长期占锁；代价是调用方必须保证快照中的节点在遍历结束前仍存活。`pool_test.rs::TestResourcePoolNoDeadlocks` 在并发创建/停止子池和遍历时把 stopped 节点保存在 `retired` 中，明确验证并示范了这一生命周期要求。

`resourcePoolID` 用 `SeqCst` 原子递减，确保并发构造内部 ID 不重复。除此之外，类型没有显式 `Send`/`Sync` 安全封装；`NonNull` 与广泛的 `unsafe as_ref/as_mut` 把别名、可变访问和存活性责任交给调用方。公开修改方法多要求 `&mut self`，但回调中可通过裸指针再次取得可变引用，因此扩展时必须首先审查别名安全，而不能只依赖 `Mutex`。

正常生命周期是：构造 -> `Start` -> 创建/使用/清理所有 Budget -> 子节点先 `Stop` -> 父节点 `Stop` -> 最后释放 Box。`Clear` 不断开 Budget 的 pool 指针；只有 `ResourcePool::Stop` 清空节点内部上游 Budget 的指针，外部 Budget 仍需由调用方管理。

## 与 Go 版本的对应关系

`pkg/util/memory/pool.go` 是逐符号对照：常量、`ResourcePool`/`Budget` 字段、构造函数、父子链表、对齐申请、两类回调、错误文本和 `Traverse` 快照均保持同一算法。`pkg/util/memory/pool_test.go` 与 `pkg/util/memory/pool_test.rs` 共享核心测试意图：随机预算不变量、双 Budget 竞争、nil/空预算、limit、对齐、多层树、reserved、回调和并发无死锁。

Rust 的主要表达差异是：Go 裸指针改为 `Option<NonNull<_>>`，函数值改为 `Arc<dyn Fn + Send + Sync>`，匿名锁内结构拆成 `ResourcePoolMu`，错误临时用 `String`，Go nil receiver 用 Option/空 pool 分支近似。Go `Name()` 返回字符串值，Rust 返回 `&str`；Rust 的 `ApproxAllocated`/`ApproxCap` 实际仍加 `Mutex`，比 Go 的无锁近似读更强。

已知迁移差异不能忽略：Go `DefMaxLimit` 来自 `arbitrator.go`，Rust 在本文件复制常量；Go 测试中的 limit 回调可用原子 setLimit 后继续，Rust 测试回调直接返回错误；Rust 极大尺寸测试允许溢出 panic；Rust 的裸指针生命周期由测试手工维持。`pool_test.rs::TestResourcePool` 还用 `Budget::Clear + Grow` 重建可观察状态，而非像 Go 测试直接调用私有 `release`。这些都表明当前实现保留主算法，但安全封装和部分边界契约仍未完全等价。

## 扩展指南

- 新增分配策略应从 `Budget::Grow/Reserve`、`ResourcePool::doAlloc/increaseBudget` 和反向的 `Shrink/doRelease/doAdjustBudget` 成对修改；必须维持 cap、used、allocated 与父预算之间的记账守恒，并同步 `pkg/util/memory/pool_test.rs::TestPoolAllocations`。
- 修改对齐或闲置回收策略时，覆盖请求小于/等于/刚超过对齐粒度、显式预留和多级父子池；重点测试 `TestBudget`、`TestActions`、`TestMultiSharedGauge`。
- 修改树生命周期时，必须同时更新 `Start`、`Stop`、`traverse` 和 prev/next/headChildren；独立测试放在 `pool_test.rs`，扩展 `TestResourcePoolTree` 与 `TestResourcePoolNoDeadlocks`，不要把测试嵌入生产文件。
- 若要接入 Rust 生产主链，应先把 `PoolLink` 裸指针所有权模型设计清楚（例如稳定拥有节点的句柄），再暴露跨线程 API；不能仅凭 Go 指针模型假定 Rust 引用有效。
- 若要对齐错误处理，应统一整数溢出、负请求、重复 Stop/Start 和回调后重新校验策略，并将 `String` 迁移到项目错误类型；同时与 `pool.go`/`pool_test.go` 核对兼容语义。
- 若要消除本地 `DefMaxLimit`，应从 `arbitrator` 模块建立单一来源并检查是否产生模块耦合；Cargo 依赖不需要为标准库实现额外增加外部 crate。
- 性能风险集中在每次分配的互斥锁、沿父链递归加锁、`Traverse` 的子节点 Vec 分配，以及回调在锁内执行；安全风险集中在 `NonNull` 悬垂和 `unsafe` 可变别名。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点；`files --filter pkg/util/memory/pool.rs` 确认目标文件有 73 个符号；`node --file pkg/util/memory/pool.rs` 分三段读取全部 897 行；`query NewResourcePool`/`query ResourcePool` 同时定位 Rust 与 Go 定义；带 `--file pkg/util/memory/pool.rs` 的 `callers/callees Grow/NewResourcePool/Traverse` 确认内部边与测试候选，并暴露同名符号误配，故外部调用再用限定 `rg` 复核。
- Rust 源与模块边界：`pkg/util/memory/pool.rs`、`pkg/util/memory/lib.rs`、`pkg/util/memory/Cargo.toml`。
- Rust 独立测试：`pkg/util/memory/pool_test.rs`。重点覆盖 `TestPoolAllocations`、`TestBudget`、`TestNilBudget`、`TestResourcePool`、`TestMemoryAllocationEdgeCases`、`TestActions`、`TestResourcePoolTree`、`TestResourcePoolUsedFromReserved` 和 `TestResourcePoolNoDeadlocks`。
- Go 对照与回归依据：`pkg/util/memory/pool.go`、`pkg/util/memory/pool_test.go`；二者用于核对算法、错误文本、树顺序、对齐和测试意图。
- 调用接线复核：限定 Rust 文件搜索 `NewResourcePoolDefault`、`OutOfCapacityActionArgs`、`ResourcePoolState` 以及 `astersql_util_memory::pool` 等路径后，未发现 Rust 生产调用者；当前直接 Rust 使用证据来自测试，模块公开证据来自 `lib.rs`。
- 本任务是纯文档分析，按计划未运行 Cargo。交付验证仅执行固定 11 章节结构检查，并人工确认符号、路径、调用边、已知差异和扩展风险均可由上述材料反查。
