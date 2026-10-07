# `pkg/domain/serverinfo/info.rs`

## 文件定位

本文件是 `astersql-domain-serverinfo` crate 的数据模型与 JSON 编解码层。crate 入口 `pkg/domain/serverinfo/lib.rs` 通过 `pub use info::*` 对外重导出这里的类型、常量和方法；同 crate 的 `syncer.rs` 负责把这些值写入或读出 etcd。`pkg/domain/serverinfo/Cargo.toml` 将 Go 对照包标为 `pkg/domain/serverinfo`，而本文件自身只使用 Rust 标准库；etcd、Tokio、配置等依赖由同 crate 的同步层使用。

在应用主链中，`pkg/domain/domain.rs` 的服务器注册流程创建 `Syncer`，随后调用 `NewSessionAndStoreServerInfo` 和 `NewTopologySessionAndStoreServerInfo`。前者最终把 `ServerInfo` 写入 `/tidb/server/info/<id>`，后者把 `TopologyInfo` 写入 `/topology/tidb/<ip:port>/info`。因此本文件不负责网络 I/O 或租约管理，而是定义这两个持久化值的形状和 Go 兼容编解码语义。

## 核心职责

- 定义节点版本、静态身份、动态标签、完整服务器信息和拓扑信息：`VersionInfo`、`StaticInfo`、`DynamicInfo`、`ServerInfo`、`TopologyInfo`。
- 提供 etcd 路径、重试/超时、拓扑租约和刷新间隔常量。`syncer.rs` 直接使用 `ServerInformationPath`、`TopologyInformationPath`、`KeyOpDefaultRetryCnt`、`KeyOpDefaultTimeout`、`TopologySessionTTL` 与 `TopologyTimeToRefresh`。
- 通过 `ServerInfo::Marshal`/`Unmarshal` 和 `TopologyInfo::Marshal`/`Unmarshal` 生成或解析稳定 JSON；实现重点是复现 Go `encoding/json` 在字段名、空值、部分更新、首个类型错误和字符串转义方面的行为。
- 在 `ServerInfo::ToTopologyInfo` 中裁剪完整节点信息，生成拓扑发现所需字段。
- 通过私有 `Json`、`JsonParser` 和 `assign_*` 辅助函数完成无第三方 JSON 依赖的解析及类型检查。

## 主要符号

- 路径与时序常量：`ServerInformationPath` 为 `/tidb/server/info`，`TopologyInformationPath` 为 `/topology/tidb`；键操作默认重试 5 次、单次超时 1 秒；拓扑 session TTL 为 45 秒、刷新间隔为 30 秒。`minTSReportInterval` 也是 30 秒，但为 crate 内部使用。
- 版本常量：`TiDBReleaseVersion` 和 `ServerVersion` 当前都是带 `this-is-a-placeholder` 的 Rust 占位值。前者写入拓扑，后者由 `syncer.rs::getServerInfo` 写入完整服务器信息，不能把它们描述成真实构建版本。
- `VersionInfo { Version, GitHash }`：被内嵌到静态信息和拓扑信息中。
- `StaticInfo`：保存节点 ID、IP、SQL/status 端口、lease 描述、启动时间、所属/假定 keyspace，以及服务器 ID。`ServerIDGetter: Option<Arc<dyn Fn() -> u64 + Send + Sync>>` 是进程内回调，不进入 JSON；`JSONServerID` 才对应 JSON 的 `server_id`。`IsAssumed` 仅以 `AssumedKeyspace` 是否非空判定跨 keyspace 假定身份。
- `DynamicInfo { Labels }`：运行时可变部分。`Clone` 返回堆分配副本；底层 `HashMap` 深拷贝，修改副本不会改变原标签。
- `ServerInfo { StaticInfo, DynamicInfo }`：完整节点记录。`Clone` 保留 `Arc` 回调引用并深拷贝标签；`Display` 克隆后调用 `Marshal`，避免为了刷新 `JSONServerID` 而修改原对象。
- `TopologyInfo`：仅含版本/Git 哈希、IP、status 端口、部署目录、启动时间和标签，供拓扑发现使用。
- `ServerInfoError(String)`：本地编解码错误类型，实现 `Display` 与 `std::error::Error`；`syncer.rs` 可将其转换为 `SyncError`。
- 私有解析层：`Json` 表示对象、数组、字符串、数字、布尔和 null；`JsonParser::{parse,value,object,array,string,unicode_code_unit,number,keyword}` 执行语法解析；`assign_string`、`assign_u32`、`assign_u64`、`assign_i64`、`assign_labels` 执行目标字段类型和范围检查；`preserve_first_error` 保存第一个赋值错误。

