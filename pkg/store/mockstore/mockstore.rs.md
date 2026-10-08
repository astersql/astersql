# `pkg/store/mockstore/mockstore.rs`

## 文件定位

本文件是 `astersql-store-mockstore` crate 的公开工厂与配置门面。crate 入口 [`lib.rs`](lib.rs) 将它声明为 `mockstore` 模块并用 `pub use mockstore::*` 公开再导出；因此测试基础设施通常从 crate 根直接使用 `NewMockStore`、`With*` 选项、两个 driver 和 bootstrap 辅助函数，而不需要感知内部模块路径。

它位于测试侧 SQL/元数据逻辑与进程内 KV 协议服务之间：上游 [`teststore/store.rs`](teststore/store.rs) 将 `MockStorage` 包装为 `Arc<MockStore>`，[`../../infoschema/internal/testkit.rs`](../../infoschema/internal/testkit.rs) 也直接用 `NewMockStore` 创建测试存储；下游则由 [`tikv.rs`](tikv.rs) 和 [`unistore.rs`](unistore.rs) 建立实际后端。这里负责收集选项、选择后端、绑定 keyspace、注入测试钩子和提供集群拓扑辅助，不实现 MVCC、RPC 或 Region 管理本身。

[`Cargo.toml`](Cargo.toml) 确认 crate 名为 `astersql-store-mockstore`，Go 对照包为 `pkg/store/mockstore`。生产依赖只有 `astersql-config` 与嵌入式 UniStore crate `astersql-store-mockstore-unistore`；表编解码和测试初始化仅是开发依赖。本文件没有条件编译项，独立测试由 `lib.rs` 通过 `#[path = "mockstore_test.rs"]` 和 `#[path = "tikv_test.rs"]` 接入。

## 核心职责

1. 用 `MockOptions` 和 `MockTiKVStoreOption` 实现 Go 风格函数式选项，配置路径、PD 地址、客户端劫持、集群检查器、latch、keyspace、DDL checker 和后端类型。
2. 由 `NewMockStore` 应用选项，在 NextGen 默认 keyspace 规则之后分派至 MockTiKV 或 EmbedUnistore 构造路径，并按需执行 DDL checker 注入。
3. 用 `MockStorage` 统一暴露 RPC 客户端、PD 客户端、模拟集群、当前 keyspace 和 latch 状态，并负责关闭 RPC 客户端。
4. 由 `MockTiKVDriver::open` 和 `EmbedUnistoreDriver::open` 把 URI 与全局 latch 配置转换为工厂选项。
5. 提供 TiFlash peer 注入、bootstrap 镜像探测，以及单 store、多 store、多 Region 的确定性测试拓扑构造。

## 主要符号

