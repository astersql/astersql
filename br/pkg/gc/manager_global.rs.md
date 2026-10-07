# `br/pkg/gc/manager_global.rs`

## 文件定位

`manager_global.rs` 属于 `astersql-br-pkg-gc` library crate；crate 根由 `br/pkg/gc/Cargo.toml` 的 `[lib] path = "lib.rs"` 指向 `br/pkg/gc/lib.rs`，后者以 `pub mod manager_global` 编入本文件。它是 `Manager` 抽象的“全局/Nullspace”实现，与 `manager_keyspace.rs` 的 keyspace 级 GC barrier 实现并列。

正常构造路径是 `manager.rs::NewManager(pd_client, keyspace_id)`：仅当 `keyspace_id == NullspaceID` 时调用 `newGlobalManager`，否则构造 `keyspaceManager`。因此本文件不负责判断运行模式，也不直接对外重导出具体类型；调用方通常持有 `Arc<dyn Manager>`。RustCodeGraph 对当前 Rust 树的直接调用检索主要落在 `br/pkg/gc/manager_test.rs`、`br/pkg/gc/parity_test.rs` 和 `br/pkg/gc/mock_test.rs`，未显示 BR 生产入口直接构造该具体类型；所以它是已经实现并由 crate 工厂接线的实现层，但不能据此断言所有 Go BR 生产调用链都已迁移到该 Rust crate。

## 核心职责

本文件把 `manager.rs::Manager` 的三个操作映射到旧式、整集群作用域的 PD GC API：

- 查询当前 GC safepoint：调用 `PdClient::UpdateGCSafePoint(ctx, 0)`，用零值表达只读查询。
- 注册或续约 BR 服务 safepoint：调用 `PdClient::UpdateServiceGCSafePoint(ctx, ID, TTL, BackupTS - 1)`。
- 删除服务 safepoint：调用同一个旧 API，但固定传入 `TTL = 0`、`safe_point = 0`。

这层存在的主要理由是兼容旧版 BR 的 global GC 保护语义。它没有使用 `GCStatesClient` 或 keyspace barrier；作用域隔离由 `NewManager` 的 `NullspaceID` 分支保证。设置和删除成功后还可触发文件信号探针，用于集成测试区分 global 与 keyspace 路径。

## 主要符号

- `pub struct globalManager { pub(crate) pd_client: Arc<dyn PdClient> }`：唯一业务字段是线程安全共享的 PD 客户端 trait object。类型公开，但字段仅 crate 内可见；惯用入口仍是 `NewManager`。
- `pub fn newGlobalManager(Arc<dyn PdClient>) -> globalManager`：纯包装工厂，不发 RPC、不校验客户端，也不创建后台任务。
- `fn failpoint_signal_file(name: &str) -> Option<String>`：读取同名环境变量；变量不存在或值为空时返回 `None`。它是 Go `failpoint.Inject` 的本地替代，不是通用 failpoint 框架。
- `Manager for globalManager::GetGCSafePoint`：以参数 `0` 调用 `UpdateGCSafePoint`，成功返回 PD 给出的 safepoint，失败经 `Trace` 返回。
- `Manager for globalManager::SetServiceSafePoint`：提交 `sp.ID`、`sp.TTL` 和 `sp.BackupTS.wrapping_sub(1)`；成功时处理 set 探针，并在 PD 返回值高于请求值且 TTL 为正时告警。
- `Manager for globalManager::DeleteServiceSafePoint`：提交 `(sp.ID, 0, 0)`；成功时处理 delete 探针。

文件没有模块级常量、枚举、额外 trait 或条件编译项。`Duration` 仅用于 set 探针开启后的三秒观察窗口。

## 执行流程

构造与调用主链如下：

