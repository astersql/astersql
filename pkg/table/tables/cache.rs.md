# `pkg/table/tables/cache.rs`

## 文件定位

本文件是 `astersql-table-tables` crate 的缓存表核心状态模块，由 [`lib.rs`](lib.rs) 以公开模块 `pub mod cache` 暴露。它承载单表内存快照、读写租约换算、远程状态句柄串行访问和容量门槛判断；远程锁状态机的具体实现位于 [`state_remote.rs`](state_remote.rs)，其中 `StateRemoteHandle<S>` 实现本文件定义的 `StateRemote` trait。

当前 Rust 接线边界需要特别说明：仓库搜索显示，这里的 `CachedTable<R>` 及其方法目前只被独立测试 [`cache_test.rs`](cache_test.rs) 直接构造和调用，尚未实现上层 `table-dependency` crate 在 [`../table.rs`](../table.rs) 中定义的 `CachedTable: Table` 接口，也没有接入 Go 版本的 SQL 执行、事务变更和异步整表装载链。因此它是已经可测试的底层核心，而不是完整 Rust 缓存表功能的生产入口。

## 核心职责

- `lease_from_ts` 将 TSO 风格时间戳的逻辑低 18 位清零，再把租约毫秒数左移 18 位后加入物理部分，对齐 Go `oracle.GetTimeFromTS` 加时长再 `oracle.GoTimeToTS` 的效果。
- `CacheData` 把快照有效区间 `[start, lease)` 与可选的 `MemBuffer` 绑定；`mem_buffer == None` 表示已经建立租约但后台装载尚未完成。
- `TokenLimit<R>` 用容量为一的令牌语义保护非线程安全远程句柄：`take` 阻塞、`try_take` 非阻塞、`put` 归还并唤醒一个等待者。
- `CachedTable<R>` 原子维护近似缓存字节数，以读写锁发布不可变 `Arc<CacheData>` 快照，并通过 `StateRemote` 请求读锁、续读租约和写锁。
- 本文件不负责扫描原表、启动后台任务、写租约周期续期、SQL 错误映射或表接口适配；这些是 Go [`cache.go`](cache.go) 中存在、但尚未完整移入此 Rust 文件的职责。

## 主要符号

- `CACHED_TABLE_SIZE_LIMIT: i64`：64 MiB 门槛。`can_apply_mutation` 检查的是当前已经装载的大小，而不是把本次增量预先相加。
- `CACHE_TABLE_WRITE_LEASE: Duration`：固定 5 秒写租约，传给 `StateRemote::lock_for_write`。
- `LOGICAL_BITS: u32`：私有常量 18，用于 TSO 物理毫秒与逻辑位之间的编码。
- `MemBuffer = BTreeMap<Vec<u8>, Vec<u8>>`：本地有序原始字节 KV。它是 Rust 侧简化的内存表示，并非 Go `kv.MemBuffer` 的完整接口替代。
- `CacheData { start, lease, mem_buffer }`：不可变快照载荷。`start` 为包含边界，`lease` 为排除边界。
- `StateRemote`：远程协调边界，要求实现者 `Send`；定义 `lock_for_read`、`renew_read_lease`、`lock_for_write`、`renew_write_lease`。本文件的 `CachedTable` 当前只调用前三者，`renew_write_lease` 预留给尚未移植的写锁保活流程。
- `TokenLimit<R>`：以 `Mutex<Option<R>> + Condvar` 模拟 Go `chan StateRemote` 容量一通道。
- `CachedTable<R>`：持有私有 `table_id`、`RwLock<Option<Arc<CacheData>>>`、`AtomicI64 total_size` 和 `TokenLimit<R> remote`。
- `CachedTable::new`、`install_cache`、`total_size`、`try_read_from_cache`、`can_apply_mutation`：不要求 `R: StateRemote` 的本地状态 API。
- `CachedTable::update_lock_for_read`、`renew_lease`、`lock_for_write`：要求 `R: StateRemote` 的远程协调 API。

文件没有条件编译项；feature 开关位于 [`Cargo.toml`](Cargo.toml)，且 `cache` 模块本身不依赖默认的 `expression-runtime` feature。

## 执行流程