## 执行流程

1. 构造阶段：`syncer.rs::getServerInfo` 从全局配置生成 `ServerInfo`，注入 `ServerIDGetter`，并填充静态字段和标签；`newSyncer` 将其置于 `Arc<RwLock<ServerInfo>>` 中。
2. 完整信息写入：`Syncer::StoreServerInfo` 取得写锁后调用 `ServerInfo::Marshal`。该方法必须先调用 `ServerIDGetter` 刷新 `JSONServerID`，然后按固定字段顺序输出 JSON；空 `Keyspace`/`AssumedKeyspace` 被省略，标签先按键排序。结果由同步层连同 server-info session lease 写入 etcd。
3. 完整信息读取：`syncer.rs::getInfo` 为每个 etcd 值创建默认 `ServerInfo` 并调用 `Unmarshal`。解析器先验证完整 JSON 语法，再按不区分 ASCII 大小写的字段名逐项更新已有对象；未知字段被忽略。成功后以解析出的 `JSONServerID` 重建一个固定返回该值的 `ServerIDGetter`。
4. 拓扑写入：`Syncer::StoreTopologyInfo` 调用 `ServerInfo::ToTopologyInfo`。转换使用 `TiDBReleaseVersion`、原 Git 哈希和节点/标签字段，并从当前可执行文件的父目录得到 `DeployPath`，失败时回退到 `.`。随后 `TopologyInfo::Marshal` 生成稳定 JSON并写入拓扑 `/info` 键。
5. 拓扑读取：`Syncer::GetAllTiDBTopology` 过滤 `/info` 键并逐个调用 `TopologyInfo::Unmarshal`；缺失字段保留默认零值，未知字段只参与 JSON 语法验证。

## 数据与状态

`StaticInfo` 的设计不变量是节点运行期间原则上不变；例外是 `JSONServerID` 在每次 `Marshal` 前从回调刷新，因为 PD 重连等场景可能改变 Domain 持有的 server ID。`ServerIDGetter` 使用 `Arc` 使克隆后的记录仍能查询同一动态来源；反序列化得到的记录无法恢复原外部来源，所以改为捕获已解码 ID 的固定闭包。

`DynamicInfo::Labels` 是唯一明确的运行时动态数据。`syncer.rs::UpdateServerLabel` 先克隆标签，只有实际变化时才对克隆后的完整 `ServerInfo` 编码并写入 etcd，成功后再替换本地动态部分。序列化时对 `HashMap` 键排序，使相同逻辑状态产生确定字节序；反序列化到已有对象时，labels 对象是增量插入而非先整体清空，但 JSON `null` 会清空标签。

`StaticInfo::Unmarshal` 只处理静态字段，明确忽略 `labels` 等动态字段；JSON `null` 表示不修改当前静态值。`ServerInfo::Unmarshal` 同样允许在现有对象上部分更新，因此调用者不能假设未出现的字段会被清零。

## 依赖与调用关系

直接上游由 RustCodeGraph 文件关系和已索引源码确认：`pkg/domain/serverinfo/lib.rs` 重导出本模块；`pkg/domain/serverinfo/syncer.rs` 构造、锁定、克隆、编解码并持久化这些类型；`pkg/domain/domain.rs` 启动服务器信息与拓扑注册；`pkg/domain/canonical_domain.rs`、`pkg/session/tidb_test.rs` 和 `pkg/domain/serverinfo/info_test.rs` 也使用本文件。RustCodeGraph 对本文件报告 4 个直接使用文件，其中包括 `canonical_domain.rs`、`domain.rs`、`info_test.rs`、`tidb_test.rs`；同 crate 内 `syncer.rs` 的具体调用由已索引源码进一步核实。

