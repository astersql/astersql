# [`pkg/bindinfo/binding_handle.rs`](binding_handle.rs)

## 文件定位

本文件位于 `astersql-bindinfo` crate 的全局 SQL Binding 入口层。`pkg/bindinfo/lib.rs` 将 `binding_handle` 声明为私有模块后通过 `pub use binding_handle::*` 重新导出其公开项，因此其他 crate 看到的是 `astersql_bindinfo` 根上的 `BindingHandle`、`NewBindingHandle` 和相关常量，而不是直接访问子模块。

它不实现绑定匹配、持久化写入或候选计划评分，而是把三个已存在的能力对象组装为一个统一句柄：`BindingCacheUpdater`（缓存及存储同步）、`BindingOperator`（绑定写操作）和 `BindingPlanEvolution`（候选计划探索）。目标文件自身共 142 行，没有条件编译项，也没有启动线程、定时器或 owner 选举的代码。

`pkg/bindinfo/Cargo.toml` 说明该 crate 名为 `astersql-bindinfo`、库入口为 `lib.rs`、关闭自动测试发现和 doctest，并以 `package.metadata.porting.go-package = "pkg/bindinfo"` 指向 Go 对照包。目标文件只直接使用 crate 内再导出接口和 Rust 标准库；Cargo 中的 parser、hint、serde 等依赖由同 crate 的其他模块消费。

## 核心职责

1. 用 `BindingHandle: Send + Sync` 给上层提供缓存、写操作和计划演进三个组件的统一访问点。
2. 在 `NewBindingHandle` 中把同一个 `BindingStore` 同时接到缓存更新器和操作器，并把 `PlanRuntime` 接到计划演进组件，建立正确的共享关系。
3. 定义 Go 公共契约对应的租约、owner 元数据和绑定表锁 SQL 常量。
4. 通过 `GetScope` 与 `Stats` 描述 `last_plan_binding_update_time` 状态项；`Stats` 的值来自缓存更新器的同步水位 `LastUpdateTime()`。

需要注意边界：文件注释描述了刷新和 owner 的系统角色，但本文件只提供 `Lease`、`OwnerKey`、`Prompt` 等值，并不调度周期刷新或使用 etcd。RustCodeGraph 的 callers 查询未给 `NewBindingHandle` 和各访问器返回跨文件调用边；对仓库 Rust 源码的文本复核也只发现定义及测试/相邻模块对常量的使用，所以目前它是已导出的组装 API，尚无可证实的 Rust 应用主链接入点。

## 主要符号

- `Lease: Duration = 3s`：传给 `newBindingOperator`。在当前 Rust 路径中，它用于 `GCBinding` 计算“当前时间减十倍租约”的物理清理截止点；不能仅凭名称推断这里存在三秒定时任务。
- `OwnerKey` / `Prompt`：分别保存 bindinfo owner 的 etcd 路径和名称前缀。本文件外未发现 Rust 使用点，当前仅保留 Go 契约值。
- `BuiltinPseudoSQL4BindLock`：标识 `mysql.bind_info` 中充当锁行的伪 SQL。
- `LockBindInfoSQL`：通过更新同一伪记录，在调用方的悲观事务内竞争行锁。直接消费点是 `binding_operator.rs::lockBindInfoTable`。
- `StmtRemoveDuplicatedPseudoBinding`：删除重复伪记录、保留任意一行的维护 SQL。Go 注释进一步限定它用于 BR 同步后去重；当前 Rust 生产代码未发现调用点。
- `BindingHandle`：公开、对象安全的 `Send + Sync` trait。`cache`、`operator`、`evolution` 返回对内部 `Arc<dyn ...>` 的借用；`GetScope` 返回静态字符串；`Stats` 返回拥有所有权的字符串映射。
- `bindingHandle`：公开结构体但沿用非 Rust 风格名称；三个公开字段各持有一个 trait object 的 `Arc`。常规构造路径返回 `Arc<dyn BindingHandle>`，隐藏具体类型。
- `NewBindingHandle(store, runtime, cache_capacity)`：唯一构造函数，依次调用 `NewBindingCacheUpdater`、`newBindingOperator`、`newBindingAuto`。
- `lastPlanBindingUpdateTime`：`Stats` 映射使用的键，值为 `"last_plan_binding_update_time"`。
- `impl BindingHandle for bindingHandle`：三个组件访问器均为零逻辑借用；`GetScope` 固定返回 `"session"`；`Stats` 读取缓存同步水位并转成字符串。