1. 调用方以表 ID 和远程句柄构造 `CachedTable::new`。初始没有快照，大小为 0，远程句柄作为唯一可用令牌存入 `TokenLimit`。
2. 读路径调用 `try_read_from_cache(timestamp, lease_duration)`：先在读锁下克隆当前 `Arc<CacheData>`，随后立即释放锁；无快照或时间戳不在 `[start, lease)` 时返回 `(None, false, false)`。
3. 有效窗口内，函数计算半租约的 TSO 单位值。当 `lease - timestamp` 小于等于半租约时置 `should_renew = true`。它返回缓冲的 `Arc`、`mem_buffer` 是否为空所表示的 `loading`，以及续租提示；本函数本身不启动续租任务。
4. 外层装载逻辑完成后可调用 `install_cache`。该方法遍历缓冲区，以所有键和值的字节长度之和更新 `total_size`，然后在写锁下替换整个 `Arc<CacheData>`。
5. 需要远程读锁时，`update_lock_for_read` 阻塞取得远程令牌，将 `lease_from_ts(timestamp, duration)` 和 `table_id` 传给远程实现；无论远程返回成功还是错误，正常返回路径都会先归还令牌。
6. `renew_lease` 在取得远程令牌前克隆当前快照，并用快照中的旧租约（无快照时为 0）调用远程续租。仅当远程返回 `Ok(new_lease)` 且新值大于 0 时，才用相同 `start` 和 `mem_buffer` 发布新快照。
7. 写路径调用 `lock_for_write`，串行取得句柄并以 5 秒固定时长委托给远程实现。实际 `StateRemoteHandle<S>::lock_for_write` 会复用足够安全的既有写租约，否则循环等待旧读租约过期。

## 数据与状态

`cache_data` 使用 `RwLock<Option<Arc<CacheData>>>`：读者获得的是不可变快照的共享所有权，因此锁释放后快照仍有效；更新采用整份替换，不在原对象上原地修改。该设计使 `renew_lease` 能保留缓冲区并只更新租约字段，但也意味着并发安装的新快照可能被基于旧快照完成的续租结果覆盖；Go 版本通过远程旧租约条件和 ABA 测试约束这一风险，Rust 当前未额外比较本地快照身份。

`total_size` 是键和值有效载荷长度的近似值，不包含 `BTreeMap` 节点、`Vec` 容量、`Arc` 或分配器开销。`install_cache` 先以 `Release` 写入大小，再替换快照；`total_size` 用 `Acquire` 读取。两者不是同一个原子事务，因此并发观察者可能短暂看到新大小/旧快照或反之，代码没有承诺二者的线性一致视图。

`can_apply_mutation(_size_delta)` 故意忽略参数，只在当前 `total_size <= 64 MiB` 时放行。这对应 Go `AddRecord`/`UpdateRecord` 在变更前检查既有缓存大小的行为：使表跨过限制的当前写入仍可执行，后续重新装载并更新大小后才阻止下一次写入。删除在 Go 中不受该门槛限制；Rust 本类型尚未接入具体变更 API。

## 依赖与调用关系

- crate 边界：[`Cargo.toml`](Cargo.toml) 声明包名 `astersql-table-tables`、库入口 `lib.rs`、`autotests = false`；测试由 `lib.rs` 的 `#[cfg(test)] mod cache_test` 独立装配。
- 标准库依赖：`BTreeMap` 提供有序 KV，`Arc` 共享快照/缓冲，`Mutex + Condvar` 实现独占令牌，`RwLock` 发布快照，`AtomicI64` 保存近似大小，`Duration` 表示租约。
- 下游远程实现：[`state_remote.rs`](state_remote.rs) 导入 `StateRemote` 和 `lease_from_ts`，并为 `StateRemoteHandle<S: RemoteStore>` 实现 trait。其写锁流程读取远程时钟、等待旧读租约并更新锁行。
- 上层契约：[`../table.rs`](../table.rs) 的 `CachedTable: Table` 定义 `Init`、`TryReadFromCache`、`UpdateLockForRead`、`WriteLockAndKeepAlive`，但当前 `cache.rs::CachedTable<R>` 没有实现该 trait。这是明确的未接线边界。
- 直接 Rust 调用者：源码搜索仅发现 [`cache_test.rs`](cache_test.rs) 调用本文件的方法；没有生产 Rust 文件构造 `CachedTable<R>`。因此不能把 Go 的 `domain`、`executor` 或事务驱动调用边宣称为 Rust 已接通。
- Go 生产调用边：[`cache.go`](cache.go) 的 `cachedTable` 被 Go `domain`、executor builder、txn driver、mutation checker 和普通表实现使用；这些只能作为迁移语义对照，不能作为 Rust 静态调用边。

