# `br/pkg/config/ebs.rs`

## 文件定位

本文件属于 workspace 成员 `astersql-br-pkg-config`，其 crate 根是同目录的 [`lib.rs`](lib.rs)，后者把 `ebs` 声明为公开模块并通过 `pub use ebs::*` 扁平导出全部公开项。该 crate 的清单 [`Cargo.toml`](Cargo.toml) 用 `package.metadata.porting.go-package = "br/pkg/config"` 明确记录了 Go 来源包。

它是 Go [`ebs.go`](ebs.go) 的 Rust 移植：定义 EBS 卷级备份/恢复元数据的 JSON 模型、元数据的局部更新和校验，以及从外部存储读取 `backupmeta` 的入口。当前仓库搜索到的生产 Rust 代码没有依赖 `astersql-br-pkg-config`，`EBSBasedBRMeta` 和 `NewMetaFromStorage` 的 Rust 引用仅出现在本 crate 的独立测试中；`br/pkg/aws/ebs.rs`、`br/pkg/task/backup_ebs.rs` 和 `br/pkg/task/restore_ebs_meta.rs` 使用的是 `astersql-br-pkg-aws` 内另一套同名模型。因此，本文件目前是已实现并由契约测试覆盖、但尚未接入 Rust BR 生产主链的配置 crate，不能把 Go 调用链当作 Rust 已接线事实。

## 核心职责

- 用 `EBSVolume`、`EBSStore`、`ClusterInfo`、`Kubernetes`、三个组件结构和根结构 `EBSBasedBRMeta` 表达部署工具与 BR 交换的 EBS 元数据，并通过 `serde` 固定 Go 兼容的 JSON 字段名。
- 用 `EBSVolumeType_Valid` 将允许的卷类型限制为 `gp3`、`io1` 和 `io2`。
- 提供数量查询、集群信息惰性初始化、resolved-ts/备份类型/版本写入以及按卷 ID 批量回填快照 ID、恢复卷 ID 和可用区。
- 用 `ConfigFromFile` 加载本地 JSON，用 `NewMetaFromStorage` 从 `Storage` 读取 `metautil::metafile::MetaFile`（当前常量值为 `backupmeta`），并在外部存储入口执行完整性校验。
- 用私有辅助函数兼容 Go `encoding/json` 的局部覆盖与 `null` 容器语义，并模拟 Go `Masterminds/semver` v1 的宽松版本解析规则。

## 主要符号

- `Error(pub String)`：本文件的轻量错误类型，实现 `Display` 与 `std::error::Error`。文件 I/O、JSON、存储以及元数据校验错误最终都被压成消息字符串。
- `EBSVolumeType = String` 与 `GP3Volume`、`IO1Volume`、`IO2Volume`：保留 Go 字符串别名及常量的公开形状；`EBSVolumeType_Valid(&str) -> bool` 是 Rust 中的独立校验函数。
- `EBSVolume`：单卷记录，字段包含原卷 ID、类型、快照 ID、恢复卷 ID、可用区和状态。
- `EBSStore`：以 `StoreID` 关联一个 TiKV store 及其卷列表。
- `ClusterInfo`：保存集群版本、全备类型、resolved-ts 和副本映射；`Version` 反序列化时同时接受正式键 `cluster_version` 和历史键 `version`。
- `Kubernetes`：将 PV、PVC、TiDBCluster CRD 和附加选项保存为 `serde_json::Value`，不引入 Kubernetes 强类型依赖。
- `TiKVComponent`、`PDComponent`、`TiDBComponent`：组件级副本与拓扑信息；Rust 使用 `i64` 保存副本数，能覆盖测试中的大于 `i32::MAX` 值。
- `EBSBasedBRMeta`：根对象。可选字段对应 Go 指针，`Options` 与 `Region` 保存顶层扩展数据。
- `GetStoreCount`、`GetTiKVVolumeCount`：分别返回 store 数和首个 store 的卷数；后者依赖“各 TiKV 节点卷布局对称”的不变量。
- `String`、`ConfigFromFile`、`NewMetaFromStorage`：三个输入输出边界，分别负责 JSON 字符串化、本地文件加载、外部存储加载并校验。
- `CheckClusterInfo` 与 `SetResolvedTS`、`SetFullBackupType`、`SetClusterVersion`：在写入前保证 `ClusterInfo` 存在；相应 getter 不做初始化。
- `SetSnapshotIDs`、`SetRestoreVolumeIDs`、`SetVolumeAZs`：遍历所有 store/volume，以 `volume.ID` 查询映射并写入目标字段。
- 私有 `checkEBSBRMeta`、`deserialize_null_default`、`merge_json_value`、`masterminds_semver_is_valid`、`semver_identifiers_valid`：分别负责完整性校验、把 JSON `null` 容器变为默认空容器、递归合并对象、宽松语义版本校验及标识符校验。

## 执行流程

本地配置加载从 `ConfigFromFile` 开始：先用 `std::fs::read` 读取全部字节，再解析为 `serde_json::Value`。当根值不是 `null` 时，它先把当前 `self` 序列化为 JSON，随后由 `merge_json_value` 递归覆盖输入中实际出现的对象键，最后反序列化回 `EBSBasedBRMeta`。因此重复加载部分 JSON 时，缺失字段保留旧值；显式出现的标量、数组、`null` 或不同 JSON 类型则替换旧值。