## 执行流程

构造路径如下：

1. 调用方提供线程安全的 `Arc<dyn BindingStore>`、`Arc<dyn PlanRuntime>` 与以字节计的 `cache_capacity`。
2. `NewBindingCacheUpdater(Arc::clone(&store), cache_capacity)` 创建缓存同步器。相邻实现初始水位为 `BindingTime::default()`，内存缓存和配额使用传入容量。
3. `newBindingOperator(store, Arc::clone(&cache), Lease)` 取得原 `store` 的所有权，同时与句柄共享同一个缓存更新器。它的创建、删除和状态变更路径在持久化成功后调用缓存增量加载；GC 使用十倍 `Lease` 作为安全窗口。
4. `newBindingAuto(runtime)` 创建计划演进对象；该对象内部再创建计划生成器，并持有规则预测器和 LLM 预测器。
5. 三个对象装入 `bindingHandle`，再擦除具体类型为 `Arc<dyn BindingHandle>` 返回。构造过程没有 I/O，也不会预加载绑定；可能失败的初始化动作没有发生，因此函数不返回 `Result`。

运行时，调用方先通过 `cache()`、`operator()` 或 `evolution()` 选取具体能力，再在对应 trait 上执行操作。`Stats()` 是本文件唯一读取组件状态的路径：调用 `BindingCacheUpdater::LastUpdateTime()`，取 `BindingTime` 元组字段 `.0`，格式化为十进制字符串并生成单项 `HashMap`。

## 数据与状态

句柄自身不保存可变业务状态，只保存三个 `Arc`。共享关系中，缓存更新器与操作器指向同一个缓存实例，缓存更新器与操作器还共享同一个存储适配器；计划演进组件拥有单独的运行时适配器。

真正的同步状态在 `binding_cache.rs::bindingCacheUpdater`：`lastUpdateTime: Mutex<BindingTime>` 是增量加载水位，`memQuota: Mutex<i64>` 保存配额，底层缓存负责条目和容量。`Stats` 是水位的快照，不会触发加载或更新。

锁 SQL 常量只包含文本，不持有数据库锁。锁的生命周期由执行该 SQL 的外部事务决定；若调用方没有在预期的悲观事务中执行，常量本身无法提供互斥保证。`StmtRemoveDuplicatedPseudoBinding` 利用 `_tidb_rowid` 保留任意一条伪记录，其效果依赖 TiDB 系统表语义。

## 依赖与调用关系

上游边界：

- `pkg/bindinfo/lib.rs` 重新导出目标文件的公开符号，并在 `cfg(test)` 下装入独立的 `binding_handle_test.rs`。
- RustCodeGraph 报告目标文件被 `binding.rs`、`binding_handle_test.rs`、`binding_operator.rs`、`binding_operator_test.rs` 引用，但精确 callers/callees 查询没有为句柄构造函数或访问器生成跨文件调用边。
- 文本复核确认 `binding_operator.rs::lockBindInfoTable` 使用 `LockBindInfoSQL`；`binding_handle_test.rs` 使用两个 SQL 常量。未发现 Rust 生产代码调用 `NewBindingHandle`，也未发现 `OwnerKey`、`Prompt`、`StmtRemoveDuplicatedPseudoBinding` 的 Rust 生产消费点。

下游边界：

- `binding_cache.rs::NewBindingCacheUpdater` 消费 `BindingStore` 和容量，提供加载水位及缓存能力。
- `binding_operator.rs::newBindingOperator` 消费 `BindingStore`、同一缓存更新器和 `Lease`，提供创建、删除、状态修改、GC。
- `binding_auto.rs::newBindingAuto` 消费 `PlanRuntime`，提供 `ExplorePlansForSQL`。
- `utils.rs::BindingStore` 抽象读取、替换、逻辑删除、状态更新、GC 和使用信息保存；`binding_auto.rs::PlanRuntime` 抽象历史绑定、执行统计、实际执行和候选计划生成。