- 常量：`NULL_KEYSPACE_ID` 是未绑定 keyspace 的 `u32::MAX` 哨兵；`SYSTEM_KEYSPACE_NAME` 与 `MAX_KEYSPACE_ID - 1` 共同定义 NextGen 默认 SYSTEM keyspace；`KEYSPACE_GC_MANAGEMENT_TYPE`/`KEYSPACE_GC_KEYSPACE_LEVEL` 定义默认 GC 配置；`IMAGE_FILE_PATH` 指向可选 bootstrap 镜像目录。
- `StoreType::{MockTiKv, EmbedUnistore}` 与 `DEFAULT_STORE_TYPE`：选择构造分支，默认是 EmbedUnistore。Rust 的 MockTiKV 分支由 `tikv.rs::new_mock_tikv_store` 复用统一的进程内协议服务，但在 `MockStorage.backend` 中保留独立身份。
- `MockKeyspaceMeta`：保存测试所需的 `id`、`name`、`state` 和配置表；`to_embedded` 只把前三项转换成嵌入式 PD 的 `KeyspaceMeta`，GC 配置保留在门面元数据中。
- `StoreError` 与 `Result<T>`：本 crate 工厂层的字符串错误包装；实现 `Display` 和标准 `Error`。
- `MockStorage`：最终门面，持有 `Arc<RPCClient>`、`Arc<PdClient>`、`Arc<Cluster>`、当前 keyspace、latch 容量和 `ddl_checked` 标志。`close` 向 RPC 客户端转发清理，`is_latch_enabled` 以容量是否大于零判断开关。
- `MockOptions`：工厂的可变配置快照。`Default` 安装空操作 inspector、默认后端、空路径/地址/劫持器、关闭 latch/DDL checker，并用 `NULL_KEYSPACE_ID` 表示无当前 keyspace。`current_keyspace_meta` 按 ID 查找并克隆元数据；非哨兵 ID 找不到时 panic，暴露配置不变量破坏。
- `ClientHijacker`、`PdClientHijacker`、`ClusterInspector`：均为 `Arc<dyn Fn(...) + Send + Sync>`，允许测试包装客户端或在集群创建后改写拓扑。`MockTiKVStoreOption` 是一次消费的 `Box<dyn Fn(&mut MockOptions) + Send + Sync>`。
- `WithMultipleOptions` 依次执行组合选项；`WithPDAddr`、`WithTiKVOptions`、`WithClientHijacker`、`WithPDClientHijacker`、`WithClusterInspector`、`WithStoreType`、`WithPath`、`WithTxnLocalLatches` 和 `WithDDLChecker` 分别修改对应字段。
- `WithMockTiFlash`：组合 inspector 与 `StoreType::EmbedUnistore`，向已 bootstrap 的首个 Region 加入指定数量、带 `engine=tiflash` label 的 store/peer。
- `enable_keyspace_level_gc_if_not_set`、`WithCurrentKeyspaceMeta`、`WithKeyspacesAndCurrentKeyspaceID`：在调用方未显式设置时补 `keyspace_level` GC；前者绑定单一 keyspace或清空绑定，后者注册多个 keyspace 并指定当前 ID。
- `NEXT_GEN`/`set_next_gen`：进程级原子开关。`DDL_INJECTOR`/`set_ddl_checker_injector`：由 `OnceLock<RwLock<Option<DdlInjector>>>` 保存的进程级可替换注入器。
- `NewMockStore`：核心入口；应用配置、补 NextGen SYSTEM keyspace、选择后端，并在 `WithDDLChecker` 启用时通过全局 injector 包装结果。
- `MockTiKVDriver`、`EmbedUnistoreDriver` 与私有 `parse_uri`：解析 `scheme:path`/`scheme://path`，校验 scheme，附加路径、后端和全局 latch 选项后调用 `NewMockStore`。
- `ImageAvailable`：仅检查镜像根目录和 `kv` 子目录是否都可读。
- `BootstrapWithSingleStore`、`BootstrapWithMultiStores`、`BootstrapWithMultiRegions`：基于已经 bootstrap 的嵌入式 `Cluster` 查询或扩展 store、peer 与 Region，并返回测试继续操纵拓扑所需的 ID。

## 执行流程

`NewMockStore` 的主流程如下：

1. 以 `MockOptions::default` 建立配置，按传入顺序消费所有 `MockTiKVStoreOption`；后面的选项可以覆盖前面的同一字段。
2. 若 `NEXT_GEN` 以 Acquire 读到开启且调用方没有显式指定 keyspace，则调用 `WithCurrentKeyspaceMeta` 创建 ID 为 `MAX_KEYSPACE_ID - 1`、名称为 `SYSTEM` 的元数据；该选项同时补上 keyspace 级 GC。
3. 按 `store_type` 分派：`MockTiKv` 调 `tikv.rs::new_mock_tikv_store`，`EmbedUnistore` 调 `unistore.rs::new_unistore`。两条路径最终进入 `unistore.rs::build_embedded`：转换 keyspace 元数据，调用嵌入式 `New` 创建 RPC/PD 客户端和集群，随后执行 inspector，再依次包装 RPC 与 PD 客户端，最后组装 `MockStorage`。
4. 若启用了 `ddl_checker_hijack`，从 `DDL_INJECTOR` 的读锁取得注入器并转换 store；未安装时返回 `StoreError`。未启用则直接返回后端结果。

