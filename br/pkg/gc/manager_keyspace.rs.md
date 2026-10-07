# `br/pkg/gc/manager_keyspace.rs`

源文件：[manager_keyspace.rs](./manager_keyspace.rs)

## 文件定位

本文件是 `astersql-br-pkg-gc` crate 中按 keyspace 隔离的 GC 屏障实现。crate 入口 `br/pkg/gc/lib.rs` 将它声明为 `manager_keyspace` 模块，但没有直接重导出其具体类型；外部调用方通常通过 `manager::NewManager` 获得 `Arc<dyn Manager>`。`NewManager` 只在 `keyspace_id != NullspaceID` 时构造这里的 `keyspaceManager`，而 `NullspaceID` 走 `manager_global.rs` 的旧全局 service safepoint API。

`br/pkg/gc/Cargo.toml` 将该目录定义为库 crate（`[lib] path = "lib.rs"`），移植元数据指向 Go 包 `br/pkg/gc`。目标文件自身只依赖标准库以及同 crate 的 `manager`、`safepoint` 模块；Cargo 中唯一生产依赖 `astersql-br-pkg-errors` 由 crate 的其他模块使用，不是本文件的直接依赖。

## 核心职责

本文件把统一的 `Manager` 契约映射到 PD 的 keyspace 级 GC States API：

1. 在构造阶段通过 `PdClient::GetGCStatesClient(keyspace_id)` 得到已绑定 keyspace 的客户端。
2. 通过 `GCStatesClient::GetGCState` 读取该 keyspace 的 `GCSafePoint`。
3. 将正 TTL 的 BR service safepoint 写成 GC barrier，barrier ID 使用 `BRServiceSafePoint::ID`，barrier 时间戳使用 `BackupTS - 1`。
4. 将非正 TTL 的设置请求转换为删除，并支持显式按 ID 删除 barrier。
5. 在两个约定的环境变量存在时写测试信号文件，使集成测试能够区分 keyspace 路径与 global 路径。

因此它解决的是“备份期间只保护指定 keyspace 的历史版本”这一作用域问题，而不是推进 GC、计算备份时间戳或管理整个备份生命周期。后几项分别由 PD、上游调用方和 `safepoint.rs`/任务编排负责。

## 主要符号

- `keyspaceManager`：`Manager` 的 keyspace 实现。它包含 `Arc<dyn PdClient>`、`KeyspaceID` 和 `Arc<dyn GCStatesClient>` 三个 `pub(crate)` 字段。类型本身为 `pub`，但没有从 crate 根重导出，正常入口仍是 `NewManager`。
- `newKeyspaceManager(pd_client, keyspace_id) -> keyspaceManager`：同步工厂函数。它调用一次 `GetGCStatesClient`，把 keyspace 绑定到 `gc_client`，不发起读写 barrier 的 RPC。
- `failpoint_signal_file(name) -> Option<String>`：读取同名环境变量；未设置、非 Unicode 或空字符串都视为未启用。它是测试探针适配器，不是通用 failpoint 框架。
- `Manager::GetGCSafePoint`：调用 `GetGCState`，成功时只返回 `GCState::GCSafePoint`。
- `Manager::SetServiceSafePoint`：`TTL <= 0` 时转发到 `DeleteServiceSafePoint`；否则调用 `SetGCBarrier(ctx, id, BackupTS.wrapping_sub(1), TTL)`。
- `Manager::DeleteServiceSafePoint`：调用 `DeleteGCBarrier(ctx, id)`，忽略成功返回的可选 barrier 信息。

本文件没有模块级常量、枚举、独立 trait、条件编译项或内嵌测试。`Manager`、`PdClient`、`GCStatesClient`、`GCState`、`GCBarrierInfo` 和 `KeyspaceID` 均定义在 `br/pkg/gc/manager.rs`；`BRServiceSafePoint`、`Context`、`SharedError` 与 `Trace` 定义在 `br/pkg/gc/safepoint.rs`。

## 执行流程