因此该文件是依赖注入/聚合边界，而非 SQL 请求入口。当前可验证的 Rust 依赖方向为“调用方 -> `BindingHandle` -> 三个子接口 -> `BindingStore`/`PlanRuntime` 适配器”；完整服务器如何提供这两个适配器在本文件及已发现调用边中未验证。

## 错误处理与边界

`NewBindingHandle`、组件访问器、`GetScope` 和 `Stats` 都不返回 `Result`。构造阶段只是内存分配和对象装配；存储或计划运行时错误留到各下游 trait 方法执行时，以 crate 的 `Result<T, BindError>` 传播。

`Stats` 内部的 `LastUpdateTime()` 会锁定缓存更新器的 mutex；相邻实现使用 `expect("update-time lock poisoned")`，所以锁中毒会 panic，而不是转成 `BindError`。`Arc` 分配理论上的进程级内存耗尽也不属于可恢复返回路径。

`cache_capacity` 在本函数中不校验，原样传给缓存构造器；容量为零或负数的具体行为属于 `binding_cache.rs`，扩展本函数时不应擅自改变该兼容语义。`GetScope` 忽略变量名，对任意输入都返回 `"session"`。`Stats` 也没有 Go 版本接收 session 参数和返回 error 的接口形态。

SQL 常量是跨实现公共契约，空格、大小写、换行和引号都可能被测试或外部工具依赖。修改它们时不能只验证语义等价。

## 并发与资源生命周期

`BindingHandle` 及三个下游 trait 都要求 `Send + Sync`，句柄通过 `Arc` 支持跨线程共享。访问器返回 `&Arc<_>`，调用方若需把组件带出借用期或交给任务，应显式 `Arc::clone`；它们不转移句柄内的所有权。

`bindingHandle` 没有自定义 `Drop` 或 `Close`。最后一个句柄及其克隆释放时，三个 `Arc` 字段依次减少引用计数；缓存组件自身提供 `BindingCache::Close`，但本文件不会自动调用它。调用方若依赖显式清理，必须通过 `cache()` 执行对应协议，不能假定释放句柄等价于 `Close`。

本文件没有线程、异步任务、channel 或事务对象。缓存水位的 mutex、底层缓存的锁以及存储事务都在下游模块管理。`Lease` 是一个不可变 `Duration` 常量，不是计时器；`OwnerKey` 也不会自动创建 owner manager。

## 与 Go 版本的对应关系

共同点来自 `pkg/bindinfo/binding_handle.go`：两版都保留 3 秒 `Lease`、相同的 owner/prompt/锁行/维护 SQL 文本，都把缓存更新、写操作、计划演进聚合到 `bindingHandle`，并暴露 `last_plan_binding_update_time` 会话级状态。

已验证的差异如下：

- Go 的 `Lease` 是可变包变量；Rust 是 `pub const Duration`，运行时不能覆盖。
- Go `BindingHandle` 通过接口嵌入直接提升三个子接口的方法；Rust 使用 `cache()`、`operator()`、`evolution()` 访问子 trait，没有把所有方法平铺到句柄 trait。
- Go 构造函数只接收 `DestroyableSessionPool`，三个组件复用 session pool；Rust 显式接收 `BindingStore`、`PlanRuntime` 和缓存容量，以适配器方式拆分存储与优化器运行时。
- Go 构造后调用 `variable.RegisterStatistics(h)`，使状态变量进入服务器注册表；Rust `NewBindingHandle` 没有对应注册调用，且仓库内未发现其生产调用者。因此只能确认 Rust 能生成统计映射，不能宣称它已出现在 `SHOW STATUS`。
- Go `GetScope` 返回 `vardef.ScopeSession`，Rust 返回字符串 `"session"`。Go `Stats` 接收 `SessionVars` 并返回 `(map[string]any, error)`；Rust 无参数、无错误，返回 `HashMap<String, String>`。
- Go 使用 `types.Time.String()` 作为状态值，Rust直接把 `BindingTime.0` 数值转为字符串。Go 测试 `TestBindingLastUpdateTime` 期望 SQL 时间文本，故两者展示格式并不等价。
- Go 注释明确重复伪记录可能由 BR 跨集群同步产生，且清理 SQL 只应由 BR 执行；Rust保留相同 SQL，但目标文件没有表达或强制该调用限制。

