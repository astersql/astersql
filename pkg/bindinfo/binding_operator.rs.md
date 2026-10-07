# `pkg/bindinfo/binding_operator.rs`

## 文件定位

本文件是 `astersql-bindinfo` crate 的全局 SQL 绑定写路径边界：它定义 [`BindingOperator`](binding_operator.rs) 接口，并用 `bindingOperator` 把持久化抽象 `BindingStore` 与内存缓存更新抽象 `BindingCacheUpdater` 组合起来。crate 根在 [`lib.rs`](lib.rs) 中声明并重新导出本模块；[`binding_handle.rs`](binding_handle.rs) 的 `NewBindingHandle` 创建缓存后调用 `newBindingOperator(store, cache, Lease)`，再把所得 trait 对象放入 `bindingHandle.BindingOperator`，因此上层通过 `BindingHandle::operator()` 取得它。

当前 Rust 生产代码搜索只发现上述组装和访问器，未发现对 `CreateBinding`、`DropBinding`、`SetBindingStatus`、`GCBinding` 的直接生产调用；这些方法目前有独立单元测试覆盖，但从 SQL 执行入口到它们的完整 Rust 接线未在索引中得到证明。文件属于 `pkg/bindinfo/Cargo.toml` 定义的 `astersql-bindinfo` 库；该 manifest 关闭自动测试发现，由 [`lib.rs`](lib.rs) 以 `#[cfg(test)] mod binding_operator_test` 显式接入测试。

## 核心职责

- 用 `BindingOperator: Send + Sync` 固定创建、逻辑删除、状态切换和物理回收四类生命周期操作的统一接口。
- 在默认实现中维护“先存储成功，后增量重载缓存”的顺序。缓存不是直接增删单项，而是统一调用 `LoadFromStorageToCache(false, false)`，让时间水位和删除墓碑由缓存同步逻辑处理。
- 创建前调用 `prepareHints` 校验/解析绑定 SQL，并统一数据库名及时间元数据。
- 把 SQL 表锁动作封装为 `lockBindInfoTable`；但当前四个默认操作均未调用该函数，跨节点串行化是否成立取决于具体 `BindingStore` 实现，而该实现在本文件及当前生产引用中未出现。
- 依据 `lease` 计算已删除记录的安全 GC 截止时间。

## 主要符号

- `pub trait BindingOperator: Send + Sync`：公开的线程安全接口。`CreateBinding(&dyn BindingValidator, Vec<Binding>) -> Result<()>` 创建或覆盖；`DropBinding(&[String]) -> Result<u64>` 返回逻辑删除数；`SetBindingStatus(&str, &str) -> Result<bool>` 返回是否实际改变；`GCBinding() -> Result<u64>` 返回物理清理数。
- `pub struct bindingOperator`：默认实现，持有 `Arc<dyn BindingStore>`、`Arc<dyn BindingCacheUpdater>` 和 `Duration lease`。结构体名保持 Go 移植命名，crate 级 lint 配置允许非驼峰形式。
- `newBindingOperator(...) -> Arc<dyn BindingOperator>`：默认构造器，隐藏具体实现并共享存储、缓存对象。
- `TestTimeLagInLoadingBinding`：与 Go 同名的测试键常量。当前 Rust 生产实现不读取它，Rust 对应测试中的旧移植片段也不是可执行的该功能验证，因此不能视为已支持时间滞后注入。
- `impl BindingOperator for bindingOperator`：四个写操作的实际实现。
- `lockBindInfoTable(&dyn BindingSqlContext) -> Result<()>`：执行 `LockBindInfoSQL`，即更新 `mysql.bind_info` 内置伪记录以取得悲观行锁；只丢弃结果行，不管理事务生命周期。

## 执行流程

`CreateBinding` 的流程是：逐条调用 `prepareHints`；任一条失败即提前返回，尚未写存储。随后每条记录分别取得 `BindingTime::now()`，把 `Db` 做 ASCII 小写化，同时覆盖 `CreateTime` 与 `UpdateTime`，克隆进 `Vec<Arc<Binding>>`。全部准备完成后调用 `BindingStore::replace_bindings`，成功才调用 `LoadFromStorageToCache(false, false)`。空输入会把空切片交给存储并仍尝试重载缓存；本文件没有把空摘要转为 SQL `NULL`，具体列编码属于存储实现责任。

`DropBinding` 先拒绝空摘要列表并返回精确错误 `sql digest is empty`。非空时以同一个当前时间调用 `mark_deleted` 批量写入墓碑，之后增量重载缓存并返回存储报告的行数。若存储失败，不重载；若重载失败，持久化删除可能已经成功，但整个方法返回错误。

`SetBindingStatus` 仅对目标状态 `enabled` 或 `disabled` 调用 `BindingStore::set_status`。其他字符串直接得到 `false`，但仍重载缓存；合法状态即使没有命中记录也同样重载。存储错误会阻止重载，缓存错误则覆盖成功路径的返回结果。

