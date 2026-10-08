# `pkg/store/mockstore/unistore/mock.rs`

## 文件定位

本文件是 `astersql-store-mockstore-unistore` crate 的一站式构造入口。crate 在
`pkg/store/mockstore/unistore/Cargo.toml` 中声明，入口 `lib.rs` 以 `pub mod mock`
加载本文件并通过 `pub use mock::*` 再导出其公开项。它位于上层 mockstore 适配层和
底层嵌入式 UniStore 组件之间：

1. `pkg/store/mockstore/unistore.rs::build_embedded` 把 `MockOptions` 转换成这里的
   `New` 参数；
2. `New` 创建并配置本地引擎、单节点模拟服务、Region 集群控制器、进程内 RPC
   客户端和 PD 门面；
3. 上层再应用 cluster/client/PD hijacker，最终构造成 `MockStorage`。

因此，这个文件负责“同时把一组彼此一致的测试组件启动起来”，而不负责处理具体
KV、Region、PD 或 RPC 请求。实际行为分别位于 `server/server.rs`、`cluster.rs`、
`pd.rs` 和 `rpc.rs`。

## 核心职责

- 根据调用者提供的路径决定数据是持久还是易失。空路径由
  `create_temp_directory` 创建系统临时目录；路径字符串以系统临时目录下的
  `tidb-unistore-temp` 开头时，也按易失实例处理。
- 从 `config::DefaultConf` 克隆一份实例配置，固定 `ValueThreshold = 0`、数据库
  路径和 `Raft = false`，并为易失实例进一步收紧内存/compaction 配置。
- 调用 `server::new_mock(&conf, 1)` 启动 cluster ID 为 1 的单节点模拟服务，取得
  同源的 `Server`、`MockRegionManager` 和 `MockPd`。
- 用同一个 Region 管理器构造 `Cluster`，再把同一个 `Server`、`Cluster`、路径和
  持久性标志交给 `RPCClient`，保证 RPC 路由看到的拓扑与服务端元数据一致。
- 把调用者给出的 PD 地址、当前 keyspace ID 和 keyspace 元数据交给
  `PdClient::new` 校验并装载，最后以 `Arc` 返回三个可共享句柄。
- 用 `NewError` 保留构建过程中 IO、Server 和 PD 三类失败来源。

本文件不实现真实 Raft，也不建立网络连接。`conf.Server.Raft = false` 明确选择
进程内 standalone/mock 路径，RPC 与 PD 对象也都是本地门面。

## 主要符号

### `pub enum NewError`

`NewError` 是构造阶段的统一错误边界，包含：

- `Io(std::io::Error)`：创建临时目录或目标目录失败；
- `Server(server::ServerError)`：数据库配置/打开、Region 引导或内部服务启动失败；
- `Pd(PdError)`：`PdClient` 初始化失败，目前主要来自初始 keyspace 元数据非法。

它实现 `Display`、`std::error::Error`，并为三种底层错误实现 `From`，使 `New`
中的 `?` 保持错误类别而不需要手工映射。`Display` 直接转发底层错误文本。

### `pub fn New(...) -> Result<(Arc<RPCClient>, Arc<PdClient>, Arc<Cluster>), NewError>`

这是本文件唯一公开业务入口。参数语义如下：

- `path: impl AsRef<Path>`：引擎根目录；空路径表示自动分配临时目录；
- `pd_addresses: Vec<String>`：模拟 PD 的成员地址输入；PD 构造时会规范化地址并
  丢弃无法规范化的项；
- `current_keyspace_id: u32`：当前租户逻辑 keyspace 标识，原样保存在 PD 门面中；
- `cluster_keyspaces: Vec<KeyspaceMeta>`：模拟集群预置的 keyspace 元数据；
- 返回值：共享同一底层服务/拓扑的 RPC 客户端、PD 客户端和 Cluster 控制器。

函数名保留 Go 的 `New` 命名，crate 根通过 `#![allow(non_snake_case)]` 接受这一
迁移接口。

### `fn create_temp_directory() -> std::io::Result<PathBuf>`

私有辅助函数以 `SystemTime::now()` 距 Unix epoch 的纳秒数生成
`tidb-unistore-temp-<nonce>` 路径并创建目录。若系统时钟早于 epoch，
`unwrap_or_default` 使用零时长而不是 panic。它不像 Go 的 `os.MkdirTemp` 那样由
系统随机化并自动重试冲突；同一纳秒路径已存在时，`create_dir_all` 会复用它。

## 执行流程

`New` 的完整顺序如下：