外部存储加载从 `NewMetaFromStorage(&Context, &dyn Storage)` 开始：调用 `Storage::ReadFile` 读取键 `astersql_br_pkg_metautil::metafile::MetaFile`，将字节直接反序列化为根元数据，然后调用 `checkEBSBRMeta`。校验严格按“存在 `ClusterInfo` → 版本可解析 → `ResolvedTS != 0` → 至少一个 TiKV store”的顺序短路；只有全部满足才返回元数据。

备份侧的典型状态更新由 setter 完成：标量 setter 先通过 `CheckClusterInfo` 惰性创建集群信息；三个卷 setter 则取得现有 `TiKVComponent`，双层遍历 store 与 volume，并按原始卷 ID 查询映射。映射缺键时写入空字符串，与 Go map 的字符串零值一致。

## 数据与状态

所有状态都由调用者持有的 `EBSBasedBRMeta` 值承载，文件没有全局可变状态。根对象中的五个结构块使用 `Option` 表达 Go 的 nil 指针；多数字段以 `#[serde(default)]` 允许 JSON 缺省。`Volumes`、`Replicas`、Kubernetes 的 PV/PVC/Options 以及顶层 `Options` 额外使用 `deserialize_null_default`，让显式 `null` 变成空 `Vec` 或空 `HashMap`，对齐 Go nil slice/map 在本文件使用场景中的零值语义。

`ConfigFromFile` 的关键不变量是“输入未出现的字段不改变已有值”；`merge_json_value` 仅在两侧都是对象时递归，否则整体替换该节点。输入根 `null` 被当作无操作。`GetTiKVVolumeCount` 只看第一个 store，故不验证各 store 卷数是否一致；调用者若用它估算总卷数，必须先保证拓扑对称。

Kubernetes 对象故意是不透明 JSON。这保留跨语言 JSON 往返能力，却不提供 Rust 编译期的 PV/PVC/CRD 字段校验。`String` 返回紧凑 JSON；虽然正常数据结构通常可序列化，它仍在序列化失败时返回字面量 `<nil>`，保持 Go 方法的容错契约。

## 依赖与调用关系

直接依赖来自 [`Cargo.toml`](Cargo.toml)：`serde`/`serde_json` 负责模型编解码，`astersql-objstore-storeapi` 提供 `Context` 与 `Storage`，`astersql-br-pkg-metautil` 提供外部存储对象名。`Storage` trait 的 `ReadFile` 是一次性读取完整对象的同步接口；`MetaFile` 在 `br/pkg/metautil/metafile.rs` 定义为 `backupmeta`。

RustCodeGraph 将 `NewMetaFromStorage` 定位到本文件，并给出它到 `checkEBSBRMeta` 和 `Error` 的调用/构造边；对该函数执行 callers 查询未返回调用者。仓库文本搜索进一步确认，Rust 生产代码没有引用本 crate 的公开 API，调用点仅见 [`ebs_test.rs`](ebs_test.rs) 与 [`parity_test.rs`](parity_test.rs)。crate 根 [`lib.rs`](lib.rs) 是当前唯一模块入口。

Go 生产链则已接线：`br/pkg/task/backup_ebs.go` 创建和更新 `config.EBSBasedBRMeta`，调用数量查询及快照/AZ 回填；`br/pkg/task/restore_ebs_meta.go` 和 `br/pkg/task/restore_data.go` 通过 `config.NewMetaFromStorage` 读取恢复元数据。Rust 任务代码当前使用 `astersql-br-pkg-aws::EBSBasedBRMeta`，不是这里的类型；未来接线时需先解决这两套模型的归属与转换，不能仅增加一个 import。

## 错误处理与边界

`ConfigFromFile` 在读取、解析输入、序列化当前状态或反序列化合并结果的任一步失败时返回 `Error`；它不添加路径或阶段上下文，只保留底层错误文本。`NewMetaFromStorage` 同样把存储和 JSON 错误转成字符串，再传播校验错误。

`checkEBSBRMeta` 的可观察错误优先级固定：`no cluster info`、`invalid cluster version: <值>`、`invalid resolved ts`、`tikv info is empty`。版本校验允许小写 `v` 前缀、仅 major 或 major.minor、数字前导零，以及合法的 prerelease/build 标识符；拒绝空数字段、超过三个核心段、无法放入 `i64` 的数字和非法标识符。虽然清单声明了 `semver = "1"`，本文件并未调用该 crate，而是用私有解析器模拟 Go `Masterminds/semver` 的不同宽松规则。