`GCBinding` 将租约转成微秒；转换溢出时使用 `i64::MAX`，再以饱和乘法计算十倍租约、以饱和减法计算 `now - 10 * lease`，最后调用 `gc_deleted_before(cutoff)`。该流程不刷新缓存，因为目标只应是已传播完成的删除墓碑。

`lockBindInfoTable` 单独调用 `BindingSqlContext::execute(LockBindInfoSQL, &[])`。调用方若要获得与 Go 相同的跨节点互斥，必须在一个保持到写操作结束的悲观事务中调用；函数自身不会开启、提交或回滚事务。

## 数据与状态

操作器自身没有可变内存状态：`store` 与 `cache` 是共享的 trait 对象，`lease` 在构造后按值保存。共享实现的内部同步由 `BindingStore: Send + Sync` 和 `BindingCacheUpdater: BindingCache` 各自负责。

绑定的持久化/缓存状态以 `Binding`、SQL 摘要和 `BindingTime` 传递。`BindingTime` 是 UTC Unix 纪元微秒数；创建时每条绑定独立取时，因此批量中时间可能不同。删除则整批共享一个时间点。删除不是立即移除，而是由 `mark_deleted` 写入墓碑，随后缓存增量加载；GC 只清理截止时间之前的墓碑。

重要不变量是：缓存加载只发生在对应持久化调用成功之后。不过存储成功与缓存刷新不是一个原子操作；刷新失败时会出现“存储已更新、调用返回错误、当前缓存尚未同步”的暂态，需要后续重载恢复。`BindingStore` 文档要求每个批量方法内部原子，但不保证本文件跨存储与缓存的原子性。

## 依赖与调用关系

上游组装链为 `NewBindingHandle` → `newBindingOperator` → `bindingOperator`，随后 `BindingHandle::operator()` 暴露 `Arc<dyn BindingOperator>`。RustCodeGraph 对四个实现方法未给出生产调用者，仓库范围的 Rust 文本搜索也只找到该组装链和测试，因此完整 SQL DDL/执行器调用链当前未验证。

下游关系如下：

- `CreateBinding` → `prepareHints` → 解析/验证绑定 SQL；随后 → `BindingStore::replace_bindings` → `BindingCacheUpdater::LoadFromStorageToCache(false, false)`。
- `DropBinding` → `BindingStore::mark_deleted` → 缓存增量重载。
- `SetBindingStatus` →（仅合法状态）`BindingStore::set_status` → 缓存增量重载。
- `GCBinding` → `BindingStore::gc_deleted_before`。
- `lockBindInfoTable` → `BindingSqlContext::execute`，SQL 常量定义于 [`binding_handle.rs`](binding_handle.rs) 的 `LockBindInfoSQL`。

`pkg/bindinfo/Cargo.toml` 直接依赖 parser 与 hint 相关内部 crate，以及 `serde`/`serde_json`；本文件不直接引用这些外部 crate，它通过本 crate 的 `Binding`、`prepareHints` 和适配 trait 间接使用解析、提示及持久化能力。

## 错误处理与边界

所有错误统一为 crate 的 `BindError`。创建时，校验、存储、缓存三阶段均用 `?` 原样向上传播；由于准备发生在调用方传入的本地 `Vec<Binding>` 上，校验中途失败不会写存储。删除明确拒绝空列表；创建空列表、空 SQL 摘要以及空状态摘要没有本地拒绝逻辑，应由下游契约决定。

状态修改不把未知 `newStatus` 当作错误，而是“不写存储、返回 `false`、仍刷新缓存”，这与 Go 版以空旧状态条件匹配不到行的外部结果一致。`GCBinding` 对超大 `Duration` 使用转换和算术饱和，避免 panic 或截止时间回绕。

本文件没有重试、补偿或事务协调。尤其存储成功而缓存加载失败时，调用者不能据返回错误断言存储未改变。`lockBindInfoTable` 也不验证受影响行数；伪锁记录及事务必须由更外层保证。

## 并发与资源生命周期

`BindingOperator`、`BindingStore` 和缓存接口均要求 `Send + Sync`，允许多个会话共享同一个 `Arc<dyn BindingOperator>`。默认实现没有本地 `Mutex`，并发安全完全建立在存储和缓存适配器的内部同步、批量原子性及时间排序之上。

与 Go 实现不同，Rust 的四个操作没有会话池获取/归还/销毁流程，也没有在事务中调用 `lockBindInfoTable`。独立锁函数只有在外层持有同一事务上下文时才能把行锁延续到写操作结束。若未来存储适配器落地到 `mysql.bind_info`，应同时验证：批量方法是否在单事务内、锁是否覆盖读改写窗口、失败是否回滚、缓存刷新是否发生在提交之后。

缓存刷新采用增量加载，依赖 `BindingCacheUpdater` 的更新时间水位线和墓碑合并规则。GC 的十倍租约窗口用于给其他节点观察删除留时间，但当前 `lease` 是构造参数，而 Go 版直接使用全局 `Lease`。

## 与 Go 版本的对应关系