两个 driver 的流程相同：`parse_uri` 先按第一个冒号拆分 URI，并去掉 remainder 开头可选的 `//`；driver 不区分大小写校验 scheme，构造 `WithPath` 和对应的 `WithStoreType`。若 `astersql_config::get_global_config().txn_local_latches.enabled`，再追加容量选项，最后进入 `NewMockStore`。[`tikv_test.rs`](tikv_test.rs) 已验证 MockTiKV driver 会反映 latch 开关、拒绝错误 scheme，并在使用后显式 `close`。

拓扑辅助函数假定嵌入式 server 已创建至少一个 Region。`BootstrapWithSingleStore` 扫描首个 Region，优先取 leader，否则取首 peer。`BootstrapWithMultiStores` 保留首 store/peer，再为 `1..count` 分配 ID、向同一 Region 加 store 和 peer。`BootstrapWithMultiRegions` 先一次性分配全部新 Region/peer ID，再按 split key 顺序调用 `Cluster::split`；每次以当前位置的 peer ID 作为源 peer 与新 Region 的初始 peer，保持 Go 的分配顺序。

## 数据与状态

`MockOptions` 是构建期状态，选项闭包只在工厂调用中顺序修改它。`cluster_keyspaces` 保存完整测试元数据，而传给嵌入式 PD 的 `KeyspaceMeta` 不含本地 `config`；最终 `MockStorage.current_keyspace` 再从配置中克隆当前项，因此调用方后续修改原输入不会改变已建 store 的门面状态。

keyspace 有三种重要状态：默认是 `current_keyspace_id == NULL_KEYSPACE_ID` 且列表为空；显式 `WithCurrentKeyspaceMeta(None)` 会设置 `keyspace_specified` 并保持未绑定，从而阻止 NextGen 自动 SYSTEM 回填；多 keyspace 选项允许当前 ID 指向列表中的一项，但 `current_keyspace_meta` 会在不匹配时 panic。GC 配置采用“只补缺省、不覆盖显式值”的规则，独立测试证明显式 `unified` 被保留，而缺省项变成 `keyspace_level`。

两个全局状态作用于整个进程。`NEXT_GEN: AtomicBool` 使用 Release 写/Acquire 读；`DDL_INJECTOR` 的 `RwLock<Option<_>>` 允许注册、替换和清除注入器。它们不是单个 store 私有配置，测试若并行修改必须自行隔离并在结束时恢复，避免影响其他构造调用。

`MockStorage` 用 `Arc` 共享客户端和集群；它本身没有后台线程句柄或 Drop 实现。`txn_local_latches` 只保留容量并提供启用判断，实际冲突检测能力属于更下游的存储实现。`ddl_checked` 在本文件构造时固定为 `false`；若注入器需要改变它，必须在包装闭包中完成。

## 依赖与调用关系

RustCodeGraph 的精确文件限定查询确认核心下游边为：

- `MockTiKVDriver::open -> parse_uri -> WithPath/WithStoreType[/WithTxnLocalLatches] -> NewMockStore`。
- `EmbedUnistoreDriver::open -> parse_uri -> WithPath/WithStoreType[/WithTxnLocalLatches] -> NewMockStore`。
- `NewMockStore -> WithCurrentKeyspaceMeta`（仅 NextGen 缺省）以及 `tikv.rs::new_mock_tikv_store | unistore.rs::new_unistore`。
- `tikv.rs::new_mock_tikv_store -> unistore.rs::build_embedded`；`unistore.rs::new_unistore -> build_embedded`。后者调用 `embedded_unistore::New`，再执行 cluster inspector 和两个 hijacker。
- `WithMockTiFlash -> WithMultipleOptions`，其 inspector 通过 `Cluster::region_manager` 扫描 Region、分配 ID、增加带 label 的 store 和 peer。
- `BootstrapWithMultiStores -> BootstrapWithSingleStore`；`BootstrapWithMultiRegions -> BootstrapWithSingleStore -> Cluster::split`。