1. `manager.rs::NewManager` 接收共享 `PdClient` 与 `KeyspaceID`。
2. `keyspace_id == NullspaceID` 时，工厂调用 `newGlobalManager`，再包装为 `Arc<dyn Manager>`；构造阶段没有 I/O。
3. 上层 GC 保护逻辑通过 `Manager` trait 调用本文件的实现。

`GetGCSafePoint` 的流程是：调用 `UpdateGCSafePoint(ctx, 0)`；成功值原样返回；错误交给 `safepoint.rs::Trace`。当前 `Trace` 只保留原错误对象，不额外抓取 Rust backtrace。

`SetServiceSafePoint` 的流程是：

1. 计算请求 safepoint 为 `sp.BackupTS.wrapping_sub(1)`，调用旧式 `UpdateServiceGCSafePoint`。
2. 仅在 RPC 成功时读取 `hint-gc-global-set-safepoint` 环境变量；若得到非空路径，向文件写入 `sp.ID`。写文件失败只打印告警，不改变 RPC 的成功结果。
3. 只要探针变量为非空路径，不论写文件是否成功，当前线程都会休眠三秒，给集成脚本留出观察窗口。
4. RPC 成功且 `last_safe_point > BackupTS - 1`、同时 `TTL > 0` 时打印保护可能丢失的告警；该条件不转化为错误。
5. 最终把 PD 错误经 `Trace` 返回，否则返回 `Ok(())`。

`DeleteServiceSafePoint` 的流程是：调用 `UpdateServiceGCSafePoint(ctx, &sp.ID, 0, 0)`；仅成功时尝试向 `hint-gc-global-delete-safepoint` 指定的文件写入 ID；文件错误只告警；最后返回 PD 调用结果。删除探针没有额外 sleep。

## 数据与状态

`globalManager` 自身不保存 safepoint、TTL、定时器或调用状态；持久状态归 PD 管理。它只持有 `Arc<dyn PdClient>`，所以克隆与共享发生在客户端引用层，而不是复制 PD 状态。

输入 `BRServiceSafePoint` 定义在 `safepoint.rs`，包含 `ID: String`、秒单位的 `TTL: i64` 和 `BackupTS: u64`。设置请求使用 `BackupTS - 1`，让 BR 备份时间戳之前的数据不被 GC；当前 Rust 代码用 `wrapping_sub(1)`，所以 `BackupTS == 0` 会得到 `u64::MAX`，不会 panic。这个边界与 Go 无符号减法的回绕结果一致，但调用者仍应提供有效 TSO，不能把回绕当成输入校验。

环境变量 `hint-gc-global-set-safepoint` 与 `hint-gc-global-delete-safepoint` 是进程级可变测试状态；变量值被解释为信号文件路径，文件内容恰为服务 ID 的原始字节。代码不会创建父目录，也不会清理信号文件。

## 依赖与调用关系

上游关系：

- `br/pkg/gc/lib.rs` 声明本模块，并重导出 `Manager`、`NewManager`、`NullspaceID` 和 safepoint 数据类型，而未重导出 `globalManager`。
- `br/pkg/gc/manager.rs::NewManager` 是具体实现的直接生产工厂边：`NullspaceID -> newGlobalManager -> Arc<dyn Manager>`。RustCodeGraph 的 `NewManager` 节点也记录了这条调用边。
- `br/pkg/gc/safepoint.rs::CheckGCSafePoint` 与 `StartServiceSafePointKeeper` 消费 `Manager` trait，因此可间接调用本实现；它们不依赖 `globalManager` 具体类型。

下游关系：

- `manager.rs::PdClient::UpdateGCSafePoint`：全局 safepoint 查询。
- `manager.rs::PdClient::UpdateServiceGCSafePoint`：服务 safepoint 设置、续约与删除。
- `safepoint.rs::{BRServiceSafePoint, Context, SharedError, Trace}`：参数、错误契约和日志字段格式。
- 标准库 `std::env`、`std::fs`、`std::thread`、`std::time::Duration`：测试探针及观察窗口。