接口的四类操作、默认实现名称、先写存储后刷新缓存、空删除列表错误、合法状态集合、十倍租约 GC 和伪记录锁 SQL 均对应 [`binding_operator.go`](binding_operator.go)。Rust 的 `operator_parity` 测试验证了数据库名小写化/时间更新、缓存重载而非直接修改、未知状态不写存储、超大租约饱和及锁 SQL。

仍存在应显式保留的差异：

- Go 版通过 `DestroyableSessionPool`、`callWithSCtx(..., true)` 和 `lockBindInfoTable` 在悲观事务中直接执行系统表 SQL；Rust 默认操作只调用抽象 `BindingStore`，没有可见的锁调用或具体生产存储实现。
- Go 创建流程先将同 `original_sql` 旧记录标记删除，再逐条插入，并检查 `SHOW WARNINGS`；Rust 把覆盖语义整体委托给 `replace_bindings`，本文件无法证明 SQL 细节、警告处理、空 digest 到 `NULL` 的转换。
- Go 为 `CreateGlobalBindingNthFail` 和 `TestTimeLagInLoadingBinding` 提供测试注入；Rust 本文件没有对应执行逻辑，后一个名称仅保留为常量。
- Go 删除多个摘要时人为生成严格递增的微秒时间；Rust 一次批量调用共享单个 `BindingTime`。Go 使用 Unicode `strings.ToLower`，Rust 使用 `make_ascii_lowercase`。
- Go 的状态迁移兼容 `using → disabled`，由 SQL 的旧状态过滤条件表达；Rust 只把目标状态交给存储，能否保留该兼容性取决于 `BindingStore::set_status`。
- Go 的 `GCBinding` 只返回错误；Rust 额外返回清理行数。Rust 对极端租约定义了饱和行为，Go 没有对应显式处理。

Go 的 [`binding_operator_test.go`](binding_operator_test.go) 还覆盖 SQL 层启用/禁用对计划命中的影响、缓存中缺失绑定时的状态更新，以及时间滞后注入；这些属于端到端语义，不能由当前 Rust 单元适配器测试替代。

## 扩展指南

新增写操作时，应先扩展 `BindingOperator`，再在 `bindingOperator` 中把持久化调用与缓存同步顺序写清，并同步修改 `BindingHandle` 暴露面（若需要）和独立的 [`binding_operator_test.rs`](binding_operator_test.rs)，不要把测试嵌入生产文件。新方法若改动系统表多行，优先扩展 `BindingStore` 的单个原子批量方法，而不是在操作器中拼接多个非原子调用。

补齐真实持久化适配器时，必须对照 Go 的锁事务、旧记录墓碑、严格时间排序、SQL `NULL` 编码、`SHOW WARNINGS` 和状态迁移过滤；这些不能仅靠现有 `RecordingStore` 证明。接入 `lockBindInfoTable` 时还要让锁与写共用事务上下文，单独提前执行没有互斥保证。

更改缓存策略时需保留“持久化先于缓存”的不变量，并为“存储成功、缓存失败”定义可恢复行为。更改 GC 应同步审查 `Lease`、缓存增量水位和多节点最大传播延迟。性能风险主要在每次写后触发一次增量加载；批量创建当前还会克隆每个 `Binding`。兼容风险集中在 `using` 历史状态、ASCII 与 Unicode 数据库名归一化、批量删除时间排序及返回值差异。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件；`files --filter pkg/bindinfo` 确认目标及相邻实现已索引；`node --file pkg/bindinfo/binding_operator.rs` 读取完整 155 行和文件使用关系。
- RustCodeGraph `query` 确认 `BindingOperator`、`bindingOperator`、`newBindingOperator`、`lockBindInfoTable` 及四个方法的 Rust/Go 定义；`callees` 确认 Rust 实现到 `replace_bindings`、`mark_deleted`、`set_status`、`gc_deleted_before`、`LoadFromStorageToCache`、`execute` 的边。`callers` 未返回生产调用者，因此又以仓库范围 Rust 搜索核实只存在 `binding_handle.rs` 的构造/访问接线和测试引用。
- 已读生产/配置路径：[`binding_operator.rs`](binding_operator.rs)、[`binding_handle.rs`](binding_handle.rs)、[`binding.rs`](binding.rs)、[`binding_cache.rs`](binding_cache.rs)、[`utils.rs`](utils.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml) 和 Go 对照 [`binding_operator.go`](binding_operator.go)。本包没有 `doc.go`。
- 已读测试路径：[`binding_operator_test.rs`](binding_operator_test.rs) 的 `operator_parity` 六项测试，以及 [`binding_operator_test.go`](binding_operator_test.go) 的状态切换、跨节点缓存缺失和时间滞后用例。测试仅作为行为证据，本纯文档任务未运行 Cargo 或 Go 测试。
- 交付结构以任务指定命令校验，要求本文件存在且恰好包含上述十一个固定二级标题；同时人工复核当前实现、Go 差异和未验证接线均有明确边界。