构造路径从 `manager::NewManager` 开始：它比较传入 ID 与 `NullspaceID`，非空 keyspace 分支调用 `newKeyspaceManager`；后者向 `PdClient` 请求一个绑定 ID 的 `GCStatesClient`，并把三个句柄保存到结构体中。此后所有 GC 状态读写都经 `gc_client`，无需在每次调用中重复传 keyspace ID。

读取流程是单次调用：`GetGCSafePoint` 把上游 `Context` 原样传给 `GetGCState`，返回状态中的 `GCSafePoint`，不读取 `TxnSafePoint` 或 `GCBarriers`。

设置流程按以下分支执行：

1. 若 `sp.TTL <= 0`，立即调用本实现的 `DeleteServiceSafePoint` 并返回其结果，不调用 `SetGCBarrier`。
2. 若 TTL 为正，以 `sp.ID`、`sp.BackupTS.wrapping_sub(1)` 和秒数 TTL 调用 `SetGCBarrier`。
3. PD 调用成功后，如环境变量 `hint-gc-keyspace-set-barrier` 给出了非空路径，则写入 `keyspace=<id>\nid=<service-id>\n`；无论写入是否成功，随后睡眠 3 秒，为外部测试脚本保留观察窗口。
4. 读取返回的 `BarrierID`、`BarrierTS`、`TTL` 以保持与 Go 调试字段消费相对应，然后返回成功。

删除流程调用 `DeleteGCBarrier`；成功后，如 `hint-gc-keyspace-delete-barrier` 指向非空路径，则写入相同两行格式的信号，但不睡眠。PD 删除成功即视为方法成功，是否确实存在旧 barrier 由 `GCStatesClient` 的语义决定；契约测试明确覆盖了删除不存在 ID 返回成功的情况。

## 数据与状态

`keyspaceManager` 的长期状态是三个不可变字段：共享 PD 客户端、数值型 `keyspace_id` 和共享的 keyspace GC 客户端。`gc_client` 承担实际状态访问；`pd_client` 在构造完成后没有被本文件的方法再次读取，保留它与 Go 字段布局及诊断意图一致。真实 barrier 状态位于 PD，不缓存在本结构体中。

每次操作的值对象是 `BRServiceSafePoint { ID, TTL, BackupTS }`。`ID` 是 barrier 的稳定键；`TTL` 在此接口中按秒传递，正数表示设置/更新，零或负数表示删除；`BackupTS` 转成 barrier 时间戳时减一。Rust 使用 `wrapping_sub(1)` 明确复现 Go `uint64` 的下溢行为，所以 `BackupTS == 0` 会产生 `u64::MAX`，本层不做参数校验，最终是否接受由 `GCStatesClient`/PD 决定。

`SetGCBarrier` 返回的 `GCBarrierInfo` 不参与后续控制流，只读取字段以对应 Go 成功日志字段；`DeleteGCBarrier` 的 `Option<GCBarrierInfo>` 被忽略。信号文件不是业务状态，也不会影响 RPC 成功结果。

## 依赖与调用关系

上游直接接线是 `br/pkg/gc/manager.rs::NewManager`：`NullspaceID` 之外的 ID 都进入本文件。crate 根 `br/pkg/gc/lib.rs` 重导出 `NewManager` 与 `Manager` 契约，隐藏具体实现。仓库搜索显示，Rust 生产编排通常接收已构造的 `Arc<dyn Manager>`；真实 keyspace 接线的直接证据位于 `tests/realtikvtest/brietest/gc_keyspace_test.rs`，它把 `gc::NewManager(..., keyspace_id)` 注入 `MemMgr` 后运行 `RunBackup`。`br/pkg/task/backup.rs` 随后通过 `StartServiceSafePointKeeper` 周期设置安全点，并由清理守卫调用 `DeleteServiceSafePoint`。

下游依赖全部由 trait 隔离：

- `PdClient::GetGCStatesClient`：构造绑定 keyspace 的客户端。
- `GCStatesClient::GetGCState`：读取 keyspace GC 状态。
- `GCStatesClient::SetGCBarrier`：写入或更新 barrier。
- `GCStatesClient::DeleteGCBarrier`：按 service ID 删除 barrier。
- `Trace`：包装并返回下游错误。
- `std::env`、`std::fs::write`、`std::thread::sleep`：仅服务于集成测试探针。