`GetResolvedTS`、`GetFullBackupType` 在 `ClusterInfo` 缺失时通过 `expect` panic；三个卷回填方法在 `TiKVComponent` 缺失时也会 panic。这与 Go 直接解引用 nil 指针的前置条件相近，调用者应先初始化或从已校验元数据进入。回填映射缺键不是错误，而是覆盖为空字符串。`NewMetaFromStorage` 校验 store 非空，但不检查每个 store 的卷、ID、region、组件副本数或 Kubernetes 内容。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道或内部缓存。`EBSBasedBRMeta` 的变更都要求 `&mut self`，由 Rust 借用规则阻止同一值上的无同步并发写入；只读查询使用 `&self`。`Storage` trait 本身要求 `Send + Sync`，但 `NewMetaFromStorage` 只执行一次同步 `ReadFile`，既不拥有也不关闭存储对象。

本地文件由 `std::fs::read` 打开、完整读取并在调用返回前释放句柄；外部对象字节和 JSON 中间值都在函数栈帧内拥有，返回或报错时自动释放。该实现会同时持有原始字节、输入 JSON、当前对象序列化结果和最终对象，在大元数据场景下存在与文档大小成比例的峰值内存，但没有跨调用资源泄漏。

## 与 Go 版本的对应关系

类型和公开方法整体逐项对应 [`ebs.go`](ebs.go)：JSON 键、卷类型白名单、store/卷计数、setter、外部存储读取顺序和完整性检查顺序均保持一致。Rust `Option<T>` 对应 Go 指针，`Vec`/`HashMap` 对应 slice/map，缺失映射键写空字符串也对齐 Go 的 map 零值。

有几处有意或现实差异。Go 的 `EBSVolumeType` 是独立字符串类型并以方法 `Valid` 校验，Rust 是 `String` 别名加自由函数。Go Kubernetes 字段使用 `corev1` 强类型，Rust 保留为不透明 `serde_json::Value`。Go `json.Unmarshal(data, c)` 会保留输入中缺失的结构字段，Rust 通过递归 JSON 合并显式复现该行为。Go `semver.NewVersion` 的错误包含底层解析细节，Rust 只生成带原值的统一消息。Rust `ClusterInfo.Version` 还接受 fixture 使用的历史键 `version`；Go 结构 tag 只声明 `cluster_version`。

测试对应关系也分层存在：[`ebs_test.go`](ebs_test.go) 与 [`ebs_test.rs`](ebs_test.rs) 都只验证磁盘 fixture 可以加载；Rust [`parity_test.rs`](parity_test.rs) 进一步覆盖卷类型、fixture 内容、setter、错误顺序、宽松版本、缺失字段保留和 `NewMetaFromStorage` 签名。当前 Rust 测试没有用 Storage fake 执行 `NewMetaFromStorage` 的成功及读取失败路径，这些仍仅由实现和 Go 对照提供证据。

## 扩展指南

新增持久化字段时，应同时修改对应 Rust 结构及 serde 键、Go `ebs.go` 对应结构、fixture 和独立测试；容器字段若需接受 JSON `null`，应明确是否使用 `deserialize_null_default`。不要把测试内嵌进 `ebs.rs`，应扩展同目录的 [`ebs_test.rs`](ebs_test.rs) 或 [`parity_test.rs`](parity_test.rs)。

增加完整性约束应修改 `checkEBSBRMeta`，并在 `parity_test.rs` 增加每个失败分支及错误优先级断言，同时核对 Go 行为；更改校验顺序会影响用户看到的首个错误。扩充卷类型应同步常量、`EBSVolumeType_Valid`、Go `Valid` 和契约测试。

若要把此 crate 接入 Rust BR 生产链，首先应盘点并统一 `br/pkg/aws/ebs.rs` 中的同名本地模型，确定由配置 crate提供 canonical 类型还是建立显式转换；随后再迁移 backup/restore 调用点。直接并存或隐式复制字段会带来序列化漂移。若引入异步 Storage API、大对象流式解析或并行回填，还需重新评估 `Context` 取消语义、错误类型、内存峰值及线程安全边界。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/pkg/config` 确认覆盖 `ebs.rs`、Go 对照及独立测试；`query NewMetaFromStorage --json` 与 `query EBSBasedBRMeta --json` 定位目标符号；`node 'ebs.rs::NewMetaFromStorage'` 验证源码及其到 `checkEBSBRMeta`/`Error` 的边；callers 查询没有返回 Rust 调用者。
- 直接阅读：[`ebs.rs`](ebs.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)、Go 对照 [`ebs.go`](ebs.go)、Rust 测试 [`ebs_test.rs`](ebs_test.rs) 与 [`parity_test.rs`](parity_test.rs)、Go 测试 [`ebs_test.go`](ebs_test.go)。
- 直接依赖证据：`br/pkg/metautil/metafile.rs` 中 `MetaFile = "backupmeta"`；`pkg/objstore/storeapi/storage.rs` 中 `Storage: Send + Sync` 及 `ReadFile(&Context, &str) -> Result<Vec<u8>>`。
- 调用面搜索：对 Rust/Go 源码检索 `NewMetaFromStorage`、`EBSBasedBRMeta`、数量查询和三个回填方法，确认 Rust 当前仅测试引用，而 Go 的 backup/restore 任务链已有生产调用。
- 行为测试证据来自现有独立测试，本任务按计划不运行 Cargo；交付前另以任务指定命令验证本文恰好包含十一个固定二级章节。
