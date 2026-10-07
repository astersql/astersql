# `pkg/domain/infosync/mock_info.rs`

## 文件定位

本文件属于 `astersql-domain-infosync` crate；crate 入口 `pkg/domain/infosync/lib.rs` 以 `mod mock_info; pub use mock_info::*;` 将这里的类型和函数重新导出。它不是 etcd/PD 上的生产节点注册实现，而是一个进程内的 mock 注册表：`GlobalInfoSyncerInit` 在完成 `InfoSyncer` 初始化后调用 `MockGlobalServerInfoManagerEntry().Add(...)`，供无远端注册表或测试辅助路径按节点 ID 查询服务器信息。

`pkg/domain/infosync/Cargo.toml` 将 Go 对照包声明为 `pkg/domain/infosync`，并直接依赖 `serde`（derive）和 `serde_json`；本文件实际直接使用 `serde`，错误返回复用 crate 根导出的 `Error` 与 `Result`。源文件已由 `pkg/domain/infosync/lib.rs` 纳入编译，相关单元测试则独立放在 `pkg/domain/infosync/info_test.rs`，没有把测试嵌入生产源文件。

## 核心职责

- `ServerInfo` 定义此 crate 对外使用的扁平节点信息，并通过 serde 字段重命名维持 Go/etcd JSON 字段名，例如 `ID -> ddl_id`、`Port -> listening_port`、`JSONServerID -> server_id`。
- `MockGlobalServerInfoManager` 串行化管理一组 mock 节点：分配从 4000 起递增的 SQL 端口，按下标或执行器地址删除，生成按节点 ID 索引的快照，并在 `Close` 时清空状态。
- `ServerIdGetter` 与 `MockServerInfo` 保留每个节点的动态 server ID 获取器；`GetAllServerInfo` 每次生成快照时重新调用获取器，而不是只返回 `Add` 时缓存的数值。
- `MockGlobalServerInfoManagerEntry` 用 `OnceLock` 提供进程级唯一管理器，避免每个调用者各自维护注册表。

它只提供进程内测试/兼容状态，不持久化、不访问网络，也不替代 `info.rs::GetAllServerInfo` 的 etcd 前缀扫描路径。

## 主要符号

- `pub struct ServerInfo`：可克隆、可比较、可 serde 序列化的节点快照。字段包括版本与 Git hash、DDL 节点 ID、IP、SQL/状态端口、lease、启动时间、JSON server ID 和标签。字段名保持 Go 风格是 crate 级 `#![allow(non_snake_case)]` 兼容策略的一部分。
- `type ServerIdGetter = Arc<dyn Fn() -> u64 + Send + Sync>`：线程安全、可共享的惰性 server ID 回调；类型本身为模块私有，但出现在公开 `Add` 方法签名中（调用者以兼容的 `Arc` 闭包传入）。
- `pub struct MockGlobalServerInfoManager`：公开管理器，唯一字段是私有的 `Mutex<MockGlobalServerInfoManagerState>`，外部不能绕过方法破坏状态不变量。
- `struct MockServerInfo`：私有条目，将可返回的 `ServerInfo` 与其动态 `getter` 绑定。
- `struct MockGlobalServerInfoManagerState`：私有可变状态；`infos` 保存插入顺序，`mock_server_port` 保存下一个待分配端口。
- `Default for MockGlobalServerInfoManager`：创建空表，并把下一个端口设为 4000。
- `Add(id, getter)`：构造回环地址节点，记录当前 Unix 秒时间戳，立即求值一次 server ID，分配当前端口后递增计数器并追加条目。
- `Delete(idx) -> Result<()>`：删除指定下标；越界时返回 `Error::External("server idx out of bound")`。
- `DeleteByExecID(exec_id)`：按字符串 `"{IP}:{Port}"` 查找并删除首个匹配条目；找不到时静默保持原状。
- `GetAllServerInfo() -> HashMap<String, ServerInfo>`：克隆全部条目，重新求值 `JSONServerID`，并以 `ID` 为 key 收集成独立快照。
- `Close()`：清空条目并将端口计数器恢复为 4000。
- `MockGlobalServerInfoManagerEntry() -> &'static MockGlobalServerInfoManager`：经函数内静态 `OnceLock` 延迟创建并返回全局管理器。