RustCodeGraph 的文件视图确认 `manager.rs` 由 RealTiKV keyspace runtime 等文件使用；精确源码关系显示 `NewManager -> newKeyspaceManager -> PdClient::GetGCStatesClient`，以及三个 `Manager` 方法到相应 `GCStatesClient` 方法的调用。由于同名 Go/Rust 符号较多，宽泛图查询会混入 Go 节点，文档中的具体调用边以带路径的节点输出和源码交叉核验为准。

## 错误处理与边界

三个 PD/GCStates 操作的错误都经 `Trace(err)` 原样向上返回；`SetGCBarrier` 失败时不会写信号文件或进入 3 秒观察等待，`DeleteGCBarrier` 失败时同样不会写删除信号。`GetGCSafePoint` 不提供默认值或重试。

信号文件写入是刻意的 best effort：失败只向标准错误输出警告，业务方法仍返回成功。设置探针即使写文件失败也会睡眠 3 秒；删除探针没有等待。环境变量为空、缺失或不可解码时探针完全跳过。

本层不校验空 barrier ID、`BackupTS == 0`、过大 TTL，也不判断返回 barrier 是否与请求一致。它只实现 Go 对齐的参数变换和错误传播，具体合法性属于 PD 客户端边界。`TTL <= 0` 是唯一在 RPC 前处理的输入规则。删除不存在的 barrier 能否成功取决于客户端契约；当前 Rust 契约测试的 mock 与 Go 行为均把它视为成功。

## 并发与资源生命周期

`keyspaceManager` 通过 `Arc` 持有两个 trait 对象，而 `PdClient`、`GCStatesClient` 和 `Manager` 都要求 `Send + Sync`，所以同一个管理器可以安全地被 keeper 或任务线程共享。本文件自身没有 `Mutex`、原子变量、通道、异步任务或后台线程，也没有维护可变缓存；并发一致性和 barrier 更新的原子性由具体 PD 客户端负责。

构造时创建/取得 `gc_client`，管理器最后一个 `Arc` 被释放时两个客户端引用随结构体一起释放；本文件没有显式 `close`。`Context` 仅借用并传给下游，所有权不转移。`SetServiceSafePoint` 中的 3 秒阻塞睡眠只在设置探针启用时发生，会占用当前调用线程，这是测试观察机制而非正常生产路径。

业务资源生命周期由上游控制：`StartServiceSafePointKeeper` 负责按 TTL 刷新，`br/pkg/task/backup.rs` 的清理守卫负责在正常结束等条件下删除。仅构造 `keyspaceManager` 不会自动设置、刷新或删除 barrier。

## 与 Go 版本的对应关系

直接对照文件是 `br/pkg/gc/manager_keyspace.go`。结构字段、构造时绑定 `GCStatesClient`、`GetGCState().GCSafePoint`、`TTL <= 0` 转删除、`BackupTS - 1`、两个探针名、信号内容以及设置探针后的 3 秒等待均保持一致。Rust 的 `Arc<dyn ...>` 对应 Go 接口值，`SharedError + Trace` 对应 Go 的 `errors.Trace`。

可见差异主要是语言与适配层差异：

- Go 把 TTL 秒数转换为 `time.Duration` 后传给 PD；Rust 的本地 `GCStatesClient` trait 直接接收 `i64` 秒数。
- Go 使用编译期 failpoint 注入并从注入值取路径；Rust 直接读取与 failpoint 同名的环境变量，服务于当前 Rust 集成测试进程。
- Go 使用结构化 debug/warn 日志；Rust 省略成功 debug 日志，文件写入失败用 `eprintln!`，并用一次字段读取保留返回信息的消费语义。
- Go 构造函数返回指针；Rust 返回值随后由 `NewManager` 包入 `Arc<dyn Manager>`。
- Go 的 `uint64` 减法自然遵循无符号语义；Rust 显式使用 `wrapping_sub`，避免 debug 构建在零值下溢时 panic。

