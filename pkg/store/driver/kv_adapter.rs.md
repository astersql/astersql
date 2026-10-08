# `pkg/store/driver/kv_adapter.rs`

## 文件定位

本文件是 `astersql-store-driver` crate 内部的 canonical KV 适配层。`pkg/store/driver/lib.rs` 以私有模块 `mod kv_adapter` 装载它；外部并不直接构造本文件中的实现，而是通过公开的 `TikvStore` 和 `kv::Storage`、`kv::Client`、`kv::Transaction`、`kv::Snapshot` 等 trait 使用它。`pkg/store/driver/Cargo.toml` 表明该 crate 直接依赖 `astersql-kv`、`astersql-store-copr`、`astersql-store-driver-txn` 和固定 tag `v0.4.2-aster.10` 的 `tikv-client`，这正对应本文件连接统一 KV 接口、coprocessor 层与 client-rust 事务客户端的三侧边界。

文件不是单纯转发门面：它实现事务、快照、内存写缓冲镜像、分页扫描、coprocessor 请求/响应转换、部分 PD/TiFlash 管理能力，以及 `TikvStore` 的完整 `kv::Storage` 接口。真实连接、runtime、RegionCache 和基础 store 生命周期仍由相邻的 `client_runtime.rs`、`tikv_driver.rs` 与 `astersql-store-copr` 提供。

## 核心职责

- 把 `TikvStore` 接入 canonical `kv::Storage` 与 `kv::Client`。主要入口是 `impl kv::Storage for TikvStore`、`impl kv::Client for TikvStore` 和 `begin_transaction`。
- 将 client-rust 的 `Transaction`/`Snapshot` 包装成 canonical `ClientTransaction`/`ClientSnapshot`，统一值、时间戳、错误和选项语义。
- 用 `ClientIterator`、`scan_region_page`、`scan_snapshot_page`、`scan_transaction_page` 实现按需分页且不跨 Region 的正向/反向扫描。
- 用 `ClientMemBuffer` 保存事务本地写集、flags 和 statement staging 的镜像，向上层暴露 `kv::MemBuffer` 观察接口；真正的提交、回滚和锁操作仍落到 client-rust transaction handle。
- 用 `cop_request`、`CopResponse`、`CopResultSubset` 在 canonical coprocessor 类型与 `astersql-store-copr` 类型之间转换，并保留分页、限流、runaway/resource-control 和运行时证据。
- 通过 PD/TiFlash HTTP、RegionCache、SST importer 和 keyspace codec 补齐 DDL、TTL、TiFlash placement/progress、Region range 与 SST 导入相关的 storage 能力。

## 主要符号

- `SharedRuntime = Arc<RwLock<ClientRuntime>>`：store 级共享 runtime；同步 trait 方法通过它取得 Tokio runtime 并 `block_on` client-rust future。
- `CLIENT_SCAN_PAGE_SIZE`、`configured_scan_batch_size`：默认页大小为 256；与 client-go scanner 一致，配置值 0/1 回退默认值，支持可安全转换的 `usize`、`u32`、`i32`、`u64`。
- `snapshot_timestamp`：把 `kv::MaxVersion` 解析为一次 PD 当前时间戳；普通历史版本不访问 PD；超过 client-rust 有符号范围时返回错误而非截断。
- `ClientIterator`/`PagingState`/`ScanPageResult`：保存当前页、位置、下一段上下界、方向与 exhausted 状态。`Next` 只在当前页耗尽后拉下一页，`Close` 阻止继续访问远端。
- `BufferState`/`ClientMemBuffer`：以 `BTreeMap<Vec<u8>, Option<Vec<u8>>>` 表示有序写入和删除 tombstone，以 `HashMap` 保存 key flags，以 stage 快照支持 `Staging`、`Release`、`Cleanup` 和 `InspectStage`。
- `ClientSnapshot`：持有共享 runtime、互斥的 client-rust snapshot handle、coprocessor store、原子扫描页大小和动态选项；实现 `Getter`、`Retriever`、`Snapshot`。
- `FailedSnapshot`：由于 `Storage::GetSnapshot` 不能返回 `Result`，保存创建快照时的 oracle/时间戳错误，并在每个可失败读取入口原样返回。
- `ClientTransaction`：组合 client-rust transaction handle、同 start-ts 快照、本地 mem-buffer、模式、时间戳、有效性、选项、schema checker、table info 和 statement stages；实现读写、锁、提交/回滚及 canonical transaction 辅助接口。
- `CopResultSubset`/`CopResponse`/`cop_request`：转换单次或批量 cop 响应及请求字段；`CopResponse::Next` 延迟传播构造/发送错误，`Close` 关闭底层 stream。
- `UnsupportedMppClient`、`ClientOracle`、`AdapterMemManager`：分别是当前 MPP 边界、oracle trait 接点和按 table 缓存 snapshot 读取结果的 store 内存管理器。MPP dispatch/connection 明确返回“不支持”，不是完整实现。
- `MPP_CLIENT`、`ORACLE` 与 `mem_manager()`：store 级静态适配对象；内存管理器由 `OnceLock` 延迟初始化。

