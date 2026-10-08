# `pkg/planner/core/plan_cache_lru.rs`

## 文件定位

本文件实现 `astersql-planner-core` crate 中的会话级 LRU 计划缓存。模块在 [`lib.rs`](./lib.rs) 中以 `mod plan_cache_lru` 声明，并通过 `pub use plan_cache_lru::*` 导出 `LRUPlanCache` 与 `NewLRUPlanCache`。其缓存值不是任意对象，而是 [`plan_cache_utils.rs`](./plan_cache_utils.rs) 定义的 `PlanCacheValue`；查找时还复用该文件的 `CheckTypesCompatibility4PC` 判断参数类型能否共享缓存计划。

当前仓库的 Rust 生产代码没有构造或调用 `LRUPlanCache`：除本模块外，`NewLRUPlanCache`/`LRUPlanCache` 的 Rust 引用只出现在独立测试 [`plan_cache_lru_test.rs`](./plan_cache_lru_test.rs)。因此它是已经由 crate 公共接口导出、具有真实实现和测试，但尚未接入 Rust 会话主链的移植模块。Go 主链则在 `pkg/session/session.go` 创建同名缓存。

## 核心职责

- 用 `VecDeque<LRUPlanCacheEntry>` 维护全局最近使用顺序：队首是最近使用项，队尾是最旧项。
- 允许同一个字符串键保存多个参数类型桶；`Get` 和兼容桶替换都以 `CheckTypesCompatibility4PC` 为匹配条件，而不是只比较键。
- 在新增条目后先按条目数容量驱逐，再按 `quota * (1 - guard)` 的估算内存阈值驱逐。
- 维护缓存自己的估算内存计数，并提供删除、清空、动态缩容和关闭接口。
- 用单个 `Mutex` 串行化所有状态访问，用 `Arc<PlanCacheValue>` 让命中值在移出缓存后仍可由调用者安全持有。

## 主要符号

- `LRUPlanCacheEntry { key, value }`：内部条目；`value` 是 `Arc<PlanCacheValue>`，不对 crate 外公开。
- `LRUPlanCacheState { capacity, entries, memory }`：锁内可变状态。`capacity` 是条目上限，`memory` 是所有条目的“键字节数 + `PlanCacheValue::MemoryUsage()`”之和。
- `LRUPlanCache { guard, quota, state }`：公共缓存句柄。`guard` 与 `quota` 构造后不可修改；队列、容量与内存计数都在 `state` 的互斥锁内。
- `NewLRUPlanCache(capacity, guard, quota)`：公共构造函数。`capacity == 0` 时采用默认容量 100；本函数不校验 `guard` 范围。
- `Get(key, parameter_types)`：查找第一个键相等且参数类型兼容的条目；命中后将它移到队首并返回其值的 `Arc` 克隆。
- `Put(key, value)`：接受任何 `Into<Arc<PlanCacheValue>>`。兼容桶已存在时替换并提升；否则插入队首并执行容量、内存控制。
- `Delete`、`DeleteAll`、`Close`：分别删除指定键的全部桶、清空状态、以清空语义关闭缓存。
- `SetCapacity`：拒绝容量 0；缩小时同步从队尾驱逐至满足新容量。
- `Size`、`MemoryUsage`：在锁内读取当前条目数和缓存估算内存。
- `state`、`entryMemoryUsage`、`removeOldest`、`memoryControl`：内部的锁恢复、内存估算、队尾驱逐和配额控制辅助函数。

文件没有 trait、模块级常量或条件编译项。名称保留了 Go 风格的大写函数/方法，以便迁移语义对照。

## 执行流程