精确上游源码证据包括：[`teststore/store.rs`](teststore/store.rs) 的包装工厂先同步 kernel NextGen 状态再调用本文件 `NewMockStore`；[`../../infoschema/internal/testkit.rs`](../../infoschema/internal/testkit.rs) 的 `TestStore::new` 直接持有返回的 `MockStorage`；[`../../executor/test/tiflashtest/tiflash_test.rs`](../../executor/test/tiflashtest/tiflash_test.rs) 用 `NewMockStore(vec![WithMockTiFlash(2)])` 构造分析副本拓扑。`lib.rs` 的公开再导出使这些调用都从 crate 根进入。

crate 级直接依赖中，`astersql-config` 提供 driver 的全局 latch 配置；`astersql-store-mockstore-unistore` 经 `lib.rs` 重导出为 `embedded_unistore`，提供 `Cluster`、`PdClient`、`RPCClient`、keyspace 类型与真实进程内实现。

## 错误处理与边界

- `StoreError` 把下游错误压缩成字符串，保留可读消息但不保留具体错误类型或 source 链。后端创建、RPC `close`、加 peer 和 Region split 都通过 `to_string` 映射。
- `parse_uri` 只要求存在冒号；`":"` 因而能解析为空 scheme 和空 path，但随后会被具体 driver 的 scheme 检查拒绝。它不是完整 URL parser，不做 percent decode、authority 或查询参数解析。
- driver 的 scheme 比较不区分大小写；其余 remainder 原样作为路径，最多移除一个开头 `//`。
- `NewMockStore` 的 `StoreType` 是封闭枚举，不存在 Go `switch default` 的未知值 panic。DDL checker 被请求但 injector 未安装时，Rust 明确返回错误；Go 对照会直接调用全局函数变量，未安装更可能触发 panic。
- `MockOptions::current_keyspace_meta` 在非空当前 ID 不属于 `cluster_keyspaces` 时 panic。这是调用方配置错误，不是可恢复的运行时错误；`WithKeyspacesAndCurrentKeyspaceID` 本身不会提前验证这一不变量。
- `WithMockTiFlash` 要求已有 Region，缺失时在 `expect` panic；`add_peer` 失败也 panic。它是测试拓扑便利选项，不提供错误返回通道。
- `BootstrapWithSingleStore` 对无 Region、无 leader/peer 返回 `StoreError`。`BootstrapWithMultiStores` 对 `count == 0` 仍先获取首 store，最后索引 `peers[0]`；因此零并不是安全的“创建零个 store”输入。`count == 1` 返回已有拓扑。多 Region 的空 split key 列表则安全返回原单 Region。
- `ImageAvailable` 只证明两个目录可读取，不验证镜像内容完整性；当前 Go `NewMockStore` 中利用镜像加速的代码也是注释状态，Rust 工厂不会自动复制或加载该目录。

## 并发与资源生命周期

所有回调类型都要求 `Send + Sync` 并由 `Arc` 持有，所以配置可携带线程安全的共享闭包；工厂本身同步执行这些闭包。`build_embedded` 的顺序是“创建集群 → inspector 改拓扑 → 包装 RPC 客户端 → 包装 PD 客户端”，扩展时不应随意改变此顺序，因为 hijacker 可能假设拓扑已经就绪。

`NEXT_GEN` 的 Acquire/Release 保证开关发布与读取的基本跨线程可见性。DDL injector 注册持写锁，构造时持读锁；当前实现借用锁内的 injector 并在读锁仍存活时同步调用它，因此其他线程不能在注入执行期间替换或清除 injector。若注入器递归调用 `set_ddl_checker_injector`，会产生锁重入风险，扩展时应避免这种模式。