这些差异没有改变当前 Manager 契约，但探针启用方式和日志可观测性并非逐字等同，扩展测试或运维诊断时需要明确区分。

## 扩展指南

若新增 keyspace GC 操作，优先先扩展 `br/pkg/gc/manager.rs` 中的 `GCStatesClient`/`Manager` 最小契约，再在本文件实现映射；如果只有 keyspace 内部需要的辅助逻辑，则保持私有，避免把具体管理器暴露为外部稳定 API。修改工厂选择规则时同步检查 `NewManager` 的 `NullspaceID` 分支，防止 global 与 keyspace 状态串写。

修改 barrier 参数时必须保持三项不变量：绑定的 keyspace 不变、service ID 原样传递、备份保护点仍与 Go 约定一致。特别注意 `BackupTS - 1` 的零值边界和 TTL 单位；任何改为 `Duration`、饱和减法或提前校验的方案都会产生兼容性差异，需要同时更新 Go 对照或明确记录偏差。

测试应继续放在独立文件而非本源文件中。最接近的覆盖面是：

- `br/pkg/gc/manager_test.rs`：工厂路由、keyspace 隔离、设置、TTL=0 删除、显式删除和读取。
- `br/pkg/gc/parity_test.rs`：参数、删除不存在项与 set/delete/get 错误传播。
- `br/pkg/gc/mock_test.rs`：可复用的 PD/GCStates 内存测试替身。
- `tests/realtikvtest/brietest/gc_keyspace_test.rs` 与 `gc_keyspace_runtime.rs`：真实 PD/TiKV RPC、探针和备份清理链。
- Go 回归基线 `br/pkg/gc/manager_test.go` 与 `tests/realtikvtest/brietest/gc_keyspace_test.go`。

新增字段或探针还应评估：`Arc` 共享下的线程安全、同步文件 I/O/睡眠对调用延迟的影响、PD API 版本兼容、错误是否仍保留根因，以及 checkpoint 失败时上游是否应保留 barrier。不要在这里复制 keeper 或备份清理逻辑。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件，目标文件在索引内。
- RustCodeGraph `files --filter br/pkg/gc`：确认目标、模块入口、Go 对照与独立测试文件集合。
- RustCodeGraph `node --file br/pkg/gc/manager_keyspace.rs --offset 1 --limit 400`：核对本文件完整 140 行及全部 7 个索引符号。
- RustCodeGraph 对 `KeyspaceManager`、`newKeyspaceManager` 的查询，以及 `manager.rs`/`lib.rs` 文件节点：核对工厂分派、trait 定义和模块导出关系。宽泛 `explore` 因跨语言同名符号产生噪声，未将模糊匹配作为独立事实依据。
- `br/pkg/gc/Cargo.toml`：核对 crate 名、库入口、Go 包移植元数据和依赖边界。
- `br/pkg/gc/manager_keyspace.go`：逐项核对 Go 实现语义。
- `br/pkg/gc/manager_test.rs`、`br/pkg/gc/parity_test.rs`、`br/pkg/gc/mock_test.rs`：核对作用域隔离、时间戳、TTL 删除、缺失项删除和错误传播。
- `tests/realtikvtest/brietest/gc_keyspace_test.rs`、`tests/realtikvtest/brietest/gc_keyspace_runtime.rs`：核对真实 keyspace manager 注入、PD RPC、探针内容、global 路径未触发和备份结束删除 barrier。
- `br/pkg/task/backup.rs`：核对 keeper 设置与清理守卫删除的上游生命周期。
- `rg` 对 `NewManager`、`SetGCBarrier`、`DeleteGCBarrier` 和两个探针名的仓库检索：确认直接引用及相关测试位置。

本任务只新增说明文档，没有修改或执行 Rust/Go 代码。按任务约束不运行 Cargo；结构检查用于确认目标文件存在且恰有规定的 11 个二级章节，人工复核用于排除把测试适配行为误写为一般生产保证。
