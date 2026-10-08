# [`pkg/store/mockstore/unistore/tikv/deadlock.rs`](deadlock.rs)

## 文件定位

本文件位于 `astersql-store-mockstore-unistore-tikv` crate 内，是进程内 mock TiKV 的死锁检测协议适配层。crate 入口 `pkg/store/mockstore/unistore/tikv/lib.rs` 以 `pub mod deadlock` 导出该模块，并把独立测试 `deadlock_test.rs` 通过 `#[path = "deadlock_test.rs"]` 接入。上层 `pkg/store/mockstore/unistore/tikv/server.rs` 的 `Server` 持有一个 `DetectorServer`，其 `detect_deadlock` 方法把请求直接转发到本文件的 `DetectorServer::detect`。

真正维护等待图并执行 DFS、TTL 淘汰和边清理的是相邻的 `pkg/store/mockstore/unistore/tikv/detector.rs`；本文件负责定义请求/响应、分发操作、转换死锁结果，以及提供一个同步的进程内客户端封装。它不负责 MVCC 加锁本身，也不包含网络协议实现。

`pkg/store/mockstore/unistore/tikv/Cargo.toml` 声明该目录为独立 library crate（`[lib] path = "lib.rs"`），并通过 `package.metadata.porting.go-package` 指向 Go 包 `pkg/store/mockstore/unistore/tikv`。当前 manifest 没有为死锁模块声明专用 feature；大量相邻 crate 依赖仅置于 Windows target 段，本文件自身只依赖标准库和同 crate 的 `detector` 模块。

## 核心职责

1. 以 `RequestType`、`DeadlockRequest` 和 `DeadlockResponse` 表达检测、按边清理和按事务清理三种操作及其结果。
2. 由 `DetectorServer::detect` 把协议层请求拆成 `Detector::detect`、`Detector::clean_up_wait_for` 或 `Detector::clean_up` 调用；检测成环时用 `convert_error` 生成响应。
3. 以 `FOLLOWER`、`LEADER` 和原子字段 `DetectorServer::role` 保存模拟的检测器角色，并提供无锁读取/切换接口。
4. 通过 `DetectorClient` 将调用方参数组装成请求，记录已提交请求，并同步调用共享服务端；检测到死锁时，经 `DeadlockWaiterManager` 回调唤醒等待者。

当前 Rust 应用链中，服务端路径已经由 `Server::detect_deadlock` 接入；代码搜索没有发现 `DetectorClient::new`、`DeadlockWaiterManager` 的文件外实现或 `pending_count` 的使用。因此客户端部分是可测试的进程内接口，但不能描述为已经复刻 Go 版本的 PD 寻址和 gRPC 流式传输。

## 主要符号

- `FOLLOWER: i32 = 0`、`LEADER: i32 = 1`：角色值，与 Go 的 `iota` 顺序一致。
- `RequestType::{Detect, CleanUpWaitFor, CleanUp}`：请求分派标签。`Detect` 可能产生响应；两个清理分支始终返回 `None`。
- `DeadlockRequest { request_type, entry }`：把操作类型和 `detector::WaitForEntry` 绑定在一起。`WaitForEntry` 携带等待方事务、被等待事务、键哈希以及诊断用 key/resource group tag。
- `DeadlockResponse { entry, deadlock_key_hash, wait_chain }`：死锁响应。`entry` 表示本次触发检测的边，`deadlock_key_hash` 和 `wait_chain` 来自等待图检测结果。
- `DetectorServer { detector, role }`：协议服务端。`new` 固定使用 3 秒边 TTL、100,000 条紧急阈值和 3,600 秒主动清理间隔；`Default` 等价于 `new`。
- `DetectorServer::detect(&DeadlockRequest) -> Option<DeadlockResponse>`：本文件的核心分派入口。
- `DetectorServer::{is_leader, change_role}`：用 Acquire/Release 顺序访问角色原子值。`detect` 本身不检查角色，调用者若要求仅 Leader 检测，需要在更外层执行该策略。
- `convert_error(DeadlockError, &WaitForEntry) -> DeadlockResponse`：将算法错误转换为协议响应；会保留触发边的三个数值字段，但主动清空该响应 `entry` 中的 key 和 resource group tag。
- `DeadlockWaiterManager: Send + Sync`：死锁唤醒回调边界，唯一方法是 `wake_up_for_deadlock`。
- `DetectorClient { server, waiter_manager, pending }`：可跨线程共享依赖的同步客户端。`submit` 记录请求、调用服务端并在有响应时触发回调；`clean_up`、`clean_up_wait_for`、`detect` 是参数化便捷入口；`pending_count` 仅观测累计提交数。
- 私有 `request(...)`：统一构造 `DeadlockRequest` 及其 `WaitForEntry`，清理请求通过它写入空诊断字节串。

## 执行流程

服务端检测路径如下：

