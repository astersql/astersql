# `pkg/store/mockstore/unistore.rs`

## 文件定位

本文件是 `astersql-store-mockstore` crate 中的嵌入式 UniStore 构造适配层。crate 入口 [`lib.rs`](lib.rs) 以 `pub mod unistore` 声明它；上游 [`mockstore.rs`](mockstore.rs) 的 `NewMockStore` 在 `StoreType::EmbedUnistore` 分支调用 `new_unistore`，而 [`tikv.rs`](tikv.rs) 的 MockTiKV 分支也复用本文件的 `build_embedded`。因此它不是 KV/MVCC 实现本身，而是把通用 `MockOptions` 转换为嵌入式服务参数，再组装统一的 `MockStorage` 门面。

[`Cargo.toml`](Cargo.toml) 确认该 crate 直接依赖 `astersql-store-mockstore-unistore`；后者由 `lib.rs` 重导出为 `crate::embedded_unistore`，实际启动入口位于 [`unistore/mock.rs`](unistore/mock.rs)。目标文件没有类型、常量、trait、条件编译项或内嵌测试，只有三个函数级入口。相关测试均位于独立文件，符合生产逻辑与测试分离的仓库约定。

## 核心职责

1. `new_unistore` 固定选择 `StoreType::EmbedUnistore`，为 `NewMockStore` 提供 UniStore 后端入口。
2. `build_embedded` 将测试侧 keyspace 元数据转换为嵌入式 PD 类型，并把路径、PD 地址、当前 keyspace 与 keyspace 清单传给 `embedded_unistore::New`。
3. 在底层集群创建成功后，按固定顺序执行 cluster inspector、RPC client hijacker 和 PD client hijacker。
4. 将创建结果与门面侧的当前 keyspace、latch 容量、后端标识合并为 `MockStorage`，同时把底层启动错误转换成本 crate 的 `StoreError`。
5. `newUnistore` 保留 Go 风格名字，作为 `new_unistore` 的无额外逻辑别名。

本文件不解析 URI、不应用函数式选项、不执行 DDL checker 注入，也不实现请求路由、事务、本地 latch 或 Region 管理；这些职责分别位于 `mockstore.rs`、嵌入式 UniStore 子 crate 及其更下游模块。

## 主要符号

- `pub fn new_unistore(options: &MockOptions) -> Result<MockStorage>`：公开的惯用 Rust 入口，只调用 `build_embedded(options, StoreType::EmbedUnistore)`。其后端标识是该函数与 MockTiKV 共用构造逻辑后的主要可观察差异。
- `pub(crate) fn build_embedded(options: &MockOptions, backend: StoreType) -> Result<MockStorage>`：crate 内共享构造器。它消费 `MockOptions` 中的 `cluster_keyspaces`、`path`、`pd_addresses`、`current_keyspace_id`、三个回调/劫持器和 `txn_local_latches`，返回完整 `MockStorage`。
- `pub fn newUnistore(options: &MockOptions) -> Result<MockStorage>`：Go 命名兼容入口，直接转发给 `new_unistore`；没有独立分支、状态或错误处理。
- `MockKeyspaceMeta::to_embedded`：虽定义于 `mockstore.rs`，却是本文件构造流程中的关键转换。它只复制 `id`、`name` 和 `state`，不会把门面元数据的 `config` map 传给嵌入式 PD。
- `embedded_unistore::New`：实际创建 `Arc<RPCClient>`、`Arc<PdClient>` 与 `Arc<Cluster>` 的下游入口。该函数根据路径选择持久或易失配置，启动进程内 server，并初始化 PD 门面。

## 执行流程

完整构造链从 `mockstore.rs::NewMockStore` 开始：它先按顺序应用所有 `MockTiKVStoreOption`，处理 NextGen 默认 keyspace，再按 `StoreType` 分派。EmbedUnistore 分支进入 `new_unistore`；MockTiKV 分支经 `tikv.rs::new_mock_tikv_store` 直接进入同一个 `build_embedded`。

`build_embedded` 的步骤与顺序如下：