关键下游关系为：`ServerInfo::Marshal` 调用 `json_string` 并执行 `ServerIDGetter`；`ServerInfo::Unmarshal`/`StaticInfo::Unmarshal`/`TopologyInfo::Unmarshal` 调用 `JsonParser` 与 `assign_*`；`ToTopologyInfo` 调用 `std::env::current_exe`；所有克隆和共享回调仅依赖 `HashMap`、`Arc`、`Duration` 等标准库类型。Cargo 中的 `etcd-client`、`tokio`、`astersql-config` 与日志依赖属于 crate 的同步/基础设施层，而非本文件直接依赖。

## 错误处理与边界

- `ServerInfo::Marshal` 在 `ServerIDGetter` 未初始化时通过 `expect` panic；这是与 Go 无条件调用函数值相对应的编程错误，不是可恢复的编码错误。独立测试 `marshal_requires_server_id_getter_like_go` 固定了这一契约。
- 三个 `Unmarshal` 入口拒绝顶层类型不符和尾随数据。解析器拒绝缺冒号、非法分隔符、未终止字符串、非法转义、控制字符及不完整数字语法；未知字段仍必须是语法有效的 JSON。
- 字段名以 `eq_ignore_ascii_case` 匹配，重复字段按出现顺序处理，后值可覆盖前值。字段类型不符时保留最早错误，但继续尝试更新后续字段，所以返回 `Err` 不代表对象完全未改变。
- 标量上的 `null` 保留原值；`labels: null` 清空标签；label 值为 `null` 时插入空字符串。整数先保存为 JSON 数字文本，再由目标 Rust 整数类型解析，因此负数写入无符号字段或越界都会报错，小数/指数即使语法合法也不能赋给整数。
- 输入先经 `String::from_utf8_lossy`，无效 UTF-8 会替换为 U+FFFD。字符串解析支持 UTF-16 代理对；孤立或无效代理项同样用 U+FFFD 替换。`json_string` 还按 Go 默认 HTML 安全策略转义 `<`、`>`、`&`、U+2028 和 U+2029。
- `TopologyInfo::Marshal` 当前返回 `Vec<u8>` 而非 `Result`，因为其字段都可由本地字符串格式化；`ToTopologyInfo` 获取可执行路径失败时不报错而回退到 `.`。

## 并发与资源生命周期

本文件不创建线程、异步任务、channel、etcd session 或锁。并发所有权体现在 `ServerIDGetter` 的 `Send + Sync` 约束和 `Arc` 共享上；实际可变 `ServerInfo` 由 `syncer.rs` 的 `Arc<RwLock<_>>` 保护。`StoreServerInfo` 持写锁调用 `Marshal`，因为刷新 `JSONServerID` 会修改对象；只读获取则在读锁下克隆。

etcd 资源生命周期由 `Syncer` 管理：完整服务器信息绑定 45 秒 session lease；拓扑 `/info` 本身无 lease，而对应 `/ttl` 键绑定拓扑 session，并按 `TopologyTimeToRefresh` 周期刷新。本文件中的 `Duration`/TTL 常量为该生命周期提供参数，但不执行续约和清理。克隆 `ServerInfo` 会增加 getter 的 `Arc` 引用计数；对象最后一个引用释放时闭包资源自动释放，没有显式关闭步骤。

## 与 Go 版本的对应关系

直接对照 `pkg/domain/serverinfo/info.go`：Rust 保留了 Go 的字段名和 JSON 键、静态/动态信息拆分、`IsAssumed`、深拷贝标签、marshal 前刷新 server ID、unmarshal 后重建 getter，以及拓扑裁剪流程。`info_test.rs` 进一步覆盖 Go `encoding/json` 的省略空 keyspace、HTML/Unicode 转义、大小写不敏感字段、重复字段、部分更新、无符号范围和未知字段语义。