`MockStorage::close` 是显式资源边界，只关闭 RPC 客户端；`Arc<PdClient>` 和 `Arc<Cluster>` 依赖引用计数释放。本文件不 spawn 任务、不创建通道，也不等待后台线程。测试应在完成后调用 `close`，尤其是 driver 和嵌入式集群测试；[`mockstore_test.rs`](mockstore_test.rs) 的 Region 测试直接保留底层 client 并在末尾关闭它。

进程级开关会跨测试存活，且 `astersql_config` 的全局 latch 配置同样会影响后续 driver 调用。新增并发测试应串行化这些全局修改或使用可恢复 guard，不能假设每个测试拥有独立全局状态。

## 与 Go 版本的对应关系

Go 对照文件是 [`mockstore.go`](mockstore.go)。两版保留相同的两个后端、默认 EmbedUnistore、函数式选项族、NextGen SYSTEM keyspace、keyspace 级 GC 缺省、两个 URI driver、TiFlash 拓扑注入、镜像探测和三类 bootstrap API。Rust 名称刻意沿用 Go 的首字母大写形式，并在 crate 根放宽 `non_snake_case` lint。

关键实现差异如下：

- Go 的默认 `clusterInspector` 调 `BootstrapWithSingleStore`；Rust 的嵌入式 `server::new_mock` 已创建单 store/单 Region，因此默认 inspector 是空操作。Rust bootstrap 辅助函数主要查询或扩展该现有拓扑。
- Go 的 MockTiKV 使用 client-go `MockCluster`/goleveldb，UniStore 使用另一具体类型；Rust 工作区当前只有一套规范的进程内 TiKV 协议 server，两个 `StoreType` 都进入 `build_embedded`，差别体现在后端标识和传入模式，而不是两套独立引擎实现。
- Go `WithTiKVOptions` 接受真实 `tikv.Option`，Rust 当前只保存 `Vec<String>`；该字段在已读的 `build_embedded` 路径中没有被消费，不能把它描述为已对底层生效。
- Go keyspace protobuf元数据自身携带 `Config`；Rust `MockKeyspaceMeta` 额外保存配置，但 `to_embedded` 不传递该 map。它仍用于门面默认值与测试断言，而不是嵌入式 PD 元数据的完整等价复制。
- Go `NewMockStore` 调 `testenv.SetGOMAXPROCSForTest`，Rust 没有对应进程调度器设置。Go 的 DDL injector 是裸全局函数变量，Rust 用锁保护的可选 `Arc`，缺失时返回 `StoreError`。
- Go driver 使用 `net/url.Parse` 和 `u.Path`；Rust 使用最小 `parse_uri`。简单 `mocktikv://`/`unistore://` 语义对齐，但复杂 URL 的规范化能力不等价。
- Go bootstrap 根据运行时具体 cluster 类型分派到 client-go 或 UniStore；Rust API 只接受嵌入式 `Cluster`。Rust `mockstore_test.rs` 对 peer 分配顺序提供了明确回归证据。

## 扩展指南