## 错误处理与边界

- `StateRemote` 的关联错误类型不被包装，三个远程 API 原样返回 `R::Error`。测试证明读锁错误后句柄仍会归还，第二次请求可以继续到达远程。
- `Mutex`、`Condvar` 和 `RwLock` 中毒会通过 `expect(...)` panic；这不是可恢复错误通道。`TokenLimit::put` 在令牌尚未取出时调用会因重复归还断言而 panic。
- 远程调用若 panic，当前实现不会执行 `put`，句柄会随栈展开离开局部变量，令牌槽保持空；后续 `take` 将永久等待。现有代码和测试只覆盖 `Result::Err`，没有 panic 恢复保证。
- `lease_from_ts` 对超大毫秒数或乘加溢出采用饱和到 `u64::MAX`，不会 panic；它丢弃输入时间戳的逻辑位。
- 缓存有效性严格为 `timestamp >= start && timestamp < lease`。到期点不可读；无快照和窗口外均不报告“加载中”。
- `renew_lease` 在无缓存时仍以旧租约 0 请求远端；返回 0 或错误都不更新本地快照。它没有确认返回的新租约大于旧租约，具体合法性由远程实现负责。
- 64 MiB 判断是 `<=`，所以恰好达到上限仍允许变更；只有重新装载后的当前大小严格大于上限才拒绝后续变更。

## 并发与资源生命周期

`TokenLimit` 确保同一 `CachedTable` 同时最多一个调用者持有 `R`。`take` 在条件变量循环中重新检查槽位，可抵御虚假唤醒；`try_take` 对应 Go `select/default`；`put` 唤醒一个等待者。类型本身没有 RAII guard，调用者必须显式归还，因此新增远程方法时每条正常和错误返回路径都要检查令牌回收，若需 panic 安全应先引入守卫类型。

快照生命周期由 `Arc` 管理：替换 `cache_data` 不会使已有读者持有的旧快照或旧 `MemBuffer` 失效。`install_cache` 和成功续租取得写锁发布新快照，读路径只短暂持有读锁。`AtomicI64` 避免为只读容量检查获取快照锁。

与 Go 版本相比，Rust 本文件不创建线程、计时器或通道消费者，也没有写锁 keep-alive 生命周期。Go `TryReadFromCache` 会非阻塞取得远程句柄并启动续租 goroutine，`UpdateLockForRead` 会异步装载原表，`WriteLockAndKeepAlive` 会每半个写租约续期；Rust 目前仅返回 `should_renew` 让外层决定调度，并暴露一次性远程操作。

## 与 Go 版本的对应关系

| Rust 符号 | Go 对照 | 对齐情况 |
| --- | --- | --- |
| `CacheData` | `cacheData` | 对齐 `Start`、`Lease` 和“空缓冲表示装载中”；Rust 用 `Option<Arc<BTreeMap<...>>>`。 |
| `lease_from_ts` | `leaseFromTS` | 对齐丢弃逻辑位并在物理时间上加租约；Rust 对数值溢出额外采用饱和语义。 |
| `TokenLimit<R>` | `tokenLimit chan StateRemote` | 对齐容量一、阻塞/非阻塞取得和归还；Rust 重复归还会断言。 |
| `try_read_from_cache` | `TryReadFromCache` | 对齐有效窗口、loading 和半租约阈值；Rust 只返回 `should_renew`，不自行启动 goroutine，也没有 Go failpoint 强制触发分支。 |
| `install_cache` | `updateLockForRead` 的异步装载完成分支 | 只抽取快照安装与大小统计；Rust 没有扫描存储、指标记录和 `Start == ts` 的陈旧装载防护。 |
| `can_apply_mutation` | `AddRecord` / `UpdateRecord` 前的大小检查 | 对齐“检查既有大小、当前变更可跨界”；Rust 尚未返回 `ErrOptOnCacheTable` 或接入表变更。 |
| `update_lock_for_read` | `updateLockForRead` 中的 `LockForRead` | 仅对齐远程请求参数；Rust 不负责异步调度和装载原表。 |
| `renew_lease` | `renewLease` | 对齐旧租约、新租约和正值更新规则；Go 另有日志、retryable-error 区分和 ABA failpoint 测试。 |
| `lock_for_write` | `lockForWrite` | 对齐固定 5 秒写租约；Go 另有 `WriteLockAndKeepAlive` / `renew`，Rust 本文件尚无周期保活。 |

