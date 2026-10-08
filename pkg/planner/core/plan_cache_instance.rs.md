# `pkg/planner/core/plan_cache_instance.rs`

## 文件定位

本文件实现 AsterSQL Rust 侧的实例级计划缓存。它位于 `astersql-planner-core` crate；`pkg/planner/core/Cargo.toml` 把该 crate 的入口指定为 `lib.rs`，`pkg/planner/core/lib.rs` 通过 `mod plan_cache_instance` 编译本模块，并用 `pub use plan_cache_instance::*` 导出其公开 API。文件没有 feature gate 或条件编译项，生产构建会包含 `InstancePlanCache` 和 `NewInstancePlanCache`；独立测试则由 `lib.rs` 中的 `#[path = "plan_cache_instance_test.rs"]` 接入。

运行时入口位于 `pkg/session/runtime.rs::runtime_instance_plan_cache`：它按 Domain 标识在进程级弱引用表中复用 `Arc<InstancePlanCache>`。`pkg/session/runtime/session.rs::ConcreteSessionInner::instance_plan_cache` 持有该共享缓存，因此它不是单个 session 私有的 LRU，而是同一 Domain 下多个会话共同使用的 prepared-plan 缓存。当前运行时工厂用 `(i64::MAX, i64::MAX)` 创建缓存，实际软/硬限制的动态接线不能仅从本文件推断；本文件只负责执行传入和后来由 `SetLimits` 设置的限制。

## 核心职责

- 以字符串缓存键分桶：`InstancePlanCacheState::items` 是 `HashMap<String, Vec<InstancePlanCacheEntry>>`，同一键下允许保存参数类型不兼容的多份计划。
- 以 `CheckTypesCompatibility4PC` 判断桶内计划是否可复用。空参数类型列表视为兼容；非空时还检查数量、类型、字符集、排序规则、整数 unsigned 标志以及 decimal 精度/小数位。
- 在 `Put` 时拒绝超过硬内存上限的条目，也拒绝同一键下已有参数类型兼容项的重复插入。
- 在 `Get` 命中时刷新单调访问序号，在 `Evict(false)` 时按最久未使用顺序释放估算数量的条目，使占用趋近软上限；`Evict(true)` 清空所有条目。
- 维护可查询的计划数、估算内存占用和软/硬上限，并通过 `Arc<PlanCacheValue>` 把只读计划值安全地交给并发调用者。

## 主要符号

- `InstancePlanCacheEntry`：内部条目，保存 `Arc<PlanCacheValue>` 与 `last_used: u64`。类型私有，调用者不能绕过缓存修改淘汰元数据。
- `InstancePlanCacheState`：锁内状态，包含分桶映射 `items`、累计估算字节数 `memory`、条目数 `plans` 和单调时钟 `use_clock`。
- `InstancePlanCache`：公开缓存类型。`state: Mutex<_>` 串行化结构、计数和 LRU 元数据修改；`soft`、`hard` 用 `AtomicI64` 支持不取得状态锁地更新限制。
- `NewInstancePlanCache(soft, hard) -> InstancePlanCache`：以空状态和给定限制构造具体缓存值。它返回具体类型而不是 `pkg/sessionctx/context.rs::InstancePlanCache` trait object；该 trait 的方法签名也与此固有实现并不相同，不能把两者视为已经接线的同一接口。
- `state()`：取得互斥锁；锁被 poison 时通过 `PoisonError::into_inner` 继续使用原状态。
- `touch()`：用 `saturating_add(1)` 推进 `use_clock`，避免整数溢出 panic。
- `Get(key, parameter_types)`：找到首个参数类型兼容的条目，刷新其 `last_used`，返回计划 `Arc` 的克隆；键不存在或没有兼容项时返回 `None`。
- `Put(key, value) -> bool`：接受任何可转为 `Arc<PlanCacheValue>` 的值，计算 `MemoryUsage`，检查硬限制和重复兼容项，成功时更新计数并追加到键对应的向量。
- `All()`：克隆并返回所有条目的 `Arc`；返回顺序取决于 `HashMap` 遍历和桶内顺序，没有稳定排序承诺。
- `Evict(all) -> usize`：返回实际淘汰条数。部分淘汰按平均条目大小估算目标数量，再按 `last_used` 从小到大选 victim；全部淘汰以当前 `plans` 为目标。
- `MemUsage()`、`Size()`、`GetLimits()`、`SetLimits()`：状态和限制的观测/配置接口。

## 执行流程

prepared SELECT 的主链可由 `pkg/session/runtime/planning.rs` 复核：

