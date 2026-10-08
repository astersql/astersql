# `pkg/store/mockstore/unistore/tikv/detector.rs`

## 文件定位

本文件是 UniStore 内嵌 mock TiKV 的等待图死锁检测核心，源码为 [`detector.rs`](detector.rs)。它只负责维护“事务正在等待哪个事务”的有向图、判断新增等待关系是否闭环，以及清理等待边；请求分发、Leader/Follower 角色和死锁后的 waiter 唤醒位于相邻的 [`deadlock.rs`](deadlock.rs)。模块由 [`lib.rs`](lib.rs) 以 `pub mod detector` 暴露，所属 crate 是 [`Cargo.toml`](Cargo.toml) 中的 `astersql-store-mockstore-unistore-tikv`。

该实现服务于进程内 mock 存储，并不直接实现网络协议或真实 TiKV 的分布式死锁检测。当前直接生产接线是 `DetectorServer::detect`：它把 `Detect`、`CleanUpWaitFor`、`CleanUp` 三类请求分别转发到本文件的对应方法。

## 核心职责

- 用 `DetectorState::wait_for: HashMap<u64, Vec<Edge>>` 保存邻接表，其中键是等待方事务 `source_txn`，每条 `Edge` 指向被等待事务。
- 在 `Detector::detect` 接受一条候选等待边之前，从 `wait_for_txn` 开始执行 DFS；若能沿既有边回到 `source_txn`，返回 `DeadlockError`，且不登记本次闭环边。
- 保存每条边的锁键哈希、原始锁键及资源组标签，使死锁结果中的 `wait_chain` 可用于诊断。
- 通过 `entry_ttl` 淘汰陈旧等待边：DFS 访问节点时惰性清理；图达到 `urgent_size` 且超过 `expire_interval` 时主动扫描全图。
- 提供按事务清理全部出边、按 `(wait_for_txn, key_hash)` 精确清理单边，以及读取当前边数的接口。

## 主要符号

- `DiagnosticContext { key, resource_group_tag }`：公开的诊断输入快照。两个字段均为拥有所有权的 `Vec<u8>`，登记边后不依赖调用者缓冲区。
- `WaitForEntry { txn, wait_for_txn, key_hash, key, resource_group_tag }`：公开的等待链元素；语义是 `txn` 正在等待 `wait_for_txn`。
- `DeadlockError { deadlock_key_hash, wait_chain }`：公开的检测结果。`deadlock_key_hash` 来自既有图中最终指回源事务的边；`wait_chain` 还会包含本次触发闭环但未登记的候选边。
- `Edge`：私有邻接边，除目标事务和诊断信息外，记录 `registered: Instant` 供 TTL 判断。
- `DetectorState`：私有可变状态，包含邻接表、边总数 `total_size` 和最近主动清理时刻 `last_active_expire`。
- `Detector`：公开检测器。`state` 由 `Mutex` 串行保护；`entry_ttl`、`urgent_size`、`expire_interval` 在构造后保持不变。
- `Detector::new(...)`：建立空图并把 `last_active_expire` 初始化为当前单调时钟。
- `Detector::detect(...) -> Option<DeadlockError>`：检测并在无环时登记候选边，是核心公开入口。
- `Detector::do_detect(...)`：私有递归 DFS，同时惰性删除访问到的过期边。
- `Detector::{clean_up, clean_up_wait_for}`：分别删除某事务全部出边和首条完全匹配的边。
- `Detector::active_expire(...)`：满足容量与时间双阈值时全图清理过期边。
- `Detector::edge_count()`：在锁内返回 `total_size`，当前主要供独立测试和观测使用。

## 执行流程

1. `DetectorServer::detect` 收到 `RequestType::Detect` 后，从请求的 `WaitForEntry` 构造 `DiagnosticContext` 并调用 `Detector::detect(source_txn, wait_for_txn, key_hash, diagnostic)`。
2. `detect` 获取整个 `DetectorState` 的互斥锁并捕获一次 `Instant::now()`；同一次检测内的主动清理、DFS 和新边登记共享该时间点。
3. `active_expire` 先判断是否同时满足“距上次主动清理严格大于 `expire_interval`”和“`total_size` 不小于 `urgent_size`”。满足时遍历所有邻接列表，删除 TTL 已超出的边和空列表，并校正总数。
4. `detect` 创建空 `visited` 集合，以候选边的目标 `wait_for_txn` 为 `current` 调用 `do_detect`，查找是否能到达候选边的源 `source_txn`。
5. `do_detect` 首先用 `visited.insert(current)` 剪枝；然后暂时从 map 中取出 `current` 的边列表，删除 `now.duration_since(registered) > entry_ttl` 的边并扣减 `total_size`，非空列表再放回 map。
6. DFS 遍历清理后的快照。某条既有边直接指向 `source` 时，以该边的 `key_hash` 创建 `DeadlockError`；否则递归访问下一事务，并在递归成功返回时把当前边追加到反向构造的等待链。
7. `detect` 收到错误后反转既有链，使顺序变成“每项等待下一项”，再追加本次候选边并返回 `Some(error)`。候选边不会写入邻接表，因此图仍只保存成环前的等待关系。
8. 若 DFS 未找到环，`detect` 检查源事务的现有出边；只有不存在相同 `(wait_for_txn, key_hash)` 时才登记新 `Edge` 并递增 `total_size`，最后返回 `None`。
9. 清理请求由 `DetectorServer::detect` 转发：`clean_up(txn)` 删除该事务的整个出边列表；`clean_up_wait_for` 只删除首条匹配的 `(wait_for_txn, key_hash)` 边，并在列表变空后删除 map 键。