## 执行流程

1. `info.rs::GlobalInfoSyncerInit` 构造并发布全局 `InfoSyncer` 后，把 `uuid` 和 `serverIDGetter` 传给全局 mock 管理器的 `Add`。
2. `Add` 获得互斥锁，以 `127.0.0.1`、当前 `mock_server_port`、当前 Unix 秒以及回调当前值创建 `ServerInfo`；随后端口加一并把条目追加到 `infos`。即使调用者传入重复 ID，向量中也会保留多个条目。
3. 查询者调用 `GetAllServerInfo`。方法在锁内遍历向量，克隆每个 `ServerInfo`，再次调用对应 getter 覆盖克隆体的 `JSONServerID`，最后收集为 `HashMap`。因此返回值与内部条目不共享可变对象；重复 ID 在收集时由后出现的条目覆盖先出现的条目。
4. `info.rs::GetServerInfoByID` 从该快照移除并返回目标 ID，缺失时构造 `Error::External("server {id} not found")`。`pkg/util/disttask/idservice.rs::GenerateSubtaskExecID4Test` 同样读取快照，再把命中的 `IP` 和 `Port` 格式化为执行器 ID；缺失返回空串。
5. 清理可按向量下标调用 `Delete`，或按地址调用 `DeleteByExecID`。测试/生命周期结束时调用 `Close`，从而同时移除所有条目并复位端口分配序列；`OnceLock` 中的管理器对象本身不会销毁。

## 数据与状态

核心不变量是：默认或 `Close` 后 `infos` 为空且下一个端口为 4000；每次成功执行 `Add` 恰好消费一个连续端口。删除不会回收端口，只有 `Close` 会复位计数器。`infos` 使用 `Vec`，所以插入顺序和按下标删除语义明确；对外快照使用 `HashMap`，不承诺遍历顺序。

`ServerInfo` 的字符串、标签表和数值在快照时均被克隆；调用者修改返回值不会回写注册表。内部保存的 `info.JSONServerID` 是加入时的值，但每次快照都以 getter 的最新结果覆盖返回副本，因此外部观察到的是查询时值。`StartTimestamp` 在加入时取 `SystemTime::now()` 相对 Unix epoch 的秒数；若系统时钟早于 epoch，`unwrap_or_default` 会使时间戳退化为 0，而不会返回错误。

公开结构的 serde 名称是协议状态的一部分。当前 `ServerInfo` 没有 Go `serverinfo.ServerInfo` 的 `StaticInfo`/`DynamicInfo` 嵌套结构，也不包含 Keyspace 或回调字段；它只保留本 crate 当前消费的扁平字段。

## 依赖与调用关系

上游调用边（RustCodeGraph 与文本引用共同验证）：

- `pkg/domain/infosync/info.rs::GlobalInfoSyncerInit -> MockGlobalServerInfoManagerEntry -> Add`：初始化时登记本节点。
- `pkg/domain/infosync/info.rs::GetServerInfoByID -> MockGlobalServerInfoManagerEntry -> GetAllServerInfo`：Rust 当前按 ID 查询走 mock 快照。
- `pkg/util/disttask/idservice.rs::GenerateSubtaskExecID4Test -> MockGlobalServerInfoManagerEntry -> GetAllServerInfo`：测试用分布式任务执行器 ID 解析。
- `pkg/domain/infosync/info_test.rs` 和 `pkg/util/disttask/idservice_test.rs` 直接取得管理器以隔离、准备和清理全局测试状态。

下游依赖全部位于标准库或 crate 边界内：`Mutex` 保护状态，`Arc` 共享回调，`OnceLock` 管理单例，`SystemTime` 生成启动时间，`HashMap` 形成查询快照，`serde` 生成 JSON 编解码实现，`crate::{Error, Result}` 承载 `Delete` 的边界错误。方法本身不调用 etcd、PD 或文件系统。

需要特别区分 `mock_info.rs::GetAllServerInfo` 与 `info.rs::GetAllServerInfo`：后者是公开集群查询 API，有 etcd 时读取 `ServerInformationPath` 前缀，没有 etcd 时返回 `InfoSyncer.server_info`；前者只读取本进程 mock 表。生产辅助函数 `GenerateSubtaskExecID` 使用前者，只有 `GenerateSubtaskExecID4Test` 使用本文件的注册表。