1. `NewLRUPlanCache` 保存 `guard`/`quota`，创建空队列与零内存计数；传入容量为 0 时改为 100。
2. `Put` 先把输入转换为 `Arc<PlanCacheValue>` 并取得状态锁，然后线性扫描队列，寻找同键且 `ParamTypes` 兼容的条目。
3. 若找到兼容桶，`Put` 移除旧条目并扣减旧估算值，再加入新估算值、把替换条目推到队首并立即返回。这个提前返回意味着替换路径不会再次执行容量或 memory guard 驱逐；[`plan_cache_lru_test.rs`](./plan_cache_lru_test.rs) 的 `test_lru_compatible_bucket_replacement_skips_memory_guard` 专门固定了该行为。
4. 若没有兼容桶，`Put` 将新条目推到队首；条目数超过 `capacity` 时反复调用 `removeOldest`，随后调用 `memoryControl`。
5. `memoryControl` 在 `quota == 0` 或 `guard == 0.0` 时禁用。否则计算 `quota * max(1 - guard, 0)`，只要缓存估算内存仍超阈值且队列非空，就从队尾驱逐。
6. `Get` 也线性扫描队列；未命中返回 `None`，命中则从原位置移除、推到队首并返回 `Arc` 克隆。后续容量驱逐因此优先删除未被提升的旧条目。
7. `Delete` 用 `retain` 一次移除指定字符串键的全部参数类型桶并汇总扣减内存；`DeleteAll`/`Close` 清空队列并将内存归零；`SetCapacity` 缩容时同样按队尾顺序同步驱逐。

## 数据与状态

LRU 顺序和键到条目的关系只保存在一个 `VecDeque` 中，没有额外哈希索引。因此 `Get`、兼容桶替换和 `Delete` 都是 O(n) 扫描；队尾驱逐本身是 O(1)，但从队列中间命中或替换需要移动元素。一个键可以对应多个不兼容的 `ParamTypes`，每个桶都独立计入容量。

`memory` 的不变量是锁内现存条目的 `entryMemoryUsage` 总和。单条估算等于 UTF-8 键的字节长度加 `PlanCacheValue::MemoryUsage()`；后者在 [`plan_cache_utils.rs`](./plan_cache_utils.rs) 中首次计算后缓存到原子字段，包含值结构、字符串容量、输出列、参数类型和计划估算。插入、替换、删除、容量驱逐和清空都成对更新该计数。

`guard` 没有范围校验。负值会把阈值放大到高于 `quota`；大于等于 1 的值经 `.max(0.0)` 变成零阈值，从而在新增路径清空所有正内存条目。`quota == 0` 或精确的 `guard == 0.0` 表示不启用内存控制。

## 依赖与调用关系

上游装配关系是 `pkg/planner/core/Cargo.toml` 的 `[lib] path = "lib.rs"`，再由 `lib.rs` 声明并再导出本模块。Cargo 清单表明本文件直接使用的类型来自同 crate（`PlanCacheValue`、`CheckTypesCompatibility4PC`）、标准库（`VecDeque`、`Arc`、`Mutex`）和 `types-dependency`（参数 `FieldType`）；`types-dependency` 映射到工作区路径 `pkg/types`。

RustCodeGraph 对本文件的文件边只列出 [`plan_cache_lru_test.rs`](./plan_cache_lru_test.rs) 与 [`plan_cache_instance_test.rs`](./plan_cache_instance_test.rs)，精确的仓库 Rust 引用搜索进一步确认：排除本文件及专属测试后没有 `LRUPlanCache`/`NewLRUPlanCache` 使用者。后者虽然被图索引列为文件关联，但测试内容实际针对 `InstancePlanCache`，不是本类型的行为覆盖；不能把它当作本缓存的上游。

下游调用中，`Get` 与 `Put` 依赖 `CheckTypesCompatibility4PC`。该函数规定：任一参数列表为空即兼容；非空列表长度必须相同；逐项比较类型、字符集和排序规则，整数还比较 unsigned 标志，decimal 要求缓存类型的长度和小数位不窄于当前类型。`entryMemoryUsage` 调用 `PlanCacheValue::MemoryUsage`，`Put`/`Get`/删除及查询接口都通过内部 `state()` 获取同一把锁。