1. 遍历 `options.cluster_keyspaces`，对每项调用 `MockKeyspaceMeta::to_embedded`，收集嵌入式 PD 接受的 keyspace 向量。此处需要分配新向量并克隆每项的名称。
2. 调用 `embedded_unistore::New(&options.path, options.pd_addresses.clone(), options.current_keyspace_id, keyspaces)`。下游先确定数据目录与持久性，建立 server、Region manager、mock PD、`Cluster`、`RPCClient` 和 `PdClient`，成功后返回三者的 `Arc`。
3. 若启动失败，立即将下游 `NewError` 格式化为字符串并返回 `StoreError`；后续 inspector、hijacker 和门面组装都不会执行。
4. 对刚创建的 `cluster` 同步调用 `cluster_inspector`。例如 `mockstore.rs::WithMockTiFlash` 利用该阶段在首个 Region 上增加带 `engine=tiflash` label 的 store/peer。
5. 若配置了 `client_hijacker`，以当前 `Arc<RPCClient>` 为输入取得替代客户端；随后对 `pd_client_hijacker` 做同样处理。两个劫持器都在集群检查之后执行，RPC 劫持先于 PD 劫持。
6. 组装 `MockStorage`：保存调用方指定的 `backend`、两个可能被替换的客户端、原集群、由 `current_keyspace_meta()` 查得的门面元数据、latch 容量，并把 `ddl_checked` 初始化为 `false`。
7. 返回后，`NewMockStore` 才可能执行 DDL checker injector；因此 DDL 包装不属于本文件的构造阶段。

## 数据与状态

本文件没有全局或长期可变状态。`MockOptions` 以共享借用进入，构造过程中不修改配置；路径和当前 ID 按引用或复制传递，PD 地址与 keyspace 数据被克隆/转换。三个回调都存于 `MockOptions` 中并同步执行。

底层资源和最终门面均以 `Arc` 共享。`MockStorage` 保存 RPC 客户端、PD 客户端和集群的独立强引用；hijacker 可以用包装后的 `Arc` 替换前两者，但不能替换本文件保存的 `cluster`。`current_keyspace` 是 `MockKeyspaceMeta` 的克隆，包含本地 `config`；传给嵌入式 PD 的 `KeyspaceMeta` 不含该配置，因此“门面元数据”和“PD 元数据”是相关但不完全相同的两份状态。

`current_keyspace_meta()` 规定了关键不变量：`current_keyspace_id == NULL_KEYSPACE_ID` 时返回 `None`；否则 ID 必须存在于 `cluster_keyspaces`，不匹配会 panic。`txn_local_latches` 在此仅作为容量数字复制到门面，`ddl_checked` 固定从 `false` 开始；本文件没有创建 latch 数据结构或设置 DDL 检查状态。

## 依赖与调用关系

RustCodeGraph 对精确符号给出的调用边为：

- `mockstore.rs::NewMockStore -> unistore.rs::new_unistore -> unistore.rs::build_embedded`。
- `tikv.rs::new_mock_tikv_store -> unistore.rs::build_embedded`，说明 Rust 当前两种 `StoreType` 共享同一套进程内启动逻辑。
- `unistore.rs::newUnistore -> unistore.rs::new_unistore`，该边只提供命名兼容。
- `build_embedded -> MockKeyspaceMeta::to_embedded`，并在源码中直接调用 `embedded_unistore::New`、三个配置回调和 `MockOptions::current_keyspace_meta`。

crate 边界由 [`Cargo.toml`](Cargo.toml) 和 [`lib.rs`](lib.rs) 共同确定：`astersql-store-mockstore-unistore` 是普通生产依赖，`lib.rs` 将其别名重导出为 `embedded_unistore`。下游 [`unistore/mock.rs`](unistore/mock.rs) 的 `New` 再依赖配置、server、RPC、PD 与 cluster 模块；本文件只面向其汇总 API，不直接操作引擎或 Region manager。

