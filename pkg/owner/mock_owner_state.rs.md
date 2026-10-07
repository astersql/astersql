# `pkg/owner/mock_owner_state.rs`

源文件：[`mock_owner_state.rs`](mock_owner_state.rs)

## 文件定位

本文件属于 `astersql-owner` crate。`pkg/owner/Cargo.toml` 将 `lib.rs` 设为 crate 入口，`lib.rs` 以公开模块 `mock_owner_state` 装配并再导出本文件的公开项。它不是基于 etcd 的生产选主实现，而是 `pkg/owner/mock.rs` 中本地 `MockManager` 的进程内共享状态后端，用于无 etcd 的模拟竞选和 Owner 生命周期测试。

文件维护的是“某个存储实例、某类 Owner 当前由哪个节点持有”这一最小状态。真实异步竞选循环、监听器通知、取消和退位流程在 `mock.rs`；本文件只负责同步、原子的状态查询与条件更新。

## 核心职责

- 用 `MockGlobalStateEntry` 提供惰性初始化、进程级唯一的模拟 Owner 登记表。
- 用 `(store_id, owner_type)` 组成复合键，使同一存储的 DDL、stats 等 Owner 类型以及不同存储之间互不干扰。
- 在同一把 `Mutex` 下实现读取、抢占、条件释放和身份比较，避免并发的 `MockManager` 同时成功成为同一键的 Owner。
- 保持 Go `pkg/owner/mock_owner_state.go` 的字符串零值语义：不存在的条目读取为空串，而不是错误或 `Option`。

它不负责计时、重试、选主公平性、持久化、跨进程一致性或租约失效；这些能力不能由该内存表推断为已支持。

## 主要符号

- `pub static MockGlobalStateEntry: LazyLock<MockGlobalState>`：进程级默认实例。首次访问时通过 `MockGlobalState::default` 创建，随后由所有 `MockManager` 共享。
- `OwnerKey`：私有复合键，含 `store_id: String` 和 `owner_type: String`；派生 `Eq`、`Hash`、`PartialEq` 以作为 `HashMap` 键，并派生 `Clone` 供写路径取得自有键。
- `pub struct MockGlobalState`：持有 `Mutex<HashMap<OwnerKey, String>>`。结构公开以允许构造独立测试状态，但字段私有，调用者不能绕过同步规则直接修改映射。
- `MockGlobalState::OwnerKey(...) -> MockGlobalStateSelector<'_>`：把两个可转换为 `String` 的维度固化为选择器。返回值借用状态表，因此不会比状态表活得更久。
- `pub struct MockGlobalStateSelector<'a>`：包含对状态表的借用和一个复合键；公开类型、私有字段，只能由 `OwnerKey` 工厂构造。
- `GetOwner(&self) -> String`：返回当前 Owner ID 的克隆；键不存在时返回空串。
- `SetOwner(&self, owner: impl Into<String>) -> bool`：仅当当前值为空串时写入候选 ID 并返回 `true`，否则不修改并返回 `false`。
- `UnsetOwner(&self, owner: &str) -> bool`：仅当参数等于当前 Owner ID 时清空值并返回 `true`，否则不修改并返回 `false`。
- `IsOwner(&self, owner: &str) -> bool`：在锁内比较参数与当前值；缺失键按空串比较。

文件没有 trait、异步函数、条件编译项或自建后台任务。

## 执行流程

1. `NewMockManager` 在 `pkg/owner/mock.rs` 中保存节点 ID、存储 UUID（缺省为 `mock_store_id`）和 Owner key。
2. `MockManager::selector` 每次操作时调用 `MockGlobalStateEntry.OwnerKey(store_id, key)`，得到定位到唯一复合键的短生命周期选择器。
3. `MockManager::try_become_owner` 先用 `IsOwner` 快速判断；若尚未持有，则递增任期 epoch，再调用 `SetOwner`。只有抢占成功的实例才触发 `OnBecomeOwner`。
4. 多个实例争用相同复合键时，`SetOwner` 在互斥锁内执行“读取空值并写入”，因此至多一个调用返回 `true`。不同复合键虽逻辑隔离，但仍串行经过同一全局互斥锁。
5. `MockManager::IsOwner` 将自身 ID 交给选择器的 `IsOwner`；`GetOwnerID` 在此判断成功后返回自身 ID，否则返回 `OwnerError::NoLeader`。
6. 取消、关闭或主动辞任最终进入 `retire_owner_if_current`，由 `UnsetOwner` 做所有权比较。只有仍持有该键的实例能清空状态并触发 `OnRetireOwner`；旧持有者不能误删后来者。
7. `GetOwner` 主要为状态观察和日志提供当前 ID，不参与所有权转移。