## 错误处理与边界

- `Delete` 是唯一显式返回错误的方法；`idx >= infos.len()` 返回 `Error::External`，不会改变状态。参数是 `usize`，因此 Go 版本的负索引分支在 Rust 类型系统中不可表达。
- `DeleteByExecID` 未命中不报错，只删除首个匹配项。当前实现用普通 `format!("{}:{}", IP, Port)`，不会像 Go 的 `net.JoinHostPort` 那样给 IPv6 地址加方括号；而 `Add` 当前固定生成 IPv4 回环地址，所以内建条目不触发该差异。若未来允许自定义 IP，必须先明确 IPv6 兼容要求并补测试。
- `GetAllServerInfo` 收集为 `HashMap` 时不会拒绝重复 ID，后插入值覆盖先插入值；内部向量仍保留两项，后续按下标/地址删除的行为也仍基于向量。
- 所有锁都使用 `lock().unwrap()`：若持锁线程 panic 导致互斥锁 poisoned，后续操作会 panic，而不是返回可恢复错误。
- getter 在持锁期间执行。getter 若 panic，锁会 poisoned；若 getter 同步重入同一管理器，非重入 `Mutex` 可能死锁。调用方应提供快速、无阻塞且不重入管理器的回调。
- `mock_server_port += 1` 在极端数量节点下存在整数溢出边界；当前测试用途和从 4000 起的生命周期没有为耗尽端口定义恢复策略。

## 并发与资源生命周期

管理器的所有可变字段由同一个 `Mutex` 覆盖，所以 `Add`、两种删除、快照和 `Close` 相互串行，调用者不会观察到半完成的条目或端口更新。`ServerIdGetter` 要求 `Send + Sync`，允许管理器在线程间共享；`MockGlobalServerInfoManager` 也因其字段组成而可安全共享。

进程级实例由 `OnceLock` 初始化一次，返回 `'static` 引用。`Close` 只重置内部逻辑状态，并不释放该单例，也不自动从任何运行中的 `InfoSyncer` 生命周期解绑。使用全局注册表的测试必须串行化并在前后调用 `Close`；`pkg/util/disttask/idservice_test.rs` 的 `registry_serial` 和 `pkg/domain/infosync/info_test.rs` 的 `serial` 正是为共享状态隔离服务。