1. 读取 `path.as_ref()`。若路径的 OS 字符串为空，调用
   `create_temp_directory`；否则复制调用者路径。
2. 计算系统临时目录下的 `tidb-unistore-temp` 前缀。这里刻意用字符串
   `starts_with`，与 Go 的 `strings.HasPrefix` 对齐；不能改成
   `Path::starts_with`，否则 `tidb-unistore-temp-123` 不会被识别为易失路径。
3. 对最终路径执行 `fs::create_dir_all`。持久和易失模式都保证根路径先存在。
4. 克隆 `config::DefaultConf`，设置：
   - `Engine.ValueThreshold = 0`，让全部 value 进入 LSM，避免 value-log 路径差异；
   - `Engine.DBPath` 为最终路径的有损 UTF-8 字符串；
   - `Server.Raft = false`，选择进程内模拟服务。
5. 若为易失路径，再设置 `VolatileMode = true`、12 MiB memtable、不同步写、单
   compactor、关闭时不压缩 L0，以及 16 MiB value-log 文件。
6. 调用 `server::new_mock(&conf, 1)`。该函数建立 safe point、数据库和内存
   lock store，分配初始 store/region/peer ID，引导一个覆盖全键空间的 Region，
   创建 `MockPd`，启动 standalone inner server，并返回三件套。
7. 将 `MockRegionManager` 放进 `Cluster::new`；该对象还维护供事务测试使用的
   一次性 RPC 延迟表。
8. 将 server、cluster、路径和 `persistent` 放进 `RPCClient::new`。构造函数还会
   创建独立 `RawHandler`，并把关闭标记初始化为 `false`。
9. 调用 `PdClient::new`，装载并校验 keyspace，初始化全局配置、外部时间戳和
   PD 地址集合。
10. 返回 `(Arc<RPCClient>, Arc<PdClient>, Arc<Cluster>)`。上层
    `build_embedded` 随后可检查/修改 cluster，并替换 client 或 PD 门面。

这个顺序很重要：PD keyspace 校验发生在 server 已启动之后。当前实现若在最后的
`PdClient::new` 失败，不会在 `New` 内显式关闭刚创建的 server 或删除自动生成的
临时目录；这是扩展错误清理时需要特别关注的边界。

## 数据与状态

- `path: PathBuf` 是这一实例的数据根路径，同时被写入引擎配置并保存在
  `RPCClient` 中用于关闭时清理。
- `persistent: bool` 不是调用者直接传入，而是由路径文本前缀推导。只有
  `false` 时，`RPCClient::close` 才删除整个根目录。
- `conf` 是 `DefaultConf` 的实例级克隆；本文件只修改克隆，不改变全局默认配置。
- `server::new_mock` 返回的三个对象共享一套状态：`Server` 与 `Cluster` 使用同一
  `MockRegionManager`，`PdClient` 包装由该 manager 构造的 `MockPd`。
- cluster ID 固定为 `1`；底层为初始 store、region 和 peer 分配 ID，并创建一个
  覆盖空起止键范围的 Region。
- `cluster_keyspaces` 被 `MockKeyspaceManager` 分别建立按 ID 排序的 map 和名称索引；
  ID 或名称重复以及 ID 超出 `MAX_KEYSPACE_ID` 都会使构造失败。
- 三个返回对象都由 `Arc` 管理，可以跨持有者共享。`RPCClient` 内部还以原子状态
  记录是否已关闭；`Cluster` 和 `PdClient` 的可变模拟状态由 `Mutex`、`RwLock`
  或原子量保护，但这些同步策略在各自实现文件中完成。

路径通过 `to_string_lossy()` 写入 `Engine.DBPath` 并用于前缀比较。非 UTF-8 路径会
被替换字符表示，因此 Rust `PathBuf` 与引擎实际接收的字符串路径可能不完全相同。

## 依赖与调用关系

上游主链：

`mockstore::new_unistore` / `newUnistore`
→ `pkg/store/mockstore/unistore.rs::build_embedded`
→ `embedded_unistore::New`
→ 本文件 `New`。

直接测试调用还包括：

- `mock_test.rs::temp_prefixed_path_is_volatile_and_removed_on_close`；
- `rpc_test.rs` 中 Region 属性、请求 marker 与关闭场景；
- `pd_test.rs::set_up_suite` 以及 keyspace、PD member 场景。

主要下游调用边：

- `New` → `create_temp_directory`、`fs::create_dir_all`；
- `New` → `server::new_mock` → 数据库/lock store/初始 Region/inner server；
- `New` → `Cluster::new`；
- `New` → `RPCClient::new` → `RawHandler::new`；
- `New` → `PdClient::new` → `MockKeyspaceManager::new` 和 PD 地址规范化。