## 数据与状态

等待图允许同一事务拥有多条出边，也允许等待同一目标事务的不同 `key_hash` 分别存在；完全相同的 `(source_txn, wait_for_txn, key_hash)` 被视为重复登记。诊断字段不参与去重，因此对同一三元组再次提交不同的 `key` 或资源组标签不会覆盖首条边的诊断快照。

`total_size` 是所有 `Vec<Edge>` 长度之和。`detect` 登记、DFS 惰性过期、主动过期、两种清理方法都在持锁状态下同步调整它；空邻接列表会从 `wait_for` 删除。测试 [`detector_test.rs`](detector_test.rs) 用 `edge_count` 验证闭环边不落图、重复边不增长、精确清理只减一条以及 TTL 清理后的计数。

过期判定保留年龄恰好等于 `entry_ttl` 的边，只删除年龄严格大于 TTL 的边。`last_active_expire` 只有真正执行全图扫描时才更新；普通 DFS 的惰性清理不会改变它。

## 依赖与调用关系

上游生产调用链为 `DetectorClient::{detect, clean_up_wait_for, clean_up}` → `DetectorClient::submit` → `DetectorServer::detect` → 本文件的 `Detector::{detect, clean_up_wait_for, clean_up}`。检测到环后，`deadlock.rs` 的 `convert_error` 把 `DeadlockError` 转成 `DeadlockResponse`，再由 `DeadlockWaiterManager::wake_up_for_deadlock` 消费。RustCodeGraph 的文件节点确认 `detector.rs` 被 `deadlock.rs`、`deadlock_test.rs` 和 `detector_test.rs` 等文件引用；精确源码核验确认上述服务端转发边。

内部调用关系是 `Detector::detect` → `Detector::active_expire` 和 `Detector::do_detect`，而 `do_detect` 递归调用自身。实现只依赖标准库的 `HashMap`、`HashSet`、`Mutex`、`Duration` 和 `Instant`；本文件不使用 crate 的外部依赖。crate manifest 通过 `[lib] path = "lib.rs"` 定义入口，并以 `package.metadata.porting.go-package = "pkg/store/mockstore/unistore/tikv"` 标明 Go 对照包；其大部分内部 crate 依赖只在 Windows target 下声明，但本检测器本身没有平台条件编译项。

## 错误处理与边界

死锁是正常业务结果，以 `Option<DeadlockError>` 的 `Some` 表示，而不是 Rust 的运行时错误；无环（包括路径不存在或路径上的边已过期）返回 `None` 并尝试登记候选边。返回错误时，`deadlock_key_hash` 是既有闭环关键边的哈希，而等待链末项是本次候选边，两者不要混为同一个字段来源。

所有锁获取都使用 `expect(...)`；若持锁线程 panic 导致 mutex poisoned，后续调用会 panic，不提供恢复路径。DFS 以 `visited` 防止既有图中与当前源事务无关的环造成无限递归，但搜索仍是递归实现，异常长等待链存在栈深风险。TTL 为零时，同一 `now` 下刚登记的边年龄可能为零并暂时有效；下一次检测只要年龄严格大于零即可清理。容量阈值为零时仍需超过 `expire_interval` 才执行主动清理。

`clean_up_wait_for` 按 `(wait_for_txn, key_hash)` 删除首个匹配项。由于登记逻辑保证同一源事务下该组合唯一，这等价于删除目标边；若未来改变去重键，必须同时重新审视此处只删首项的假设。

## 并发与资源生命周期

`Detector` 的全部可变图状态由单个 `std::sync::Mutex<DetectorState>` 保护。一次 `detect` 从主动过期、完整 DFS 到登记或返回错误始终持锁，因此并发请求观察不到中间状态，也不会在“检查无环”和“登记新边”之间插入另一条边。代价是 DFS 和全图过期扫描期间其他检测与清理请求全部阻塞。

`Edge` 拥有诊断字节及登记时刻，随邻接表删除而释放；没有后台线程、定时器、通道或异步任务。TTL 回收是请求驱动的：主动扫描只从 `detect` 入口触发，惰性扫描只清理由本次 DFS 实际访问的节点；长期没有检测请求时，过期边仍保留在内存中。`DetectorClient` 自身的 pending 队列和 waiter 生命周期属于 `deadlock.rs`，不由本文件管理。

## 与 Go 版本的对应关系