实现差异如下：Go 直接使用 `encoding/json`、`maps.Clone`、`os.Executable` 和 `mysql.TiDBReleaseVersion`；Rust 为避免本文件增加 JSON 依赖而内置轻量解析器，并用标准库手工编码。Go 的嵌入字段在 Rust 中变成显式 `StaticInfo`/`DynamicInfo` 成员。Go 的 `ServerIDGetter` 是可为 nil 的函数值，Rust 用 `Option<Arc<dyn Fn() -> u64 + Send + Sync>>` 表达，并在编码时显式 `expect`。Go 的真实版本来自 parser/mysql，而 Rust 当前两个版本常量仍是占位值，这是已存在的迁移限制。Go 侧没有同路径 `info_test.go`；最接近的 Go 行为集成证据在 `pkg/domain/serverinfo/syncer_test.go`，Rust 的直接编解码回归则独立放在 `pkg/domain/serverinfo/info_test.rs`。

## 扩展指南

新增完整服务器字段时，应先判断其生命周期：启动后不变的字段进入 `StaticInfo`，可在线更新的字段进入 `DynamicInfo`。随后必须同步更新对应 `Marshal` 字段名/省略规则、`unmarshal_fields` 或 `ServerInfo::Unmarshal` 分派、Go `info.go` 对照以及独立的 `info_test.rs`；若字段也供 PD 拓扑发现使用，还要同步 `TopologyInfo`、`ToTopologyInfo` 与拓扑编解码。不要把 Rust 测试嵌入生产源文件。

修改 JSON 行为时要保留三个兼容点：字段名大小写、对已有对象的部分更新、遇到首个类型错误后继续处理后续字段。给 `Json`/`JsonParser` 增加语法能力时，应在 `info_test.rs` 同时覆盖有效值、非法边界、Unicode 和未知字段嵌套；若要替换为外部 JSON crate，需逐项证明上述 Go 兼容行为而不能只验证正常往返。

修改路径、TTL 或刷新常量会影响 `syncer.rs` 的 etcd 键布局和资源时序，应同步检查 `syncer_test.rs` 与 Go `syncer_test.go`。修改 getter 或克隆策略时，还需检查锁持有范围、闭包的 `Send + Sync` 能力和标签是否仍为深拷贝。版本占位值若被真实构建元数据替换，应优先接入统一版本来源，避免 `ServerInfo` 与 `TopologyInfo` 发布不一致的版本。

## 验证依据

- RustCodeGraph 状态：本地索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`pkg/domain/serverinfo/info.rs` 被索引为 58 个符号。读取了该文件全部 747 行，并查询了 `ServerInfo`、`Marshal`、`Unmarshal`、`ToTopologyInfo`；精确 callers/callees 命令未返回可用结果，因此没有据此虚构函数级图边。
- 已索引源码：`pkg/domain/serverinfo/info.rs`、`pkg/domain/serverinfo/info_test.rs`、`pkg/domain/serverinfo/lib.rs`、`pkg/domain/serverinfo/syncer.rs`、`pkg/domain/domain.rs`、Go 对照 `pkg/domain/serverinfo/info.go`。
- 配置与测试证据：`pkg/domain/serverinfo/Cargo.toml`；直接测试 `info_test.rs` 的 9 个测试覆盖编码、Display、getter 缺失、部分更新、类型/Unicode、重复字段、拓扑默认值、深拷贝和静态身份解码；同步生命周期的相关覆盖位于 `pkg/domain/serverinfo/syncer_test.rs` 与 Go `pkg/domain/serverinfo/syncer_test.go`。
- 人工复核结论：本文件存在于网络同步层与 etcd 值之间，运行时由 `Syncer` 构造并在注册、标签更新、远端查询和拓扑刷新路径调用；安全扩展入口及必须同步的独立测试已在上一节列明。
- 本任务只新增说明文档，未运行 Cargo，也未修改 Rust、Go、Cargo 或只读总计划；最终以任务指定的 11 标题结构命令验证。