Cargo 边界方面，本 crate 直接依赖同目录的 `config`、`server`、`tikv` 子 crate 和
`astersql-util`。本文件通过 `lib.rs` 的再导出名称引用前三者；没有 feature 条件或
条件编译分支。`fail` 是 crate 依赖但未被本文件直接使用。

RustCodeGraph 的文件关系把 `mock.rs` 标记为被 `mock_test.rs`、`pd_test.rs`、
`rpc_test.rs` 和 `pkg/store/driver/tikv_driver.rs` 使用；其中真正的生产构造调用由
`pkg/store/mockstore/unistore.rs::build_embedded` 的源码明确给出。通用名称 `New`
的图查询存在重名噪声，因此调用主链以文件级图结果和该适配层源码共同核验。

## 错误处理与边界

- 临时目录创建和根目录创建的 IO 错误通过 `From<std::io::Error>` 转成
  `NewError::Io`。
- server 侧错误通过 `NewError::Server` 返回，包括配置形状错误、数据库打开失败、
  Region 引导失败和 inner server 启动失败。
- PD 初始化通过 `NewError::Pd` 返回。当前可见校验拒绝超限 keyspace ID、重复 ID
  和重复名称。
- 无效 PD 地址在 `normalize_mock_pd_addrs` 中被过滤，而不是让 `New` 失败；调用者
  必须区分“地址列表为空”和“构造失败”。
- 空路径有特殊含义；非空但带 `tidb-unistore-temp` 字符串前缀的路径同样会在关闭
  时递归删除。调用者不应把需保留的数据放在这种命名的路径下。
- `create_temp_directory` 使用纳秒时间名和 `create_dir_all`，不提供排他创建保证。
- `to_string_lossy` 意味着非 UTF-8 路径没有严格的字节级保真保证。
- `SystemTime` 早于 epoch 不会报错，而会使用 nonce 0；这提高了构造容错，但也增加
  路径碰撞可能性。
- `New` 不做事务性回滚：在目录或 server 已创建后发生后续错误，可能留下资源。
  若增强该路径，应增加独立回归测试，而不能只依赖成功路径测试。

## 并发与资源生命周期

`New` 自身是同步函数，没有创建显式 Rust 任务或通道。它调用的
`server::new_mock` 会启动 standalone inner server，并把死锁检测 leader/started
状态设为真。返回后，各句柄通过 `Arc` 共享；本文件不持有全局单例，因此多个
`New` 实例在对象状态上相互独立，但自动临时目录名仍依赖时间 nonce 的唯一性。

主要资源所有者是 `RPCClient`：

- `RPCClient::close` 用 `AtomicBool::swap` 实现幂等关闭；
- 第一次关闭先停止 server，再在 `persistent == false` 且路径存在时递归删除目录；
- `Drop for RPCClient` 会忽略关闭错误并调用 `close`，所以最后一个 `Arc<RPCClient>`
  释放时会尽力清理；需要获知停止/删除错误的调用者必须显式调用 `close`；
- 持久路径只停止服务，不删除目录；易失路径删除整个传入根目录；
- 返回的 `Cluster` 可能比 RPC client 活得更久，但 server 的停止和目录清理由
  RPC client 生命周期触发，不由 `Cluster` 或 `PdClient` 驱动。