快照创建期间锁一直持有，包括克隆字段和执行全部 getter；节点数或 getter 延迟会线性延长其他管理操作的等待时间。锁释放后，返回的 `HashMap` 完全独立，读取它无需继续持有管理器锁。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/domain/infosync/mock_info.go`。两版都维护受互斥锁保护的顺序条目，从端口 4000 开始逐次分配，支持按下标/执行器 ID 删除、生成 ID 到节点的映射，并在 `Close` 后清空且复位端口。

当前可验证差异如下：

- Go `getServerInfo` 从全局配置填充 advertise address、status port、lease、keyspace、labels，并填入 TiDB 版本与 Git hash；Rust `Add` 固定 `127.0.0.1`，只主动填充 ID、SQL 端口、启动时间和 server ID，其余字段保持默认值。因此不能把 Rust mock 文档化为完整复刻生产拓扑元数据。
- Go 数据模型复用 `pkg/domain/serverinfo.ServerInfo` 的静态/动态嵌套结构并保存 `ServerIDGetter`；Rust 在本文件定义扁平 `ServerInfo`，把 getter 存在私有 `MockServerInfo` 中，并在每次 `GetAllServerInfo` 时刷新返回副本的 `JSONServerID`。
- Go `GetAllServerInfo` 返回内部 `*ServerInfo` 指针组成的 map；Rust 返回深度克隆的值快照，隔离调用方修改。
- Go `Delete` 接受 `int` 并显式拒绝负数；Rust 接受 `usize`，只需检查上界。
- Go `DeleteByExecID` 使用 `net.JoinHostPort`，正确括起 IPv6；Rust 当前使用直接冒号拼接。
- Go 的入口是包级已初始化变量；Rust 用函数加 `OnceLock` 惰性初始化，调用语法为 `MockGlobalServerInfoManagerEntry()`。
- Go 当前 `GlobalInfoSyncerInit` 使用独立的 `serverinfo.Syncer`，并不在该函数里向 mock 管理器登记；Rust `GlobalInfoSyncerInit` 明确执行 `Add`。这是当前仓库两条实现链的实际接线差异。

## 扩展指南

- 增加或修改拓扑字段时，先更新 `ServerInfo` 及 serde rename，并同时核对 `pkg/domain/infosync/info.rs` 的 etcd JSON 解码、`pkg/domain/infosync/types_test.rs` 的线格式断言，以及 Go `pkg/domain/serverinfo` 模型；协议字段变更存在跨版本兼容风险。
- 改变 mock 节点构造策略时，修改 `Add`（或抽出私有构造函数），不要让调用者直接访问私有状态。若要进一步对齐 Go 配置字段，应明确哪些全局配置在测试中稳定，并扩展独立的 `pkg/domain/infosync/info_test.rs`，避免把测试放回生产文件。
- 若要支持自定义/IPv6 地址，应让 `DeleteByExecID` 和 `pkg/util/disttask/idservice.rs::GenerateExecID` 共享一致的 host/port 规范，并添加 IPv4、IPv6、空 host 和未命中用例；否则登记端和删除端可能生成不同 key。
- 若要改变动态 server ID 语义，应同时检查 `Add` 与 `GetAllServerInfo` 的 getter 调用时机，并新增“回调值变化后快照刷新”、panic/重入约束或明确的错误转换测试。
- 若允许并发规模扩大或 getter 执行变重，可考虑先在锁内复制条目、锁外调用 getter；但必须验证 `Close`/删除与快照的一致性语义，不能无意把单一原子快照变成混合时点结果。
- 删除或重命名公开符号前需同步 `info.rs`、`pkg/util/disttask/idservice.rs` 及各自独立测试。尤其不要把 mock `GetAllServerInfo` 与生产集群 `info.rs::GetAllServerInfo` 合并为含糊接口。

## 验证依据

- RustCodeGraph `status`：索引覆盖 11,467 个文件，其中 Rust 文件 7,032 个；目标文件已索引。
- RustCodeGraph `node --file pkg/domain/infosync/mock_info.rs --offset 1 --limit 400`：核对了目标文件全部 144 行、所有结构体、方法、serde 属性和条件编译情况（本文件无条件编译项）。
- RustCodeGraph `query MockGlobalServerInfoManager`、`query MockGlobalServerInfoManagerEntry` 与精确 `node`：核对 Rust/Go 同名定义，并确认 `MockGlobalServerInfoManagerEntry` 的调用者包括 `GlobalInfoSyncerInit`、`GetServerInfoByID`、`GenerateSubtaskExecID4Test` 及相关测试。
- RustCodeGraph `node GlobalInfoSyncerInit`、`node GetServerInfoByID`、`node GenerateSubtaskExecID4Test`：核对初始化、按 ID 查询和分布式任务测试解析的调用边。通用 `callers Add/GetAllServerInfo` 因跨语言同名歧义不够精确，额外用 `rg` 对直接引用作了补证。
- 已读 crate/入口：`pkg/domain/infosync/Cargo.toml`、`pkg/domain/infosync/lib.rs`；该目录没有 `doc.go`。
- 已读实现/调用：`pkg/domain/infosync/mock_info.rs`、`pkg/domain/infosync/info.rs`、`pkg/util/disttask/idservice.rs`。
- 已读 Go 对照：`pkg/domain/infosync/mock_info.go`、`pkg/domain/infosync/info.go`、`pkg/domain/serverinfo/info.go`、`pkg/domain/serverinfo/syncer.go`。
- 已读独立测试：`pkg/domain/infosync/info_test.rs::mock_server_info_manager_matches_go_lifecycle` 验证端口 4000/4001、按地址删除和 `Close` 后复位；`pkg/util/disttask/idservice_test.rs::{test_registry_resolution_matches_go_fallbacks, production_registry_resolves_remote_server_info}` 验证缺失回退、mock 地址解析及生产 etcd 路径与 mock 路径的区分。现有直接测试未覆盖 `Delete` 的成功/越界分支、动态 getter 更新、重复 ID、IPv6 删除和锁 poisoned/reentrant 情况。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以任务指定命令验证文档存在且恰有 11 个固定二级标题，并人工复核所有行为结论均可回指上述源码、调用边或测试。