Go 侧实际上游是 `pkg/session/session.go` 对 `plannercore.NewLRUPlanCache` 的构造；Rust 当前没有等价会话接线，因此“会话级”描述来自模块职责和 Go 对照，而不是声称 Rust 会话已经使用它。

## 错误处理与边界

本模块只有 `SetCapacity(0)` 返回显式错误，错误文本为 `capacity of LRU cache should be at least 1`；其他接口不返回业务错误。构造时容量 0 不报错而是回退到 100，这与运行期调容的严格规则不同。

`Get` 的两次可失败操作用 `?` 返回 `None`：找不到兼容条目时正常未命中；找到索引后若 `VecDeque::remove` 意外失败也按未命中处理，不过索引来自同一锁域内的即时扫描，正常情况下不会失效。删除不存在的键、从空队列驱逐、重复清空和关闭都安全无操作。

锁毒化不是永久错误：`state()` 用 `poisoned.into_inner()` 继续使用原状态。这提高了可用性，但也意味着若另一个线程在修改状态中 panic，缓存不会自动清空或重新校验 `memory` 不变量。内存计数使用 `i64` 且没有 checked arithmetic；正常计数由成对加减保持非负，`memoryControl` 比较时用 `max(0)` 防止负数直接转换成巨大 `u64`。

## 并发与资源生命周期

`LRUPlanCache` 的全部可变状态由一个 `Mutex<LRUPlanCacheState>` 保护。包括 `Size` 和 `MemoryUsage` 在内的读操作也取得独占锁；`Get` 必须写 LRU 顺序，所以同样不能使用只读锁。一次方法调用内，队列结构和内存计数的修改均在同一锁域完成，没有观察到半更新状态的窗口。

缓存值由 `Arc` 管理。缓存持有一个强引用，`Get` 返回另一个强引用；条目被替换、删除或驱逐只释放缓存自己的引用，不会使外部正在使用的计划失效。`Close` 不关闭外部资源，只调用 `DeleteAll`，缓存对象本身之后仍可再次 `Put`。

本文件不创建线程、异步任务、通道或事务，也没有 `Drop` 实现。线程安全依赖 `Mutex` 以及 `PlanCacheValue` 内部拥有型数据和原子运行时统计；专属 LRU 测试覆盖顺序和内存行为，但没有并发压力测试，因此本类型跨线程并发的运行验证仍是覆盖缺口。

## 与 Go 版本的对应关系

Rust 对照文件是 [`plan_cache_lru.go`](./plan_cache_lru.go)，核心语义保持一致：默认容量 100、同键多参数类型桶、命中/替换提升到 LRU 前端、超容量从尾部驱逐、删除一个键的全部桶、缩容驱逐、关闭等价清空，以及替换兼容桶后提前返回而不运行 memory guard。

实现数据结构不同：Go 用 `map[string]map[*list.Element]struct{}` 加双向链表，按键先定位桶；Rust 只用 `VecDeque`，所有匹配都线性扫描。Go 的 `Get` 在 `RLock` 下仍移动链表，Rust 则用互斥锁明确串行化该写操作。

Rust 还没有完整复刻 Go 的外围接线。Go 构造函数接收 `sessionctx.Context`，支持测试用 `onEvict` 回调，并通过 `updateInstanceMetric` 更新计划数和可选的会话计划缓存内存指标；Rust 没有这些字段或副作用。Go `memoryControl` 读取进程级 `memory.InstanceMemUsed()`，Rust 比较本缓存自身的估算 `memory`，因此两者在相同 quota 下的触发时机可能不同。Go 的 `DeleteAll`/`MemoryUsage` 接受 nil 接收者，Rust 所有方法要求有效引用。Go 生产会话已经构造该缓存，Rust 尚未接入生产调用链。

Rust 专属测试 [`plan_cache_lru_test.rs`](./plan_cache_lru_test.rs) 对照 Go 测试覆盖了默认容量、多桶淘汰、`Get` 提升、删除/清空、缩容错误、内存计数、小配额 guard 和替换提前返回；Go 测试额外观察 `onEvict`、内部 bucket 收缩及指标相关内存结果。