没有发现独立 Rust 测试直接点名 `new_unistore`、`build_embedded` 或 `newUnistore`。间接证据来自 [`mockstore_test.rs`](mockstore_test.rs) 对 keyspace 缺省、显式 GC 配置和 ID 绑定的验证，以及 [`unistore/mock_test.rs`](unistore/mock_test.rs) 对 `New` 的临时路径、易失模式和关闭清理的验证。

## 错误处理与边界

`embedded_unistore::New` 的 `NewError` 可表示 IO、server 启动或 PD 构造失败；本文件用 `error.to_string()` 将它压缩为 `StoreError(String)`。这保留用户可读消息，但丢失具体错误变体与 source 链，调用者不能再按错误类型匹配。启动失败会在所有用户回调之前短路。

`cluster_inspector`、`client_hijacker` 和 `pd_client_hijacker` 的签名不返回 `Result`，所以它们无法通过本函数的错误通道报告失败；回调若 panic，panic 会直接越过构造器。`current_keyspace_meta()` 同样可能因非空 ID 未出现在列表中而 panic。新增输入验证时，应明确这些配置错误是继续 panic 还是转为 `StoreError`，避免无意改变测试兼容行为。

本文件不验证路径、PD 地址、keyspace 状态或后端枚举组合。路径与 PD 构造错误交给 `embedded_unistore::New`；keyspace 列表允许为空；`backend` 只是写入门面的标签，`build_embedded` 不根据它选择不同引擎。`tikv_options`、`ddl_checker_hijack` 和 `keyspace_specified` 不在本文件读取，不能据此声称它们在此处对底层生效。

## 并发与资源生命周期

构造过程本身是同步的：底层服务创建完成后，inspector 和两个 hijacker 在调用线程内依序执行。本文件不 spawn 线程、不创建通道、不持锁；回调类型在 `mockstore.rs` 中要求 `Send + Sync`，但这里不会并行调用它们。扩展时必须保留“集群就绪 → inspector → RPC hijacker → PD hijacker → 门面可见”的顺序，除非同时更新依赖这一时序的测试钩子。

资源所有权通过 `Arc` 转交给 `MockStorage`。`MockStorage::close` 只显式调用 RPC 客户端的 `close`；下游 `RPCClient::close` 幂等地停止 server，并在非持久模式删除临时目录。`RPCClient` 另有 `Drop` 清理保障，而 `PdClient::close` 与 `Cluster::close` 当前为空操作。由于 hijacker 能替换客户端，包装器应保留必要的关闭转发，否则门面的 `close` 可能无法抵达原 RPC 客户端。

若构造在 `embedded_unistore::New` 成功后、门面返回前因 inspector/hijacker panic，中间资源只能依赖已创建 `Arc` 的析构路径释放；本文件没有显式回滚。持久路径不会在正常关闭时删除，空路径或 `tidb-unistore-temp*` 路径按下游规则视为易失目录。

## 与 Go 版本的对应关系

直接对照是 [`unistore.go`](unistore.go) 的 `newUnistore`。两版都调用 UniStore `New`，传入路径、PD 地址、当前 keyspace 与 keyspace 清单；都在创建集群后执行 inspector，并最终返回 mock storage 门面。两者也都把创建错误立即向上传播。

已验证的差异如下：

- Go 在 `unistore.New` 后先用 `util.InterceptedPDClient` 包装 PD，再根据当前 ID 是否为 `constants.NullKeyspaceID`，分别调用 `tikv.NewTestTiKVStore` 或 `tikv.NewTestKeyspaceTiKVStore`；Rust 不构造 client-go KVStore，而是直接把嵌入式 RPC/PD 客户端装入自己的 `MockStorage`。
- Go 把 client hijacker、PD hijacker、latch 和 `tikvOptions` 交给 client-go 构造器；Rust 在本文件中直接依次调用两个 hijacker，只把 latch 容量记录到门面，且不读取 `tikv_options`。
- Go 的 keyspace 分支把 `*opts.currentKeyspaceMeta()` 传给 keyspace store；Rust 始终调用 `current_keyspace_meta()` 填充 `Option<MockKeyspaceMeta>`。非空 ID 与列表不一致时，Rust 会在门面组装阶段 panic。
- Go 使用 `errors.Trace` 包装 UniStore 启动错误，保留其错误链语义；Rust 转为字符串型 `StoreError`。
- Go 的 `newUnistore` 是单一入口；Rust 抽出 `build_embedded`，让 MockTiKV 与 EmbedUnistore 复用实现，并用 `StoreType` 保留门面身份。因此不能将 Go 的两套具体后端差异投射为 Rust 当前事实。
- Rust 的 `newUnistore` 只是兼容别名；实际 Rust 工厂调用惯用命名的 `new_unistore`。