`br/pkg/gc/Cargo.toml` 的生产依赖只有相邻的 `astersql-br-pkg-errors`；PD 在本 crate 中通过本地 `PdClient` trait 抽象，不由该 manifest 直接引入外部 PD/grpc crate。manifest 注释还明确生产库保持无 `kvproto/grpcio` 依赖，重型 mockstore dev-dependencies 已移除。

## 错误处理与边界

三个 PD 调用的错误都通过 `Trace` 返回；当前 Rust `Trace` 是原错误透传，因此保留具体错误及其文本，但不像 Go `errors.Trace` 那样补充调用栈。`parity_test.rs` 用 `pd unavailable` 验证 set/get 错误文本仍能被上层观察。

信号文件写入属于旁路诊断：`std::fs::write` 失败只通过 `eprintln!` 告警，不覆盖 PD 成功，也不返回给调用者。相反，PD 失败时完全不触发对应信号文件逻辑，以免测试把失败请求误判为已生效。

值得注意的输入与行为边界：

- `SetServiceSafePoint` 不像 keyspace 实现那样在 `TTL <= 0` 时显式转调删除；它把 TTL 原样传给旧 PD API。真正明确的删除路径是 `DeleteServiceSafePoint` 的 `(0, 0)` 参数组合。
- `TTL > 0` 只控制“PD 返回更高 safepoint”告警是否有保护意义，不校验 TTL 合法性。
- `last_safe_point` 高于请求值只告警，调用仍成功；上层若需要把保护丢失视为失败，必须在契约层另行设计，不能仅修改日志。
- `Context` 是否取消由 `PdClient` 的实现解释；本文件不预检查 `Context::Done()`。
- 文件路径来自环境变量，未做路径规范化或安全校验；该机制应限制在受控测试环境。

## 并发与资源生命周期

`Manager` 和 `PdClient` trait 都要求 `Send + Sync`，`Arc<dyn PdClient>` 允许多个调用者共享一个客户端。本文件没有内部锁，也没有可变字段；并发正确性和 RPC 排序由具体 `PdClient`/PD 保证。多个调用可以并发执行，代码不在本地串行化同一服务 ID 的更新。

构造和销毁没有显式资源动作：`newGlobalManager` 增加客户端的 `Arc` 所有权，最后一个引用释放时才由客户端自身清理资源。本文件不启动 keeper；周期续约线程的生命周期位于 `safepoint.rs::StartServiceSafePointKeeper`。

探针会引入两个并发注意点：set 探针用同步 `std::thread::sleep` 阻塞当前调用线程三秒；并发请求若写同一路径会互相覆盖，因为 `std::fs::write` 不是追加协议，也没有锁。环境变量是进程级状态，测试必须隔离设置与清理，避免并行用例串扰。

## 与 Go 版本的对应关系

直接对照文件是 `br/pkg/gc/manager_global.go`。类型、工厂和三项接口逐一对应：Go `globalManager.pdClient` 对应 Rust `pd_client`；Go `newGlobalManager` 返回指针对应 Rust 值再由 `NewManager` 包入 `Arc`；三项 PD 参数保持一致。

已确认的语义对齐包括：查询使用 `UpdateGCSafePoint(ctx, 0)`；设置使用 `BackupTS-1`；删除使用 `UpdateServiceGCSafePoint(ID, 0, 0)`；PD 错误向上传播；set/delete 探针仅在 PD 成功后运行；set 探针保留三秒观察窗口；PD 实际 safepoint 更高且 TTL 为正时仅告警。

实现形态上的差异包括：

- Go 使用编译/运行时 failpoint 注入并从 `failpoint.Value` 取路径；Rust 使用同名环境变量。
- Go 使用结构化 `log`/`zap`；Rust 使用 `eprintln!`，且省略普通 debug 成功日志。
- Go `errors.Trace` 可携带栈信息；Rust `Trace` 当前只透传 boxed error。
- Go 文件用编译期接口断言 `var _ Manager = (*globalManager)(nil)`；Rust 的 `impl Manager for globalManager` 已由类型系统直接检查。
- Rust 的减法显式写为 `wrapping_sub(1)`，把 Go `uint64` 在零值时的回绕行为写清楚。