## 执行流程

1. `TikvStore::Begin`（canonical trait）拒绝带指定 `StartTS` 的写事务选项，然后调用 `begin_transaction`。后者从 store 取得 `ClientRuntime` 和 transaction client，以 `ClientTransactionMode` 选项启动 client-rust transaction，记录 start-ts，并创建相同时间戳的 `ClientSnapshot`。
2. `ClientTransaction::Set`/`Delete` 先检查空值和可选 size limits，再在共享 runtime 上执行 `stage_put`/`stage_delete`，成功后同步更新 `ClientMemBuffer` 和内存 hook。`Get`/`BatchGet` 直接调用 transaction handle，从而保留 client-rust 的“读己之写”语义。
3. statement staging 同时在 client-rust handle 和本地 mem-buffer 建立 stage，`ReleaseStatement`/`CleanupStatement` 只接受当前栈顶 stage，并让远端 staging 与本地镜像一起提交或回退。
4. `Commit` 安装可选 schema lease checker，再调用 client-rust commit 并记录 commit-ts；schema checker 的原始 canonical 错误通过旁路 slot 保留。只读悲观事务改走 `Rollback` 释放锁，commit-ts 保持 0。`Rollback` 与成功提交都会令 `valid=false`。
5. `GetSnapshot` 调用 `ClientSnapshot::new`。若传入 `MaxVersion`，`snapshot_timestamp` 只向 PD 取一次 TSO，并固定用于该快照的全部页；失败则返回 `FailedSnapshot`。点读和 batch-get 在同一个 snapshot handle 上执行，点读保留 commit-ts。
6. `Iter`/`IterReverse` 构造闭包并由 `ClientIterator::paged` 预取首个有界页。`scan_region_page` 先用 RegionCache 定位当前 Region，将请求裁剪在该 Region 内；满页时从末键之后（正向）或末键之前的排他上界（反向）续扫，非满页时跳到 Region 边界。空页会继续推进，非推进的 Region 定位被当作错误，避免死循环。
7. `kv::Client::Send` 先以 `cop_request` 映射请求类型、StoreType、ranges、replica read、隔离级别、优先级、分页、超时、限流器和 runaway/resource-control 对象，再调用 coprocessor store。结果由 `CopResponse` 流式暴露，并由 `CopResultSubset` 提供数据、起始键、内存、耗时、read-pool 细节和 MVCC/读取证据。
8. DDL/TTL/TiFlash 辅助方法从 `TikvStore` 取得 PD 地址、TLS、RegionCache 或 keyspace codec；SST 导入委托 `sst_import::write_and_ingest_with_options`。这些路径与事务读写共享 store 配置，但不经过 `ClientTransaction`。

## 数据与状态

`ClientTransaction` 的权威远端状态在 `Arc<Mutex<ClientTransactionHandle>>` 中，本地 `ClientMemBuffer` 是 canonical 接口所需的镜像。写入只有在 client-rust staging 成功后才进入镜像；`BTreeMap` 保证迭代键序，`None` 表示删除。flags 与写集分开保存，stage 当前只快照 `writes`，因此扩充 stage 语义时必须确认 flags 是否也需要回滚。

快照与事务扫描的游标状态由闭包捕获的 runtime、handle、coprocessor store，加上 `PagingState` 的上下界和 exhausted 标记组成。每个迭代器只持有一页 entries，内存上界主要由页大小决定；`KeyOnly` 选择 client-rust 的 keys-only API。`ClientSnapshot::scan_batch_size` 用 `AtomicU32` 发布，其他动态选项受 `Mutex<HashMap<...>>` 保护。

事务还保存 `start_ts`、`commit_ts`、`valid`、模式、table-info cache、opaque vars、memory hook、size/schema 等动态选项。`AdapterMemManager` 是进程内、按 table-id/key 分层的 `RwLock<HashMap<...>>` 缓存，`Delete(table_id)` 才清除整表缓存；它不等价于 TiKV MVCC cache。

## 依赖与调用关系

上游入口主要是 canonical trait 调用。`pkg/session/runtime/control.rs`、`typed_adapter_bridge.rs`、DDL、meta 和 executor 等模块通过 `dyn kv::Storage`/`Snapshot` 使用 `Begin`、`GetSnapshot` 和 scan；`pkg/store/driver/tikv_driver.rs` 的固有 `Begin` 也直接调用 `kv_adapter::begin_transaction`。`pkg/store/driver/coprocessor_adapter_test.rs` 直接调用 crate-private `cop_request` 验证请求转换。