`Cluster` 的一次性延迟表由 `Mutex` 保护，`PdClient` 的配置/keyspace 状态使用
`RwLock` 和原子量，`RPCClient` 的关闭位使用原子量；本文件通过共享同一实例而不是
复制状态，维持这些并发不变量。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/store/mockstore/unistore/mock.go`，总体流程保持一致：空路径建
临时目录、以 `tidb-unistore-temp` 字符串前缀判断易失性、创建目录、覆盖同一组引擎
配置、以 cluster ID 1 启动 mock server、构造 cluster/RPC/PD 并返回。

需要注意的表示与实现差异：

- Go 返回 `(*RPCClient, pd.Client, *Cluster, error)`；Rust 返回具体 `PdClient` 和
  三个 `Arc`，错误统一为 `NewError`。
- Go 用 `os.MkdirTemp` 生成具备排他创建语义的随机后缀；Rust 用当前纳秒数拼路径并
  `create_dir_all`。
- Go 用 `strings.HasPrefix(path, path.Join(os.TempDir(), ...))`；Rust明确采用
  `to_string_lossy().starts_with(...)` 保持这种文本前缀语义。
- Go 把 `RPCClient` 字段逐项初始化并将 `srv.RPCClient = client`；Rust 封装为
  `RPCClient::new`，其 server 调度模型不需要同样的回指赋值。
- Go 的 `newPDClient` 接口本身不返回错误；Rust `PdClient::new` 会验证初始
  keyspace，因此 `NewError` 多出 `Pd` 分支。
- 上层 Go `pkg/store/mockstore/unistore.go::newUnistore` 会进一步包装 PD client、按
  keyspace 类型创建测试 TiKV store；Rust 对应的
  `pkg/store/mockstore/unistore.rs::build_embedded` 当前直接应用 hijacker 并组装
  `MockStorage`。本文件只负责两边共同的底层三件套构造职责。

Rust 独立测试 `mock_test.rs::temp_prefixed_path_is_volatile_and_removed_on_close` 专门
锁定 Go 的字符串前缀兼容语义：即使调用者显式传入 `tidb-unistore-temp-*` 路径，
关闭后也必须被删除。

## 扩展指南

- 新增启动参数时，先判断它属于构造编排还是具体子系统。只影响引擎启动顺序、共享
  对象接线或生命周期的内容应进入 `New`；具体 RPC/PD/Region 行为应留在各自模块。
- 修改持久性判断必须同步 `mock_test.rs`，至少覆盖空路径、普通持久路径、显式
  `tidb-unistore-temp-*` 路径和非 UTF-8 路径策略；不能用组件级
  `Path::starts_with` 直接替代当前 Go 兼容语义。
- 修改易失配置时，应与 `mock.go::New` 的字段逐项比对，并在独立测试中通过
  `client.server().database().options()` 一类可观察接口验证真实传递结果，避免只测
  局部配置变量。
- 增加 keyspace 规则时，应同步 `pd_test.rs`，并确保失败发生后 server/目录得到
  回滚；若在 `New` 中加入 guard，应验证成功时所有权正确移交、失败时只清理本次
  创建的目录。
- 调整返回类型或 Arc 所有权会影响 `pkg/store/mockstore/unistore.rs::build_embedded`
  的 hijacker 接线以及大量使用 `RPCClient`/`PdClient`/`Cluster` 的测试，应优先保持
  三者共享同一 server/region manager 的不变量。
- 测试逻辑应继续放在独立的 `mock_test.rs`（必要时扩展 `pd_test.rs` 或
  `rpc_test.rs`），不要内嵌回生产源文件。Rust 行为变化还应与同路径 Go 文件和测试
  意图对齐。
- 性能风险主要来自易失配置、compactor 数量、memtable/value-log 大小和同步写；
  正确性风险主要来自路径误删、初始化半失败泄漏及 server/cluster/PD 使用不同元数据
  实例；兼容风险主要来自 Go 前缀规则和错误可见性的差异。

## 验证依据

本说明使用以下直接证据：

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件，目标目录和 `mock.rs` 已被
  索引；`files --filter pkg/store/mockstore/unistore` 确认相关源/测试集合。
- RustCodeGraph `node --file pkg/store/mockstore/unistore/mock.rs`：核对
  `NewError`、`New`、`create_temp_directory` 的完整实现和目标文件的文件级使用者。
- RustCodeGraph `query`：精确定位 `mock.rs::New`（第 71 行）与
  `mock.rs::create_temp_directory`（第 125 行）；通用 callers/callees 对 `New` 重名
  消歧不稳定，未把噪声结果当成调用事实。
- RustCodeGraph `node`：核对 `unistore.rs::build_embedded` 的上游调用；核对
  `server.rs::new_mock/create_db`、`cluster.rs::Cluster::new`、
  `rpc.rs::RPCClient::new/close/Drop`、`pd.rs::PdClient::new` 与
  `MockKeyspaceManager::new` 的下游语义。
- Cargo 边界：`pkg/store/mockstore/unistore/Cargo.toml` 和 `lib.rs`。
- Go 对照：`pkg/store/mockstore/unistore/mock.go` 与上层
  `pkg/store/mockstore/unistore.go`。
- Rust 独立测试：`pkg/store/mockstore/unistore/mock_test.rs`；调用覆盖补充参考
  `pd_test.rs` 和 `rpc_test.rs`。Go 的 PD 套件也通过同包 `New` 建立测试环境。

本任务是纯文档分析，未修改 Rust/Go/Cargo，未运行 Cargo。结构验收通过固定标题
命令检查；人工复核重点为构造顺序、路径删除条件、共享对象关系、Go 差异和失败清理
边界均能回溯到上述符号或文件。