`br/pkg/gc/manager_test.go::TestNewManager`、`TestGlobalManager` 与 Rust `manager_test.rs::test_new_manager`、`test_global_manager` 对应，验证 global/keyspace 隔离和 Set/Delete/Get。`parity_test.rs::go_rust_public_contract_matches` 进一步断言全局 PD 调用序列精确为 `("br-test-global", 300, 999)` 后接 `("br-test-global", 0, 0)`，并验证 set/get 错误传播。

## 扩展指南

若扩展全局 GC 行为，优先按职责选择接入点：改变 global RPC 参数或响应判断时修改本文件相应 `Manager` 方法；改变 global/keyspace 选择条件时修改 `manager.rs::NewManager`；改变续约、取消或超时策略时修改 `safepoint.rs`，不要把后台生命周期塞进 `globalManager`；扩展 PD 能力时先在 `manager.rs::PdClient` 增加最小 trait 面，再同步 mock。

任何行为修改都应同时更新独立测试文件，而不是在生产 `.rs` 中内嵌测试：

- `br/pkg/gc/manager_test.rs`：Set/Delete/Get 与作用域路由的主回归。
- `br/pkg/gc/parity_test.rs`：精确 RPC 参数序列、Go/Rust 公共契约和错误传播。
- `br/pkg/gc/mock_test.rs`：需要记录新调用或注入新错误时扩展 mock `PdClient`。
- Go 基线 `br/pkg/gc/manager_test.go`：确认移植逻辑仍与上游意图一致；若 Go 行为本身改变，应同步核对 `manager_global.go`。
- 涉及 global/keyspace 路径识别时，核对 `tests/realtikvtest/brietest/harness.rs` 中四个 GC 信号探针的隔离约定。

兼容性风险集中在旧 PD API 参数、`BackupTS-1`、TTL=0 删除约定和错误透传；性能风险主要是 set 探针同步 sleep 及信号文件 I/O。不要把告警条件擅自升级为错误，也不要在未证明调用方契约的情况下把 global 路径改成 keyspace barrier API。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter br/pkg/gc` 找到本 crate 的生产与独立测试文件。
- RustCodeGraph `node --file br/pkg/gc/manager_global.rs --offset 1 --limit 260`：读取目标文件完整 126 行和全部 7 个符号。
- RustCodeGraph `query GlobalGCManager`、`query NewGlobalGCManager`、`node NewManager`、`node GetGCSafePoint`、`node SetServiceSafePoint`、`node DeleteServiceSafePoint`：核对 Rust/Go 对应定义；图中明确给出 `NewManager -> newGlobalManager`，以及 Rust set/delete 到 `failpoint_signal_file` 的调用边。宽泛 `explore` 受仓库大量同名符号干扰，未把其非目标结果作为结论。
- 源与边界：`br/pkg/gc/manager.rs`、`br/pkg/gc/lib.rs`、`br/pkg/gc/safepoint.rs`、`br/pkg/gc/Cargo.toml`。
- Go 对照：`br/pkg/gc/manager_global.go`、`br/pkg/gc/manager_test.go`。
- Rust 独立测试：`br/pkg/gc/manager_test.rs`、`br/pkg/gc/parity_test.rs`、`br/pkg/gc/mock_test.rs`；集成探针约定还参考 `tests/realtikvtest/brietest/harness.rs`。

本任务是纯文档分析，按计划不运行 Cargo。结构验收要求本文恰有上述十一个固定二级标题；事实复核重点是工厂分派、三个 PD 调用参数、成功后探针、错误透传及 Go/Rust 测试对应关系。