## 数据与状态

核心状态是 `HashMap<OwnerKey, String>`。键的两个字符串共同定义竞选域，值为空串表示“无 Owner”，非空字符串表示 Owner 节点 ID。`SetOwner` 与 `UnsetOwner` 使用 `entry(...).or_default()`，所以即使操作失败也可能为此前不存在的键留下一个空字符串条目；公开行为仍与缺失键相同，但映射不会主动删除空条目。

`GetOwner` 和 `IsOwner` 对缺失键采用 `unwrap_or_default()`，因此 `IsOwner("")` 在缺失键上会返回 `true`。这与 Go map 查询得到字符串零值的行为一致，但也意味着调用者必须把空串保留为“无人持有”的哨兵值，不能把空字符串当作合法节点 ID。源码没有显式拒绝 `SetOwner("")`：该调用会返回 `true`，但状态仍可被后续候选抢占。

公开方法返回拥有所有权的 `String` 或布尔值，不暴露锁守卫、内部键或映射引用。

## 依赖与调用关系

直接依赖全部来自标准库：`HashMap` 存储映射，`LazyLock` 初始化全局实例，`Mutex` 串行化每一次访问；`pkg/owner/Cargo.toml` 不因本文件引入额外第三方依赖。

RustCodeGraph 的文件节点显示目标文件被 `pkg/owner/mock.rs`、`pkg/owner/manager_1_aster_unit_test.rs`、`pkg/owner/manager_test.rs` 和 `pkg/dxf/framework/handle/status_testkit_test.rs` 使用。直接生产调用链集中在 `mock.rs`：

- `MockManager::selector` -> `MockGlobalState::OwnerKey`；
- `MockManager::try_become_owner` -> `MockGlobalStateSelector::SetOwner`；
- `MockManager::retire_owner_if_current` -> `MockGlobalStateSelector::UnsetOwner`；
- `Manager for MockManager::IsOwner` -> `MockGlobalStateSelector::IsOwner`。

测试可直接创建 `MockGlobalState::default()` 验证状态原语，也可通过 `NewMockManager` 间接覆盖完整竞选链。基于 etcd 的 `pkg/owner/manager.rs` 不依赖此表。

## 错误处理与边界

这些 API 不返回 `Result`。正常竞争失败通过 `false` 表达，缺失条目通过空串表达；因此调用者必须区分“条件不满足”与系统错误的概念，本模块没有可恢复的 I/O 错误。

所有锁获取都调用 `.expect("mock owner mutex poisoned")`。如果持锁线程发生 panic 并导致互斥锁中毒，后续访问会以该固定消息再次 panic，而不会恢复状态或返回错误。这适合测试辅助状态的 fail-fast 策略，但不应当被当作生产容错保证。

边界还包括：状态仅在当前进程有效、无持久化和清理机制；选择器持有状态借用，不能脱离对应 `MockGlobalState`；Owner ID 为空串会破坏“空值即无人持有”的调用约定；全局实例会跨同一测试进程的测试保留状态，测试应使用不同键、显式释放，或创建独立的 `MockGlobalState`。

## 并发与资源生命周期

`LazyLock` 保证 `MockGlobalStateEntry` 只初始化一次，并具有进程生命周期。`Mutex<HashMap<...>>` 让每个操作的查找、比较与写入构成一个临界区；尤其 `SetOwner` 和 `UnsetOwner` 的 compare-and-set 语义不会被同一进程中的其他线程插入。

选择器本身没有锁，也不拥有后台资源；每次方法调用才获取并在返回前释放全局锁。锁内只有哈希表访问、字符串比较/克隆/转换，不执行 `await`、回调或日志，从而避免持锁跨异步暂停。代价是所有竞选域共用一把锁，大量互不相关键仍会互相争用；当前用途是本地 Mock，源码没有分片或性能保证。