这些差异表明当前 Rust 文件是面向可注入适配器的独立 crate 实现，并非 Go 服务器接线的逐签名替换。后续对齐必须补充真实调用和测试证据，不能从 Go 接入情况推断 Rust 已接入。

## 扩展指南

- 增加跨三个子系统的统一能力时，先判断应放入 `BindingHandle`，还是归属 `BindingCacheUpdater`、`BindingOperator` 或 `BindingPlanEvolution`。业务操作优先放到职责明确的子 trait，句柄只承担组装和导航。
- 增加构造依赖时，应同步调整 `bindingHandle` 字段、`NewBindingHandle` 参数和直接构造测试，并检查是否仍让 operator 与 handle 共享同一缓存/存储；不要无意创建两份缓存导致读写视图分裂。
- 改变 `Lease` 会同时改变 Go 对齐值和 Rust GC 的十倍安全窗口，需在独立测试文件 `pkg/bindinfo/binding_operator_test.rs` 覆盖截止时间边界。
- 修改锁 SQL 时，同步更新 `pkg/bindinfo/binding_handle_test.rs` 的精确文本断言，并核对 `binding_operator.rs::lockBindInfoTable` 与 Go/BR 契约。测试逻辑应继续放在独立 `*_test.rs`，不要内嵌进生产文件。
- 扩展状态项时，在 `Stats` 中新增稳定键，并为作用域、初始值、格式和注册路径添加独立测试。若目标是服务器可见状态，还必须实现并验证 Rust 侧注册接线；仅修改映射不够。
- 若接入 owner 选举或周期刷新，应放到拥有任务生命周期和关闭信号的上层组件，不应让这个当前无生命周期管理的值对象悄然 spawn 后台任务。
- 性能上，三个访问器无额外分配；`Stats` 每次分配 `HashMap` 和两个字符串。高频采集若成为热点，可在保持返回所有权和并发快照语义的前提下优化。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`pkg/bindinfo` 下 45 个文件均在索引中。
- RustCodeGraph 源码/符号：完整读取 `pkg/bindinfo/binding_handle.rs`；查询 `BindingHandle`、`NewBindingHandle`、`NewBindingCacheUpdater`、`newBindingOperator`、`newBindingAuto`、`LastUpdateTime`、`BindingStore`；对 `NewBindingHandle` 及访问器运行 callers/callees，未返回跨文件静态调用边。
- RustCodeGraph 相邻实现：读取 `pkg/bindinfo/lib.rs`、`binding_cache.rs` 的同步器与构造段、`binding_operator.rs` 全部接口和实现、`binding_auto.rs` 的运行时/演进接口与构造段。
- crate 边界：读取 `pkg/bindinfo/Cargo.toml`，确认包名、库入口、依赖和 Go 包映射。
- Go 对照：读取 `pkg/bindinfo/binding_handle.go` 全文，并读取 `binding_operator_test.go::TestBindingLastUpdateTime`，确认注册、作用域和状态格式契约；Go 源码注释提供 BR 去重 SQL 的使用限制。
- Rust 测试：读取 `pkg/bindinfo/binding_handle_test.rs` 全文；它只精确断言 `LockBindInfoSQL` 和 `StmtRemoveDuplicatedPseudoBinding` 文本。相关操作行为另位于独立的 `binding_operator_test.rs`。
- 文本补查：在 Rust 源码中搜索句柄、构造函数及常量，确认 `LockBindInfoSQL` 的生产消费点和当前缺失的 Rust 服务器接入。该补查用于弥补图查询未生成的调用边，不替代图中的符号事实。
- 本任务是纯文档分析，按计划未运行 Cargo。最终结构检查要求本文恰有十一个固定二级标题；事实复核范围限于目标文件及上述直接模块、Go 对照和测试，未验证完整服务器运行时集成。