下游事务路径为 `TikvStore -> ClientRuntime -> tikv_client::{Transaction, Snapshot}`，错误经 `astersql_store_driver_txn::map_client_error` 归一化，值经 `canonical_value` 转换。扫描另外依赖 `astersql_store_copr::Store` 的 RegionCache 来切分 Region。coprocessor 路径为 `kv::Request -> cop_request -> copr::Store::get_client().send -> CopResponse`。DDL/管理路径还直接依赖 `reqwest`、`serde_json`、`region_split_config`、`sst_import` 和 `astersql_util_codec`。

RustCodeGraph 将该文件标为被 `pkg/executor/point_get.rs`、`pkg/session/runtime/canonical_table_reader.rs`、`pkg/store/driver/tikv_driver.rs` 等 10 个文件使用；精确 query 定位了 `begin_transaction`、`scan_region_page`、`cop_request`、`ClientIterator` 和 `ClientTransaction`。由于大量入口通过 trait 动态分派，图上的直接 callers 不完整，实际上游以 trait 实现、`rg` 引用和模块入口共同核对。

## 错误处理与边界

client-rust 错误统一经 `map_client_error` 转为 `kv::errors::SharedError`；适配器自身、锁中毒、HTTP、PD 和 codec 错误经 `adapter_error` 包装。不存在的 key 转为 `kv::ErrNotExist`，空 value 被 `ErrCannotSetNilValue` 拒绝。snapshot timestamp 超出 `i64::MAX`、Region 定位不包含当前键、Region/页边界不推进、返回行数超过 limit、无 runtime/RegionCache、非法 PD JSON 等都会显式失败。

已确认的能力边界包括：client-rust 公开 API 不能以指定 start-ts 打开可写事务；MPP 构建返回空任务而 dispatch/connection 返回不支持；named keyspace 的 TiFlash placement/progress 当前拒绝；`GetMinSafeTS` 固定返回 0；storage `SetOption`/`GetOption` 未保存值；transaction 的 `RollbackMemDBToCheckpoint` 为空、`IsPipelined` 为 false、`MayFlush` 为空成功；canonical `RequestSource` 当前是无字段桩。这些均应视为当前事实，不能在调用方文档中写成完整支持。

`GetSnapshot` 本身因 trait 约束不能返回错误：时间戳失败通过 `FailedSnapshot` 延迟到 `Get`、`BatchGet`、`Iter`、`IterReverse`。相对地，缺失 production runtime 或 RegionCache 使用 `expect`，属于 store 构造不变量，违反时会 panic。若 mutex/rwlock 中毒，大部分网络路径返回错误，但少数 option/cache 辅助路径使用 `unwrap`/`expect`。

## 并发与资源生命周期

store 的 `ClientRuntime` 被 `Arc<RwLock<_>>` 共享；事务和快照 handle 各由 `Arc<Mutex<_>>` 串行化，因为同步 canonical trait 会在持锁期间用单一 Tokio runtime `block_on` 异步 client-rust 操作。扫描迭代器闭包克隆 Arc，因此迭代器可独立持有底层资源直到 `Close` 或析构；`Close` 清空当前页、标记 exhausted，并阻止后续拉页。

`ClientTransaction` 的 lifecycle 是 begin 后 `valid=true`，成功 commit/rollback 后变为 false；只读悲观 commit 特别通过 rollback 释放锁。statement stage 是栈式结构，release/cleanup 必须针对最后一个活动 stage，防止远端与本地状态错位。`CopResponse::Close` 幂等，标准与 batch iterator 会被显式关闭，随后清空 stream。

`ClientSnapshot` 的 `scan_batch_size` 使用 Acquire/Release 原子访问，options 和 handle 使用 mutex；`ClientMemBuffer`、共享 runtime、内存 cache 使用 rwlock。HTTP 客户端设置 10 秒 timeout。文档没有发现显式后台任务由本文件创建；runtime 和网络对象的最终关闭仍归 `TikvStore::Close`/相邻 driver 生命周期负责。

## 与 Go 版本的对应关系

Rust 没有同名 `kv_adapter.go`；语义来源分散在 `pkg/store/driver/tikv_driver.go` 与 `pkg/store/driver/txn/{txn_driver.go,snapshot.go,scanner.go,unionstore_driver.go}`。Go `tikvStore.Begin` 由 client-go `KVStore.Begin` 后包装 `txn_driver.NewTiKVTxn`，`GetSnapshot` 包装 client-go snapshot；Rust 则在本文件直接用 client-rust handle 实现相同 canonical traits。`Name`、`Describe`、`CurrentVersion`、`GetLockWaits`、codec/cluster/keyspace 等 storage 方法对应 Go `tikvStore` 方法。