直接对照文件是 [`detector.go`](detector.go)，行为测试是 [`detector_test.go`](detector_test.go)。Rust 的 `Detector`、`DetectorState::wait_for`、`Edge`、`DiagnosticContext` 分别对应 Go 的 `Detector` 状态字段、`waitForMap`、`txnKeyHashPair`、`diagnosticContext`；`detect`/`do_detect`/两种清理/`active_expire` 与 Go 同名方法保持相同主流程。

两版共同保证：成环触发边不登记；等待链按“当前项等待下一项”排序；相同目标但不同 key hash 可并存；完全相同的目标与 key hash 去重；过期边不参与成环；主动过期要求时间与容量条件同时满足。Rust 测试 `deadlock_cycle_returns_ordered_diagnostic_wait_chain`、`broken_cycle_and_duplicate_registration_match_go`、`expired_edges_are_removed_without_reporting_a_deadlock` 分别固定这些语义，并与 Go 的 `TestDeadlock` 相互印证。

实现差异包括：Go 用 `container/list`，Rust 用 `Vec<Edge>`；Go 把错误表示为 `kverrors.ErrDeadlock`/protobuf 条目，Rust 在本文件定义本地 `DeadlockError`/`WaitForEntry`，由 `deadlock.rs` 再转换为服务响应；Go 主动清理会写日志，Rust 当前不记录日志；Rust DFS 增加了 `HashSet` 访问剪枝，可安全终止不包含当前源事务的既有环；Rust 将原 Go 的独立 `register` 逻辑内联到 `detect`。这些是表示和健壮性差异，不应在扩展时误当作可删除的简化。

## 扩展指南

- 若改变环检测或等待链顺序，优先修改 `Detector::detect` 与 `Detector::do_detect`，并在独立的 [`detector_test.rs`](detector_test.rs) 增补多分支图、非源环和链顺序用例；不要把测试内嵌进生产源文件。
- 若改变去重维度或允许同一事务/目标/键的多条边，必须同步修改登记检查、`clean_up_wait_for` 的删除语义、`total_size` 不变量，以及 `deadlock.rs` 的请求字段映射。
- 若新增诊断字段，需要贯穿 `DiagnosticContext`、`Edge`、`WaitForEntry`、DFS 构造链、`deadlock.rs` 的请求/响应转换，并同步 Go 对照语义与两边独立测试。
- 若优化性能，重点评估单全局锁、DFS 中克隆边快照、`Vec::remove` 的线性移动和全图主动扫描。优化不得破坏“检测与登记原子化”及计数一致性；引入分片锁时尤其要防止并发交错漏检闭环。
- 若调整 TTL 策略，要明确边界是 `>` 还是 `>=`、是否需要无请求后台回收，以及 `last_active_expire` 更新条件，并增加确定性时钟或边界测试，避免仅依赖易抖动的 sleep。
- 公开类型或服务接线变化还需检查 [`deadlock.rs`](deadlock.rs) 和 [`deadlock_test.rs`](deadlock_test.rs)。crate 归属或 feature 变化则同步核验 [`Cargo.toml`](Cargo.toml) 与 [`lib.rs`](lib.rs)。

兼容性风险主要是等待链字段/顺序影响上层死锁响应，正确性风险主要是漏检、误报、计数下溢或清理错边，性能风险主要是持锁 DFS 与全图扫描造成请求串行等待及深链递归。

## 验证依据

- RustCodeGraph：`status` 显示当前索引覆盖 11,467 个文件；`files --filter pkg/store/mockstore/unistore/tikv/detector.rs` 定位到目标文件；文件节点显示 257 行、14 个符号及 `deadlock.rs`、`deadlock_test.rs`、`detector_test.rs` 等引用者；`query` 确认 `Detector`、`do_detect`、`active_expire`、`clean_up_wait_for`、`edge_count` 的定义位置。图的宽泛同名查询会混入 Lightning 和 planner 的其他 detector，因此最终调用边以目标文件节点和相邻源码精确核验为准。
- Rust 源码：[`detector.rs`](detector.rs) 核验所有类型、方法、锁范围、DFS、TTL、清理与计数逻辑；[`deadlock.rs`](deadlock.rs) 核验真实上游分发、错误转换和 waiter 回调；[`lib.rs`](lib.rs) 核验模块公开与独立测试装配。
- crate 配置：[`Cargo.toml`](Cargo.toml) 核验 crate 名称、库入口、Go 包迁移元数据、target 依赖和本文件无额外依赖的事实。
- 独立测试：[`detector_test.rs`](detector_test.rs) 核验三事务闭环、链顺序、闭环边不登记、精确清理、重复边去重、不同 key hash 共存和 TTL 惰性清理。
- Go 对照：[`detector.go`](detector.go) 与 [`detector_test.go`](detector_test.go) 核验移植结构、阈值条件、等待链语义和差异。本文是纯文档分析，按任务约束未运行 Cargo 或代码测试；交付验证使用任务规定的 11 章节结构检查并人工复核链接与证据。