## 扩展指南

- 新增真正影响后端启动的 `MockOptions` 字段时，应在 `build_embedded` 中接入 `embedded_unistore::New` 或新的下游构造参数，并在独立测试文件中证明行为变化；只在 `MockOptions` 保存字段不等于功能已生效。
- 修改 keyspace 传递时，应同步检查 `MockKeyspaceMeta::to_embedded`、`current_keyspace_meta`、下游 `unistore/pd.rs` 与 [`mockstore_test.rs`](mockstore_test.rs)。尤其要决定 `config` 是否应进入 PD 元数据，并覆盖 Null ID、多个 keyspace、缺失当前 ID 和显式 GC 配置。
- 新增或调整 inspector/hijacker 时，应在新的 `unistore_test.rs` 或已有独立 `mockstore_test.rs` 中用记录调用次序的替身验证：启动失败不调用回调、inspector 先于两个 hijacker、替换客户端被写入结果、关闭能传递到底层。测试不要放进生产 `unistore.rs`。
- 若要让 `StoreType::MockTiKv` 使用独立实现，应改变 `tikv.rs::new_mock_tikv_store` 的下游，而不是在 `build_embedded` 内仅凭标签分支；同时更新门面测试与 Go 差异说明。
- 改进错误类型时，要评估依赖 `StoreError` 文本的调用者，并为 IO、server、PD、配置不变量及回调失败分别提供回归覆盖。
- 正确性风险集中在 keyspace ID/列表一致性、回调顺序和 hijacker 的关闭转发；兼容风险集中在 Go client-go 选项尚未等价接线；性能成本主要是每次构造启动完整进程内服务、克隆地址/keyspace 向量及额外客户端包装。

## 验证依据

- RustCodeGraph `status`：索引可用，包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/store/mockstore` 确认目标文件已索引。
- RustCodeGraph `node --file pkg/store/mockstore/unistore.rs --offset 1 --limit 160`：读取目标全部 66 行，并报告它被 `mockstore.rs` 与 `tikv.rs` 使用。
- RustCodeGraph `node new_unistore` 与 `node build_embedded`：确认 `NewMockStore -> new_unistore -> build_embedded`、`newUnistore -> new_unistore` 和 `new_mock_tikv_store -> build_embedded` 的调用边；目标文件的三个函数及完整实现均已核对。
- crate 与模块边界：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`mockstore.rs`](mockstore.rs) 和 [`tikv.rs`](tikv.rs)，用于核对依赖、再导出、选项定义、返回门面和两后端分派。
- 下游实现：[`unistore/Cargo.toml`](unistore/Cargo.toml)、[`unistore/mock.rs`](unistore/mock.rs) 与 [`unistore/rpc.rs`](unistore/rpc.rs)，用于核对 `New` 的启动步骤、路径持久性以及 RPC 关闭/析构语义。
- Go 对照：[`unistore.go`](unistore.go)，用于核对 UniStore 创建、inspector、NullKeyspace 分支、client-go 构造参数、错误传播和最终 mockstorage 包装。
- 独立测试：[`mockstore_test.rs`](mockstore_test.rs) 验证本构造器消费的 keyspace 配置；[`unistore/mock_test.rs`](unistore/mock_test.rs) 验证下游临时路径的易失与关闭清理。目录检索未发现 Rust 或 Go 测试直接点名本文件的三个函数，因此回调顺序与门面组装目前主要由生产源码和代码图作证。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前执行任务指定的固定十一章节结构检查，并人工复核文档只陈述上述源码、调用图、Cargo、Go 和测试能够支持的事实。