Go 集成测试 [`cache_test.go`](cache_test.go) 还验证 SQL 扫描/索引/连接命中、写入使缓存失效、事务 ABA、全局租约变量、等待时长和指标；这些是完整 Go 子系统的证据，不代表 Rust 单元已经覆盖相同集成行为。

## 扩展指南

- 接入生产链前，应在独立源文件中实现或适配 [`../table.rs`](../table.rs) 的 `CachedTable: Table`，不要把测试或 SQL 集成逻辑塞回 `cache.rs`；并在同目录独立 `*_test.rs` 文件补齐契约测试。
- 移植异步装载时，应保留 Go `updateLockForRead` 的关键不变量：先发布 `mem_buffer = None` 的加载中快照，装载完成时确认请求身份仍匹配，再安装数据并更新大小，避免旧任务覆盖新租约周期。
- 将 `should_renew` 接到调度器时必须使用 `try_take` 或等价非阻塞机制，确保同一远程句柄不会并发进入；还要保留 Go 的旧租约条件以防 ABA。
- 增加写锁保活时，应补齐半租约 ticker、退出信号、`renew_write_lease` 调用、租约原子发布和错误退出，并与 Go `WriteLockAndKeepAlive` 的首次结果通知顺序一致。
- 修改大小策略时需明确近似口径及“当前变更跨界”的兼容行为；相关 Rust 测试应更新 [`cache_test.rs`](cache_test.rs)，SQL 层接通后还应增加独立集成测试，而不是只验证方法返回值。
- 修改租约换算、窗口边界或远程协议时，要同步检查 [`state_remote.rs`](state_remote.rs) 及其独立测试 [`state_remote_test.rs`](state_remote_test.rs)，特别关注逻辑位、到期等号、旧租约参数和远程返回 0 的含义。
- 若要增强异常安全，优先把远程令牌封装为 Drop 时自动归还的守卫；否则每个新增提前返回、错误映射或 panic 边界都可能遗失令牌。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 7,032 个 Rust 文件、目标文件识别出 30 个符号；`node --file pkg/table/tables/cache.rs` 核对了完整 257 行源码，`query` 精确确认了 `CachedTable`、`TokenLimit`、`try_read_from_cache`、`renew_lease`、`install_cache`、`update_lock_for_read` 和 `can_apply_mutation` 的定义位置。
- 调用图验证：对目标方法运行了 `callers` / `callees` 与精确 `explore`；因仓库大量跨语言同名符号造成图查询噪声，随后按技能允许的回退方式用 Rust 源码搜索消歧，确认生产 Rust 无直接调用，直接调用集中于 `pkg/table/tables/cache_test.rs`。
- 已读 Rust 路径：`pkg/table/tables/cache.rs`、`pkg/table/tables/lib.rs`、`pkg/table/tables/cache_test.rs`、`pkg/table/tables/state_remote.rs`、`pkg/table/tables/state_remote_test.rs` 的关联位置，以及 `pkg/table/table.rs` 的上层 `CachedTable` trait。目标目录没有 `doc.go`。
- 已读边界配置：`pkg/table/tables/Cargo.toml`，确认 crate 名、入口、feature、依赖与 `go-package = "pkg/table/tables"` 移植元数据。
- 已读 Go 对照：`pkg/table/tables/cache.go` 完整实现与 `pkg/table/tables/cache_test.go` 完整测试，核对了异步装载、容量门槛、读/写租约、ABA、SQL 命中和指标等语义。
- Rust 独立测试证据：`cache_test.rs` 覆盖 TSO 换算、容量一令牌、有效窗口和半租约、KV 字节计数、当前变更跨界、远程参数、错误后令牌归还、正租约更新快照；这些测试未在本纯文档任务中运行 Cargo。
- 交付结构以任务规定命令验证，目标是恰好包含本文 11 个固定二级标题；另外人工检查所有本地链接目标存在、没有把 Go 集成行为误写为 Rust 已接线能力。