1. `Server::detect_deadlock` 收到 `&DeadlockRequest`，调用其成员 `DetectorServer::detect`（`server.rs:695-698`）。
2. `DetectorServer::detect` 按 `request.request_type` 分支。
3. `Detect` 分支把 `entry.txn`、`entry.wait_for_txn`、`entry.key_hash` 以及克隆后的诊断字节串交给 `Detector::detect`。
4. `Detector::detect` 在 `detector.rs` 的互斥状态中清理适用的过期边并搜索等待图：无环时登记新边并返回 `None`；成环时不登记触发边，而返回包含闭环 key hash 和有序等待链的 `DeadlockError`。
5. 有错误时 `convert_error` 构造 `DeadlockResponse`。等待链保留各边的诊断字段；顶层触发 `entry` 只保留 txn/wait-for/key-hash 三个标识字段。

清理路径直接分发：`CleanUpWaitFor` 删除匹配 `(txn, wait_for_txn, key_hash)` 的一条边，`CleanUp` 删除 `txn` 的全部出边，两者都没有响应载荷。

客户端路径从 `DetectorClient::{detect, clean_up_wait_for, clean_up}` 开始，经私有 `request` 组装值后进入 `submit`。`submit` 先在 `pending` 中保存请求克隆，再同步调用服务端；只有检测得到 `Some(response)` 时才调用 `waiter_manager.wake_up_for_deadlock(response)`。这里没有后台发送循环、重试或网络 I/O。

## 数据与状态

本文件保存三类状态：

- `DetectorServer::detector` 持有真正的等待图；其状态结构、边计数、注册时间和过期策略均定义在 `detector.rs`，不是本文件重复维护。
- `DetectorServer::role: AtomicI32` 初始为 `FOLLOWER`。该字段只记录角色；当前 `DetectorServer::detect` 不根据它拒绝或转发请求。
- `DetectorClient::pending: Mutex<Vec<DeadlockRequest>>` 是只增不减的提交历史。名称虽为 pending，但当前实现既不弹出已同步处理的请求，也没有容量上限；`pending_count` 因而表示累计提交数量，而不是仍待网络发送的数量。

请求和响应拥有自己的 `Vec<u8>`。服务端把请求诊断字段克隆到 `DiagnosticContext`，使等待图不借用调用方缓冲区。`convert_error` 消费 `DeadlockError` 并移动其 `wait_chain`，避免再次复制整条链。

## 依赖与调用关系

上游已验证调用边为 `server.rs::Server::detect_deadlock -> deadlock.rs::DetectorServer::detect`。`Server::new` 同时调用 `DetectorServer::new`，因此每个 mock TiKV `Server` 实例拥有自己的等待图。`deadlock_test.rs` 也直接构造 `DetectorServer`，验证协议转换。

下游关键调用边为：

- `DetectorServer::new -> detector.rs::Detector::new`；
- `DetectorServer::detect(Detect) -> Detector::detect -> convert_error`（仅成环时转换）；
- `DetectorServer::detect(CleanUpWaitFor) -> Detector::clean_up_wait_for`；
- `DetectorServer::detect(CleanUp) -> Detector::clean_up`；
- `DetectorClient::submit -> DetectorServer::detect -> DeadlockWaiterManager::wake_up_for_deadlock`（最后一条仅在成环时发生）。

RustCodeGraph 将目标文件列为 30 个符号，并报告 `server.rs`、`deadlock_test.rs` 等文件对它的导入/使用；精确 `rg` 补充确认了服务端转发点位。全仓库精确搜索未发现 `DetectorClient` 的文件外构造、`DeadlockWaiterManager` 实现或 `pending_count` 使用，因此不要把这条客户端链误认为当前生产路径。

## 错误处理与边界

该 API 不返回 `Result`。正常无环、清理操作和未知于响应层的“无死锁”都表现为 `None`；只有检测到闭环时返回 `Some(DeadlockResponse)`。底层算法的 `DeadlockError` 是检测结果载荷，并非 I/O 异常。

所有同步原语都使用 `expect` 处理中毒：`DetectorClient::submit`/`pending_count` 在客户端队列锁中毒时以 `deadlock client queue poisoned` panic；底层 `Detector` 在等待图锁中毒时也会 panic。本文件没有恢复、日志或退避逻辑。

边界语义由 `detector.rs` 决定：重复的 `(wait_for_txn, key_hash)` 不新增边；同一事务对同一目标但不同 key hash 可形成多条边；过期边不能参与成环判断；触发闭环的最后一条边不会落入等待图。清理不存在的边或事务是幂等的无操作。

`convert_error` 清空顶层 `response.entry` 的诊断字段是刻意对齐 Go `convertErrToResp` 的协议行为，不应被当成诊断信息丢失缺陷；完整诊断仍位于 `wait_chain`。角色值没有类型封装，`change_role` 可存入 0/1 以外的值，此时 `is_leader` 返回 false。

## 并发与资源生命周期

`DetectorServer` 通过 `Detector` 内部的 `Mutex` 串行化等待图变更；角色使用 `AtomicI32`，读取为 Acquire、写入为 Release。`DetectorClient` 的请求历史由 `Mutex<Vec<_>>` 保护，`server` 和 `waiter_manager` 由 `Arc` 持有，回调 trait 要求 `Send + Sync`，所以客户端可以放入多线程共享容器中使用。