扫描语义刻意跟随 client-go：0/1 batch size 回落默认值、正反向半开范围、逐 Region 路由、keys-only 选项和按需取页。`kv_adapter_test.rs` 的测试名称和断言明确记录了这些对齐点。事务的 Get/Set/Delete/Iter/BatchGet/Commit/Rollback/LockKeys/SetOption 等接口对应 Go `tikvTxn`，snapshot 点读、批读和迭代对应 Go `tikvSnapshot`。

并非所有 Go 能力已经等价：Go `GetMPPClient` 返回真实 copr MPP client，而 Rust 返回 `UnsupportedMppClient`；Go store option 使用 `sync.Map`，Rust storage option 仍为空；Rust 的部分 checkpoint/pipelined API 是显式桩。另一方面，本文件还集中实现了 Rust client 需要的 runtime 同步桥、错误快照、PD/TiFlash HTTP 与 cop 类型转换，不能简单按某一个 Go 文件逐行对应。

## 扩展指南

- 新增 transaction/snapshot option 时，先确认 canonical option 的动态类型，再同步更新 `ClientTransaction::SetOption`、`ClientSnapshot::SetOption` 与 client-rust handle；扫描相关选项还需检查两个 scan page 函数。测试放在独立的 `pkg/store/driver/kv_adapter_test.rs`，不要内嵌到生产文件。
- 修改扫描必须保持半开区间、Region 内单请求、满页续键、反向排他边界和“每次必须推进”不变量；同步扩展正/反向、空页、Region 边界、提前 Close 与超量返回的测试。性能风险集中在 page size、锁持有期间 `block_on` 和不必要的整页复制。
- 新增事务状态时，要让 client-rust handle 与 `ClientMemBuffer` 镜像原子地保持一致，并同时考虑 statement release/cleanup、commit/rollback、内存 hook 和错误后的状态。若扩展 flags staging 或 checkpoint，不能沿用当前空实现而不增加回滚测试。
- 扩展 cop 字段时同时更新 `cop_request`、必要的 response 转换以及 `coprocessor_adapter_test.rs`；要区分 TiKV standard DAG 与 TiFlash batch cop 的合法组合，并保留限流器、runaway checker、RU 和 runtime evidence。
- 增加 PD/TiFlash/DDL 能力时复用 `TikvStore` 的 PD 地址和 TLS，明确 keyspace v1/v2 编码差异；HTTP/RegionCache 失败必须带上下文返回。真实集群验证应继续放在独立且默认忽略的测试中。
- 若要补齐 MPP、safe-ts、store options、pipelined flush 或 checkpoint，首先对照 Go 文件和 canonical trait 约束，再确认 client-rust 上游能力；这些是明确的迁移缺口，不应以假成功桩掩盖。

## 验证依据

- 源码与装配：`pkg/store/driver/kv_adapter.rs`（2489 行）、`pkg/store/driver/lib.rs`、`pkg/store/driver/Cargo.toml`。
- RustCodeGraph：`status` 显示索引包含 11467 个文件；`node --file pkg/store/driver/kv_adapter.rs` 分段读取源码；`query` 定位 `begin_transaction`、`scan_region_page`、`cop_request`、`ClientIterator`、`ClientTransaction`；`callees` 确认关键构造/错误转换边，但 trait 动态调用的 callers 需结合源码引用核对。
- Rust 测试：`pkg/store/driver/kv_adapter_test.rs` 验证 storage trait、PD store 计数、MaxVersion/历史时间戳、失败快照、batch size、KeyOnly、惰性分页、正反向 Region 边界和真实 TiKV MVCC；`pkg/store/driver/coprocessor_adapter_test.rs` 验证 cop 字段、TiFlash batch、限流、runaway/resource-control、stream/close 与运行时证据。真实 TiKV 用例标记 `#[ignore]` 且需要 `REAL_TIKV_PD`。
- Go 对照：`pkg/store/driver/tikv_driver.go`，以及 `pkg/store/driver/txn/txn_driver.go`、`snapshot.go`、`scanner.go`、`unionstore_driver.go`。它们提供 storage、事务、快照与扫描的语义基线；Rust 特有的同步 runtime 和 client-rust 限制以当前 Rust 源码为准。
- 本任务是纯文档分析，按计划不运行 Cargo；验收只执行固定 11 章节的结构命令，并人工复核上述符号、边界、调用链和扩展风险均有本地代码或独立测试依据。