1. `ConcreteSession::PlanPreparedKVPhysical` 用 `NewPlanCacheKey` 构造计划缓存键，并把键字节编码成十六进制 `instance_key`；同时从执行参数推导 `FieldType` 列表。
2. 它调用 `InstancePlanCache::Get(&instance_key, &parameter_types)`。命中时，本文件先锁住整个状态，定位字符串键对应的桶，再调用 `CheckTypesCompatibility4PC` 顺序寻找第一份兼容计划。成功后递增 `use_clock`、刷新条目 `last_used`，并克隆 `Arc` 返回。
3. 上层从命中的 `PlanCacheValue::Plan` 恢复物理计划；未命中时正常构建和优化计划，并由 `CachedPlan::try_capture` 生成拥有型快照。
4. 执行结束时，`FinishPreparedKVPhysicalPlan` 对命中值更新运行时统计；若本次是未命中且计划可缓存，则调用本文件的 `Put`。`Put` 在锁外先把值转为 `Arc` 并取得已缓存的内存估算，随后持锁原子地完成限制/重复检查和结构更新。
5. 内存仲裁不能满足编译配额时，`pkg/session/runtime/control.rs::clear_memory_sensitive_plan_cache` 调用 `Evict(true)` 清空实例缓存，并同时清理其他会话计划状态。

部分淘汰流程是：读取软限制；占用低于软限制则直接返回；以 `memory / plans` 计算平均条目大小，以向上取整的 `(memory - soft) / average` 得出目标条数；收集所有 `(last_used, Arc)` 并排序；用 `Arc::ptr_eq` 在各桶中移除选中的对象；最后删除空桶并同步扣减 `memory`、`plans`。这是“按平均大小估算条数”，不保证不同大小条目淘汰后精确落到软限制以下。

## 数据与状态

缓存键的业务组成由上游 `NewPlanCacheKey` 决定，本文件只把它当作不透明 `String`。同键的 `Vec` 是参数类型桶：兼容性而非 `FieldType` 的逐字段完全相等决定命中和重复。`Get` 返回桶中第一个兼容项，所以若兼容关系未来变得非等价或多个不兼容插入项后来同时兼容，插入顺序会影响选择结果。

`PlanCacheValue` 定义在 `pkg/planner/core/plan_cache_utils.rs`。计划快照和解析元数据在插入后按共享只读对象使用，而执行次数、处理键数等运行时统计由该类型自身的原子字段更新。其 `MemoryUsage()` 缓存“计划估算 + 结构体 + 字符串容量 + 输出列 + 参数类型”的值；本文件的 `memory` 是这些值的和，不包含 `HashMap`、键字符串、`Vec`、`Arc` 和缓存状态本身的分配开销，因此是配额估算而非进程实际 RSS。

三个计数不变量应在持有 `state` 锁时成立：`plans` 等于所有桶条目总数；`memory` 等于所有条目 `PlanCacheValue::MemoryUsage()` 之和；空桶只可能在一般操作的短暂内部阶段存在，`Evict` 完成后会清理。`use_clock` 只在成功插入或命中时推进；未命中和被拒绝的插入不改变 LRU 顺序。

软/硬限制与缓存状态分开存储。`SetLimits` 只替换两个原子值，不主动淘汰既有计划，也不校验 `soft <= hard`、非负数或与当前占用的关系。调用者若降低限制，需要显式触发 `Evict`；超过硬限制的既有内容也不会因此被自动删除。

## 依赖与调用关系

直接下游依赖如下：

- `std::collections::HashMap`：字符串键到参数类型桶的映射。
- `std::sync::{Arc, Mutex, MutexGuard}`：共享计划所有权和缓存状态串行化。
- `std::sync::atomic::{AtomicI64, Ordering}`：限制的并发读写；读取使用 Acquire，写入使用 Release。
- `crate::PlanCacheValue`：缓存载荷与内存估算来源。
- `crate::CheckTypesCompatibility4PC`：`Get`/`Put` 的参数类型复用规则。
- `types-dependency`：公开 `Get` 签名中的 `metadata::FieldType`，对应 `Cargo.toml` 的本地 `astersql-types` 依赖。

直接上游中，`pkg/planner/core/lib.rs` 负责模块导出；`pkg/session/runtime.rs::runtime_instance_plan_cache` 负责按 Domain 构造和共享；`pkg/session/runtime/planning.rs` 的 prepared KV 规划/收尾路径调用 `Get` 和 `Put`；`pkg/session/runtime/control.rs::clear_memory_sensitive_plan_cache` 调用 `Evict(true)`。RustCodeGraph 对 `NewInstancePlanCache` 还记录了四个本文件独立测试调用者，对 `Evict` 记录了生产调用者 `clear_memory_sensitive_plan_cache`。`All`、`MemUsage`、`Size`、`GetLimits`、`SetLimits` 当前主要由单元测试直接覆盖；不要仅因公开导出就假设它们均有生产调用。