`submit` 在持有 `pending` 锁时只执行克隆与 push，随后释放锁，再进行检测和回调；因此底层图锁或回调不会与队列锁嵌套。回调是同步执行的：慢回调会延长调用者的 `submit`，回调 panic 也会直接向上传播。

本文件不创建线程、任务、channel、socket 或取消令牌，也没有显式 `Drop`。资源生命周期由 `Arc` 的最后一个强引用和各结构默认析构管理。`pending` 历史没有清空接口，长生命周期且高请求量的客户端会线性保留请求及其诊断字节串，这是扩展时需要评估的内存风险。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/store/mockstore/unistore/tikv/deadlock.go`。服务端语义基本逐项对应：三种请求类型、默认 `Detector` 参数、错误到响应的字段映射、Follower/Leader 常量和原子角色访问均可找到对应实现。独立 Rust 测试 `deadlock_test.rs::deadlock_response_entry_matches_go_proto_conversion` 特别验证了 Go 转换的重要细节：触发边的数值字段保留、顶层 key/tag 为空、闭环 key hash 与等待链诊断字段正确。

客户端实现则是有意可见的缩减边界，不能视为等价网络移植。Go `DetectorClient` 通过 PD 查找首个 Region Leader，建立 gRPC 双向流，使用容量 10,000 的 channel，分别运行发送/接收循环，在连接失败时等待 3 秒重建，并通过 `lockwaiter.Manager` 唤醒等待方。Rust `DetectorClient` 仅持有本地 `Arc<DetectorServer>`，同步调用并把请求追加到无界历史 `Vec`；没有 PD、gRPC、后台循环、重连、取消或显式连接关闭。

核心图算法应同时参考 `detector.rs` 与 Go `detector.go`。`detector_test.rs` 和 Go `detector_test.go` 共同覆盖等待链顺序、断环、重复边、按边/按事务清理以及 TTL 过期行为。Rust 算法额外使用 `visited` 集合避免 DFS 在非源环上无限递归；该细节属于 `detector.rs`，本文件只是透传结果。

## 扩展指南

- 新增请求类型时，必须同时扩展 `RequestType`、`DetectorServer::detect` 的穷尽匹配和必要的客户端构造入口；若与 Go 协议对齐，还需核对 `deadlock.go`/kvproto 的枚举与字段语义。
- 修改响应字段或转换规则时，优先修改私有 `convert_error`，并在独立文件 `pkg/store/mockstore/unistore/tikv/deadlock_test.rs` 增加回归断言；不要把 Rust 测试内嵌到生产源文件。
- 改变 TTL、紧急容量或清理间隔时，修改 `DetectorServer::new`，同步核对 Go `NewDetectorServer` 的常量，并用 `detector_test.rs` 验证底层过期行为。
- 若要真正接入 `DetectorClient`，应先明确是保持进程内同步语义，还是补齐 Go 的远程 Leader 路由。后者涉及 PD、传输、重连、取消、背压和关闭生命周期，超出本文件当前依赖和接线范围，不能只把 `pending` 换成 channel 就宣称等价。
- 若角色要成为强制路由条件，应在服务端/RPC 边界明确 Follower 行为，并新增角色切换测试；直接在 `detect` 中静默丢弃请求会改变现有调用契约。
- 性能上应关注请求诊断字节复制、等待图全局互斥和客户端历史的无界增长；兼容性上应保持 wait-chain 顺序、deadlock key hash 来源以及顶层响应 entry 清空诊断字段的 Go 语义。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；目标 `deadlock.rs` 被识别为 30 个符号，并显示 `server.rs`、`deadlock_test.rs` 等使用方。
- RustCodeGraph 源码节点：`deadlock.rs` 全文件；`detector.rs` 的 `Detector::{new, detect, do_detect, clean_up, clean_up_wait_for, active_expire}`；`server.rs` 的 `Server::{new, detect_deadlock}`。
- crate 与装配：`pkg/store/mockstore/unistore/tikv/Cargo.toml`、`pkg/store/mockstore/unistore/tikv/lib.rs`。
- Rust 独立测试：`pkg/store/mockstore/unistore/tikv/deadlock_test.rs`、`pkg/store/mockstore/unistore/tikv/detector_test.rs`。
- Go 对照与测试：`pkg/store/mockstore/unistore/tikv/deadlock.go`、`pkg/store/mockstore/unistore/tikv/detector.go`、`pkg/store/mockstore/unistore/tikv/detector_test.go`。
- 精确调用搜索：`Server::detect_deadlock` 在 `server.rs:695-698` 转发；全仓库未找到 `DetectorClient::new` 的文件外调用、`DeadlockWaiterManager` 实现或该类型 `pending_count` 的使用。

这是纯文档分析，没有运行 Cargo 或代码测试。结构验证要求目标文档存在，并且上述十一个固定二级标题各出现一次。