Owner 的异步任务、`CancellationToken`、`Notify`、监听器及 epoch 生命周期都由 `pkg/owner/mock.rs` 管理。该文件只确保状态切换原子，不保证候选公平、事件顺序或通知恰好一次；这些性质要结合上层代码和测试判断。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/owner/mock_owner_state.go`。Rust 保留了 Go 的核心模型和方法名：全局 `MockGlobalStateEntry`、按 `storeID + ownerTp` 组成的键、互斥锁保护的 map，以及 `GetOwner`、`SetOwner`、`UnsetOwner`、`IsOwner` 四种操作。

语义对应如下：Go 缺失 map 键得到字符串零值，Rust 用 `unwrap_or_default()` 得到空串；Go 在锁内判断空值后赋值，Rust 用 `entry(...).or_default()` 在同一锁守卫内完成；Go 仅在当前值等于传入 Owner 时清空，Rust 完全相同。两者都会保留值为空串的键，而不是删除 map 项。

实现差异主要来自语言：Rust 把 `(store_id, owner_type)` 封装为派生 `Hash`/`Eq` 的 `OwnerKey`，选择器通过生命周期借用父状态；Rust 读取时克隆字符串以避免泄露锁内引用；Rust `Mutex` 可能中毒并触发 panic，Go `sync.Mutex` 没有对应状态。当前 Rust 文件不是简化桩，其状态转换与 Go 文件逐项对齐；跨进程 etcd 语义仍由另一实现承担。

## 扩展指南

- 新增状态操作时，应放在 `MockGlobalStateSelector` 上，并把“检查 + 修改”保留在一次锁获取中；不要先调用 `GetOwner` 再另行写入，否则会产生竞态窗口。
- 若要改变竞选域，需同时修改私有 `OwnerKey`、`MockGlobalState::OwnerKey` 和 `MockManager::selector`，并确认 Go `ownerKey` 的兼容语义。键的等价关系变化会直接影响不同后台服务是否共享 Owner。
- 若要允许空字符串作为合法 ID，必须先引入显式的无 Owner 表示（例如 `Option<String>`），同步调整四个方法、Go 对照和所有调用者；只修改单一方法会破坏 CAS 不变量。
- 若要回收空条目，可在成功释放时删除键，但应评估借用键删除及 Go 行为差异，并增加并发回归测试；当前实现有意采用清空值的 Go 语义。
- 状态原语的直接回归应继续放在独立文件 `pkg/owner/manager_1_aster_unit_test.rs`，不要把测试嵌入本源文件；至少覆盖键隔离、重复抢占失败、错误持有者释放失败、正确释放成功和空值边界。完整生命周期还应同步 `pkg/owner/mock_test.rs` 或 `pkg/owner/manager_test.rs` 中的 `MockManager` 测试。
- 性能优化若改成分片锁或并发映射，必须保持单键操作原子，并证明不会在回调或异步等待期间持锁。

## 验证依据

- RustCodeGraph `status --json`：索引已初始化，覆盖 Rust/Go 文件；目标文件节点完整显示 127 行源码，并列出 `pkg/owner/mock.rs` 及相关测试使用者。
- RustCodeGraph `node --file pkg/owner/mock_owner_state.rs --offset 1 --limit 400`：核对全局变量、两个结构体、五个公开方法及其锁内逻辑。
- RustCodeGraph `query MockGlobalState`、`query MockGlobalStateSelector`：核对 Rust/Go 同名类型和 Go 对照方法；精确 `callers` 查询在本次环境中未在 60 秒内返回，因此调用边另由已索引的 `mock.rs` 文件节点与精确引用搜索交叉确认。
- 已读 Rust 路径：`pkg/owner/mock_owner_state.rs`、`pkg/owner/mock.rs`、`pkg/owner/lib.rs`、`pkg/owner/manager_1_aster_unit_test.rs`、`pkg/owner/mock_test.rs`、`pkg/owner/manager_test.rs`。
- 已读边界声明：`pkg/owner/Cargo.toml`；该目录不存在 `doc.go`，包定位由 `lib.rs` 的 crate 文档和模块声明核对。
- 已读 Go 对照：`pkg/owner/mock_owner_state.go`、`pkg/owner/mock.go`、`pkg/owner/manager_test.go`。Go 没有该状态表的同名独立测试，`manager_test.go` 通过 `NewMockManager` 的竞选、关闭和 Owner 查询间接覆盖它。
- 直接 Rust 回归 `mock_global_state_is_scoped_and_compare_and_sets` 验证缺失键空串、`IsOwner("")`、不同 store/type 隔离、首次抢占成功、重复抢占失败、错误 ID 不能释放及正确 ID 释放；`mock_managers_compete_resign_and_notify_like_go` 验证上层竞争与交接。
- 本任务为纯文档分析，按计划不运行 Cargo；交付验证仅包含固定章节结构检查和人工事实复核。