## 错误处理与边界

本模块没有 `Result` 型 API，也不记录淘汰原因。正常的容量或匹配失败通过 `Put == false`、`Get == None`、`Evict == 0` 表达；调用者若需要区分“硬上限”和“重复兼容项”，当前接口不能提供原因。相比 Go 的 `Evict` 返回 `(detailInfo, numEvicted)`，Rust 仅返回数量。

锁 poison 不会传播 panic：`state()` 取得被 poison 的内部值继续运行。这保证缓存仍可访问，但不能自动证明发生 panic 时中间状态仍满足计数不变量；后续维护者若在持锁更新过程中加入可能 panic 的操作，应特别审查恢复策略。

重要边界包括：

- `Get` 对不存在的键或不兼容参数类型返回 `None`；兼容性函数把任一侧空类型列表当作通配兼容。
- `Put` 只在“插入后占用严格大于 hard”时拒绝，恰好等于 hard 可以插入；它不会为了新条目自动触发软限制淘汰。
- `Evict(false)` 在 `memory < soft` 时跳过；在恰好等于 soft 时最终因 `to_release <= 0` 返回零。
- 零条目、非正平均大小或非正待释放量都会使部分淘汰返回零。当前正常 `PlanCacheValue::MemoryUsage` 会包含结构体开销，但构造器和 `SetLimits` 本身不验证负限制。
- `state.memory + mem`、淘汰向上取整表达式以及累计计数依赖合法、合理的内存估算。代码只对 `use_clock` 使用饱和加法，未对这些 `i64` 算术做溢出保护。
- `All` 返回 `Arc` 克隆，因此淘汰只移除缓存所有权；外部仍持有的计划值会继续存活，`MemUsage` 则只统计仍在缓存中的条目。

## 并发与资源生命周期

`InstancePlanCache` 的结构状态由一把 `Mutex` 保护，所有 `Get`、`Put`、`All`、`Evict`、`MemUsage`、`Size` 都在这把锁下访问。这样可让桶、计数和 LRU 元数据作为一个事务式临界区更新，并使类型满足 `Send + Sync`；代价是命中读取也会写 `last_used` 并独占全局锁，高并发下可能形成热点。

`soft` 和 `hard` 是原子字段，允许 `SetLimits` 与缓存操作并行。一次 `Put` 或 `Evict` 只读取一次相关限制，因此并发的限制变更对该次操作可能在下一次调用才可见；Acquire/Release 保证原子值发布，但并不把两个限制作为不可分割的一对更新。并发读取 `GetLimits` 也可能观察到来自两次配置更新的混合组合。

条目由 `Arc<PlanCacheValue>` 管理：缓存插入取得一份强引用，`Get`/`All` 再克隆；淘汰或缓存整体释放只减少缓存持有的引用，不强制销毁仍在执行中的计划。`runtime_instance_plan_cache` 外层表保存 `Weak`，最后一个会话/调用者释放强引用后，缓存及仍由它独占的条目会自然析构；下一会话可为同一 Domain 新建缓存。