## 扩展指南

- 若要把缓存接入 Rust 会话主链，应从 `NewLRUPlanCache` 的实际构造点开始，明确 quota 是缓存局部预算还是进程内存水位，并新增独立的会话集成测试；不要仅因 API 已由 `lib.rs` 导出就假定功能已启用。
- 若新增按键索引以改善 O(n) 查找，必须同时维护索引、`VecDeque` 顺序和 `memory` 三者的一致性，并覆盖兼容桶替换、队尾驱逐、删除全部桶和缩容路径。Go 的 bucket + list 结构可作为语义参考，但 Rust 不能保存会因队列移动而失效的裸位置。
- 若修改 `CheckTypesCompatibility4PC` 或桶选择规则，应同步更新 [`plan_cache_utils_test.rs`](./plan_cache_utils_test.rs) 的类型兼容测试和 [`plan_cache_lru_test.rs`](./plan_cache_lru_test.rs) 的多桶/替换测试，特别关注空参数列表、varchar/varstring、unsigned integer 与 decimal 宽度。
- 若改变内存估算，需同时审查 `entryMemoryUsage`、`PlanCacheValue::MemoryUsage` 的缓存策略、所有加减路径和 guard 测试。公开字段在首次 `MemoryUsage` 后发生容量变化不会自动使值内缓存失效，这是扩展可变字段时的兼容风险。
- 若补齐 Go 的指标或淘汰回调，应在锁内/锁外调用策略、回调重入和 panic 隔离上做显式设计，并放在独立测试文件中；不要把测试模块内嵌进本生产文件。
- 性能敏感改动应验证命中、替换与删除的复杂度，兼容桶很多时当前线性扫描会放大锁持有时间；正确性改动则必须保持“队首最新、队尾最旧”和内存总和两个核心不变量。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标文件已完整索引；`node --file pkg/planner/core/plan_cache_lru.rs` 读取了 1–182 行并列出两个文件关联。
- RustCodeGraph 对 `NewLRUPlanCache`、`LRUPlanCache` 的精确查询，以及对构造函数、`Get`、`Put`、`memoryControl` 等符号的 callers/callees 查询；图确认构造函数创建两个状态结构，`Put` 调用 `state`/`memoryControl` 并创建条目，`Get` 调用 `state`。同名通用方法产生的噪声未作为结论依据。
- RustCodeGraph `node` 读取：[`plan_cache_lru.rs`](./plan_cache_lru.rs)、[`plan_cache_utils.rs`](./plan_cache_utils.rs) 中 `PlanCacheValue`/`MemoryUsage`/`CheckTypesCompatibility4PC`、[`lib.rs`](./lib.rs) 的模块声明与再导出、[`plan_cache_lru_test.rs`](./plan_cache_lru_test.rs)、[`plan_cache_instance_test.rs`](./plan_cache_instance_test.rs)、[`plan_cache_lru.go`](./plan_cache_lru.go) 和 [`plan_cache_lru_test.go`](./plan_cache_lru_test.go)。
- Cargo 边界来自 [`Cargo.toml`](./Cargo.toml)：crate 名为 `astersql-planner-core`，库入口为 `lib.rs`，默认 feature 为空，`nextgen` 不条件控制本模块，`types-dependency` 指向 `../../types`，porting 元数据指向 Go 包 `pkg/planner/core`。
- `rg` 核验：`lib.rs` 在第 166 行声明模块、第 229 行再导出、第 368–369 行挂载独立测试；排除目标和专属测试后，仓库 Rust 源码中没有 `LRUPlanCache`/`NewLRUPlanCache` 引用；Go 的 `pkg/session/session.go` 第 443 行构造该缓存。
- 行为边界由 Rust 专属测试固定：默认容量与多桶容量、命中提升、按键删除与清空、缩容和容量 0 错误、内存计数、memory guard 驱逐、兼容桶替换跳过 guard。本文档任务不改变运行时代码，按计划不运行 Cargo。