- 新增构建选项时，先把字段放入 `MockOptions` 并给出安全默认值，再增加返回 `MockTiKVStoreOption` 的函数；若选项应影响真实后端，必须同步接到 `unistore.rs::build_embedded` 或对应构造函数，并在独立 `mockstore_test.rs` 中证明生效，不能只存字段。
- 新增后端时，需要扩展 `StoreType`、`NewMockStore` 分派、driver/注册入口以及 `MockStorage.backend` 断言。应明确它是否拥有独立引擎；不要复制当前 MockTiKV 复用嵌入式实现的描述作为未经验证的事实。
- 修改 keyspace 规则时，应同时检查 `enable_keyspace_level_gc_if_not_set`、两个 keyspace 选项、`current_keyspace_meta`、NextGen 默认分支和 `teststore/store.rs` 的 `set_next_gen` 调用；测试需覆盖显式 None、显式 GC 模式、多 keyspace ID 不匹配及 SYSTEM 缺省。
- 修改 DDL checker 注入时，应保留“bootstrap 前配置、构造后包装”的调用契约，并为未安装、安装、清除和并发替换增加独立测试。注意不要让注入器在读锁内回写同一全局锁。
- 修改 driver URI 或 latch 逻辑时，同步 [`tikv_test.rs`](tikv_test.rs)，并为 EmbedUnistore driver 添加对称覆盖；兼容风险集中在 path 解释、scheme 大小写和全局配置泄漏。
- 修改拓扑辅助函数时，同步 [`mockstore_test.rs`](mockstore_test.rs) 和需要真实拓扑的 TiFlash/Region 测试。需要先定义 `count == 0`、空 split keys、无 leader、重复/无序 split keys 的行为，避免以 panic 偶然充当契约。
- 性能风险主要来自每次工厂调用启动完整嵌入式服务、keyspace/地址向量克隆和劫持器额外包装；正确性风险主要来自进程级全局开关、选项应用顺序及 current ID 与 keyspace 列表不一致。测试逻辑继续放在独立 `*_test.rs`，不要嵌入生产文件。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/store/mockstore/mockstore.rs` 确认目标已索引；`node --file ... --offset 1 --limit 500` 读取目标全部 498 行并报告被 17 个文件使用。
- 精确符号查询：带文件限定的 `node` 核对了 `NewMockStore`、`WithMockTiFlash`、`BootstrapWithMultiStores`、`BootstrapWithMultiRegions`、`MockTiKVDriver` 和 `EmbedUnistoreDriver`。图中明确给出 `NewMockStore -> WithCurrentKeyspaceMeta/new_mock_tikv_store/new_unistore`、两个 driver `open -> NewMockStore`，以及 `mockstore_test.rs::multi_region_bootstrap_uses_go_peer_allocation_sequence -> BootstrapWithMultiRegions`。
- 目标与 crate 边界：[`mockstore.rs`](mockstore.rs)、[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)，核对公开符号、依赖、Go 包元数据、再导出和独立测试接线。
- 下游实现：[`tikv.rs`](tikv.rs) 与 [`unistore.rs`](unistore.rs)，核对两个后端最终的构建路径，以及 inspector、hijacker、keyspace、latch 和 `MockStorage` 的组装顺序。
- 上游调用：[`teststore/store.rs`](teststore/store.rs)、[`../../infoschema/internal/testkit.rs`](../../infoschema/internal/testkit.rs)、[`../../executor/test/tiflashtest/tiflash_test.rs`](../../executor/test/tiflashtest/tiflash_test.rs)，分别证明 NextGen 包装、TestStore 持有和 TiFlash 选项的实际入口。
- Rust 测试：[`mockstore_test.rs`](mockstore_test.rs) 验证 keyspace 哨兵、显式 GC 保留、缺省 GC 和多 Region peer 分配序列；[`tikv_test.rs`](tikv_test.rs) 验证全局 latch、URI 错误和资源关闭。
- Go 对照：[`mockstore.go`](mockstore.go) 全部 372 行，核对 driver、选项、keyspace、后端分派、DDL 注入、镜像探测和 bootstrap API。目录内 Go 测试仅在 `tikv_test.go` 覆盖 latch 配置，未发现同路径 Go 测试直接覆盖 keyspace 或 bootstrap 辅助函数；相关 Rust 行为因此以独立 Rust 测试和 Go 生产实现共同为证。
- 本任务只生成文档，按计划不运行 Cargo。交付前使用任务指定命令检查文件存在且固定二级标题恰好 11 个，并人工复核每项关键结论可回指上述符号或文件。