LRU 使用 `u64` 单调逻辑时钟而非墙钟，避免同一系统时钟 tick 内访问顺序不确定。时钟饱和到 `u64::MAX` 后后续访问拥有相同时间戳，排序的精确新旧关系不再有保证，但不会因加法溢出 panic。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/plan_cache_instance.go`，独立测试分别是 `pkg/planner/core/plan_cache_instance_test.go` 与 `pkg/planner/core/plan_cache_instance_test.rs`。两版保留的核心语义包括：按字符串键分组、同键按参数类型兼容性选计划、硬限制拒绝写入、重复兼容计划不重复插入、软限制触发近似 LRU 淘汰、全部清空、内存/数量计数以及可动态设置限制。

实现并非机械同构：

- Go 用 `sync.Map` 加带哨兵头的原子单链表，插入用 CAS，并用 `inEvict` 与独立 `evictMutex` 协调淘汰；Rust 用一把 `Mutex<InstancePlanCacheState>` 串行化所有结构访问，没有 lock-free/CAS 路径。
- Go 用原子 `time.Time` 记录真实时间并按阈值删除所有 `lastUsed <= threshold` 的节点；同一时间戳可能使实际删除数多于估算数。Rust 用单调序号排序后精确取 `target` 个 `Arc` victim，结果更确定。
- Go `Put` 在淘汰中可直接失败，也可能因 CAS 竞争失败；Rust 持锁后不存在这两种竞争失败，`false` 仅表示硬限制或兼容重复项。
- Go 构造器返回 `sessionctx.InstancePlanCache` 接口，`Evict` 还返回说明文本；Rust 构造器返回具体类型，固有 API 的类型化参数和返回值与 Rust `sessionctx` trait 不同。
- Go 的 domain 初始化会把构造函数接到全局钩子并按配置管理缓存；Rust 当前由 session runtime 自己按 Domain 建立弱引用共享表，且工厂传入无限限制。配置到 `SetLimits`/后台部分淘汰的完整等价接线在所读直接证据中未验证，不能声称已经与 Go 完全一致。

Rust 独立测试覆盖基本命中、硬限制、重复拒绝、确定性 LRU、不同键、全部清空、限制 API、交错访问、`Send + Sync` 和八线程并发读写。Go 测试还强调随机并发读取/写读和淘汰后空头节点清理；Rust 的 `HashMap` 表示不需要哨兵头节点，但修改并发模型时仍应保持相同可见行为。

## 扩展指南

- 修改键或参数复用规则时，优先改上游键生成或 `CheckTypesCompatibility4PC`，并同步检查 `Get` 与 `Put`，避免命中规则和去重规则分叉。至少扩展 `pkg/planner/core/plan_cache_instance_test.rs`；参数类型细节还应扩展 `pkg/planner/core/plan_cache_utils_aster_unit_test.rs` 或 `pkg/planner/core/tests/prepare/prepare_test.rs`。
- 新增删除单键、返回拒绝原因、淘汰详情或指标时，应在 `InstancePlanCache` 的公开 API 上明确语义，并核对 `pkg/sessionctx/context.rs` 的 trait 是否需要统一；不要只改 Go 风格名称而留下两套不兼容接口。
- 调整内存核算时，应同时核对 `PlanCacheValue::MemoryUsage` 与本文件计数更新。若把键、桶或容器开销计入，插入和淘汰必须使用同一公式，测试不要假定传入的 `plan_memory_usage` 就是条目总占用。
- 改变淘汰算法时，应保持“最近访问受保护”“全部淘汰后 Size/MemUsage 为零”“外部 `Arc` 不被强制失效”三个行为，并增加不同条目大小、软限制等于当前占用、限制动态降低以及访问时钟并列/饱和的独立测试。
- 若为生产补齐动态限制或后台部分淘汰，应从 `runtime_instance_plan_cache`、全局变量更新路径和内存仲裁入口接线，并验证同一 Domain 共享、不同 Domain 隔离。该工作涉及本文件之外的生命周期，不应通过在缓存内部读取全局配置来隐式耦合。
- 若为性能将实现改回细粒度锁或 lock-free 结构，必须保持计数与桶的并发不变量，并用独立 Rust 测试覆盖重复并发插入、读写与淘汰交叠、限制并发更新；不能把测试逻辑内嵌到本生产文件。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件可按行完整读取。
- RustCodeGraph `node --file pkg/planner/core/plan_cache_instance.rs`：核对 214 行源文件中的三个内部/公开结构、构造器、全部固有方法及无条件编译事实。
- RustCodeGraph `node InstancePlanCache`、`node NewInstancePlanCache`：核对 Rust/Go/sessionctx 三种同名定义，并确认 Rust 构造器的四个独立测试调用者。
- RustCodeGraph 对限定符号 `pkg/planner/core/plan_cache_instance.rs::Get`、`::Put`、`::Evict` 的查询：确认 `Get -> state`、`Put -> state/touch`，以及生产调用边 `clear_memory_sensitive_plan_cache -> Evict`。对 `Get`/`Put` 的生产调用由 `pkg/session/runtime/planning.rs` 直接源码核对补足。
- `pkg/planner/core/Cargo.toml` 与 `pkg/planner/core/lib.rs`：核对 crate 入口、`types-dependency`、模块声明、公开再导出和独立测试装配。
- `pkg/session/runtime.rs`、`pkg/session/runtime/session.rs`、`pkg/session/runtime/planning.rs`、`pkg/session/runtime/control.rs`：核对按 Domain 共享、session 持有、prepared 计划命中/写入和内存压力清空主链。
- `pkg/planner/core/plan_cache_utils.rs`：核对 `PlanCacheValue`、`MemoryUsage` 和 `CheckTypesCompatibility4PC` 的真实语义。
- `pkg/planner/core/plan_cache_instance.go`、`pkg/planner/core/plan_cache_instance_test.go`、`pkg/planner/core/plan_cache_instance_test.rs`：核对 Go/Rust 实现差异、容量与匹配边界、淘汰行为和并发测试意图。

本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定的 `test -f` 加固定标题计数命令验证文档存在且恰有 11 个规定二级章节，并人工检查未把未验证的配置接线写成已支持事实。
