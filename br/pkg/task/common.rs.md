# `br/pkg/task/common.rs`

## 文件定位

`common.rs` 是 `astersql-br-pkg-task` crate 的公共任务配置与基础设施适配层。crate 根 `br/pkg/task/lib.rs` 以 `pub mod common` 挂载它；`br/pkg/task/Cargo.toml` 将该 crate 定义为 `br/pkg/task` Go 包的 Rust 移植库，并直接依赖 `astersql-metaservice`、恢复、GC、连接、流与 checkpoint 等 BR 子 crate。

它位于命令行与具体备份/恢复任务之间：`br/cmd/br/main.rs` 经命令层 `DefineCommonFlags` 注册公共参数，`backup.rs`、`restore.rs`、`stream.rs`、`backup_raw.rs`、`backup_txn.rs` 等再调用 `Config::ParseFromFlags`、`NewMgr`、`GetStorage` 或 `ReadBackupMeta`。文件不负责完整业务编排；其中一些外部边界仍是迁移期实现，尤其 `NewMgr` 返回 `MemMgr`、`GetStorage` 返回新的 `MemStorage`、`LogArguments` 为空操作，不能视为 Go 版真实集群和对象存储能力的等价实现。

## 核心职责

- 统一声明 BR 公共 flag 名、默认值、隐藏项和弃用项，入口是 `DefineCommonFlags`、`DefineDatabaseFlags`、`DefineTableFlags`、`DefineFilterFlags` 与 `HiddenFlagsForStream`。
- 用 `Config` 聚合存储、PD、TLS、限速、过滤、并发、加密、keyspace、操作标识和元数据批量大小，并在 `Config::ParseFromFlags` 中完成解析及交叉校验。
- 用 `TLSConfig` 将 CA/证书/私钥路径转换为 PD、KV 和元服务所需的安全配置。
- 解析数据密钥与 master key，执行二选一、算法和密钥长度校验；敏感 flag 通过 `flagToZapField` 脱敏。
- 为任务编排提供管理器、存储、backupmeta、keepalive、控制台输出、进度文件和 namespaced etcd 客户端等公共边界。
- 保留 Go `br/pkg/task/common.go` 的命名与多数配置语义，同时明确当前 Rust 移植尚未覆盖的真实 IO、解密/兼容性检查和持续进度循环。

## 主要符号

- `FullBackupType(String)`：`Valid` 只接受 `FullBackupTypeKV`（`kv`）和 `FullBackupTypeEBS`（`aws-ebs`）。
- `TLSConfig { CA, Cert, Key }`：`IsEnabled` 仅以 CA 是否为空判断 TLS；`ToTLSConfig` 在启用时检查非空路径存在；`ToPDSecurityOption`、`ToKVSecurity` 和 `ParseFromFlags` 负责不同消费端的形态转换。
- `Config`：文件的核心状态对象。重要字段包括 `BackendOptions`、`Storage`、`PD`、`TLS`、`RateLimit`、三类并发、过滤状态、`OperationContext`、两套 `CipherInfo`、`MasterKeyConfig`、`KeyspaceName` 和 `MetadataDownloadBatchSize`。
- `Config::EnsureOperationContext`：保证 `OperationID` 与 `StartedAt` 成对存在；两者都为空时调用 `NewOperationContext` 创建快照，只有一项时失败。
- `Config::ParseFromFlags`：公共解析主入口；处理限速溢出、过滤器优先级、正周期、PD 非空、TLS/PD scheme、一组密钥配置和可选 keyspace。
- `Config::parseCipherInfo`、`parseLogBackupCipherInfo`、`parseAndValidateMasterKeyInfo`：分别解析全量数据密钥、日志数据密钥和逗号分隔的 master-key URL。后者禁止日志明文数据密钥与 master-key 同时配置。
- `GetCipherKeyContent`、`checkCipherKeyMatch`：执行 key/key-file 二选一、hex 解码以及 AES-128/192/256 的 16/24/32 字节精确长度检查。
- `NewMgr`、`GetStorage`：保持任务层工厂签名，但当前构造 `MemMgr`/`MemStorage`；空 PD 和 TLS 文件错误仍会传播。
- `ReadBackupMeta`：从调用者提供的 `Storage` 读取元数据；非明文配置按 `CrypterIvLen` 跳过前缀，然后把剩余内容按 JSON 解码为 Rust 桩 `BackupMeta`。
- `dialEtcdWithCfg`、`dialEtcdWithCfgAndFactory`：把 TLS、keepalive、keyspace 与 PD 地址转换给 `astersql_metaservice::DialEtcdClient`；后者暴露可注入 `PdClientFactory` 供元服务分组测试。
- `flagToZapField`：存储 URL 删除 query，明文密钥、Azure encryption key 和 master-key 输出 `<redacted>`。
- `progressFileWriterRoutine`：按单次调用的当前进度写两位小数百分比；取消、总量无效或完成时删除文件。

## 执行流程

1. 命令构造阶段调用 `DefineCommonFlags` 注册公共默认值；数据库、表或通用过滤场景再叠加相应 `Define*Flags`。流任务调用 `HiddenFlagsForStream` 隐藏不适用项。
2. 具体任务的配置类型调用 `Config::ParseFromFlags`。该函数先读取存储、凭据、checksum 与并发，再检查 `rateLimit * rateLimitUnit` 是否会溢出 `u64`。
3. 过滤解析按“显式 `--filter` > `--db`/`--table` > `*.*`”选择路径；db/table 会通过 `EncloseName`/`EncloseDBAndTable` 记录，未开启大小写敏感时用 `CaseInsensitive` 包装过滤器。
4. 解析 switch-mode、keepalive、后端、TLS 与 PD；switch-mode 为零或 PD 列表为空立即失败。`normalizePDURLs` 随后剥离 `http(s)://`，并拒绝 scheme 与 TLS 开关矛盾的组合。
5. `parseCipherInfo` 解析全量备份算法与密钥；`parseLogBackupCipherInfo` 解析日志密钥；`parseAndValidateMasterKeyInfo` 验证包装算法和各 master-key URL。最后读取元数据批大小及按需注册的 keyspace。
6. 上层任务取用配置：例如 `backup_txn.rs`、`backup_raw.rs`、`restore.rs`、`restore_raw.rs`、`restore_txn.rs` 和 `stream.rs` 调用 `NewMgr`/`GetKeepalive`；恢复路径调用 `ReadBackupMeta`；`backup_ebs.rs` 与 `restore_ebs_meta.rs` 调用进度文件写入函数。
7. 元服务场景由 `dialEtcdWithCfg` 构造 `PdSecurity` 与 `EtcdDialConfig`，再按 `KeyspaceName`、PD 列表和可选工厂创建 namespaced etcd 客户端。

## 数据与状态

`Config` 是可变、可克隆的任务配置快照，而非全局单例。`Default` 只填 Rust 零值与少量布尔值；真正的 CLI 默认配置由 `DefaultConfig` 创建 `FlagSet`、调用 `DefineCommonFlags` 后再走完整解析获得。因此直接构造 `Config::default()` 的 keepalive、checksum 并发和元数据批大小可能仍为零，嵌入式调用者可通过内部 `adjust` 补齐。

`Schemas` 与 `Tables` 是解析时清空重建的 `HashSet`；`FilterStr` 保留原始/生成的过滤表达式，`TableFilter` 保存可执行过滤器，`ExplicitFilter` 单独记录用户是否真的修改了 filter flag。`UserFiltered` 只观察 schema/table/filter 字符串是否非空。

`CipherInfo` 与 `LogBackupCipherInfo` 持有明文数据密钥字节，`MasterKeyConfig` 持有包装算法和有序 master-key 列表；日志输出必须经过 `flagToZapField`，不得直接格式化这些输入。`OperationContext` 的 ID、开始时间和 `restore_id` hint 用于跨阶段标识同一操作。

`NewMgr` 和 `GetStorage` 每次创建新的 `Arc<dyn Mgr>`/`Arc<dyn Storage>`，当前后端是进程内内存对象；`dialEtcdWithCfg` 返回真实 `NamespacedEtcdClient`，其连接资源生命周期由返回对象持有。

## 依赖与调用关系

上游入口包括：

- `br/cmd/br/cmd.rs` 将任务层 `DefineCommonFlags`、`LogArguments`、`ParseTLSTripleFromFlags` 接到 BR 命令层；`br/cmd/br/main.rs` 在根命令初始化时调用命令层入口。
- `br/cmd/br/debug.rs` 直接使用 `Config::ParseFromFlags`、`GetStorage`、`ReadBackupMeta`、`NewMgr` 和 `GetKeepalive`。
- `br/pkg/task/backup*.rs`、`restore*.rs` 与 `stream.rs` 解析公共配置并调用管理器/存储辅助函数；`restore.rs`、`restore_raw.rs`、`restore_txn.rs` 直接读取 backupmeta。
- `backup_ebs.rs` 和 `restore_ebs_meta.rs` 使用 `progressFileWriterRoutine` 输出 operator 可见进度。

主要下游依赖包括：`crate::stubs` 提供 `FlagSet`、错误类型、过滤器、存储/管理器 trait 与内存实现；`crate::encryption::validateAndParseMasterKeyString` 解析 master-key URL；`url` 用于日志 URL 脱敏；`hex` 用于数据密钥解码；`serde_json` 用于当前 Rust `BackupMeta` 解码；`astersql-metaservice` 提供 keyspace 感知的 etcd 拨号。`Cargo.toml` 中的 BR 子 crate 依赖说明该模块位于任务层聚合 crate，而不是通用基础库。

RustCodeGraph `status` 显示索引覆盖本文件，并报告它被 10 个文件使用；`query` 能区分 `common.rs::NewMgr`、`common.rs::ReadBackupMeta` 等 Rust 符号与同名 Go 符号。精确 callers/callees 命令对传入符号 ID 出现名称歧义，因此具体调用点以索引结果和上述仓库引用搜索共同核验。

## 错误处理与边界

- `ParseFromFlags` 对限速乘法溢出、空 db/table、零 switch-mode、空 PD、非法过滤器、非法 cipher、密钥冲突和 TLS/PD scheme 冲突返回 `ErrInvalidArgument` 语义错误；各 flag 读取错误使用 `?` 原样传播。
- `TLSConfig::ToTLSConfig` 只检查配置中非空路径是否存在，并返回一个启用标记；它没有在本文件内加载或解析证书内容。
- `GetCipherKeyContent` 只删除文件末尾一个 LF，保留 CR 和其他空白以对齐 Go `hex.DecodeString` 行为；读取失败加注 `failed to read cipher file`，非 hex 统一使用 `cipherKeyNonHexErrorMsg`。
- `parseLogBackupCipherInfo` 在有效日志加密时调用 `checkCipherKeyMatch(&self.CipherInfo)`，即校验全量密钥而非日志密钥；源码注释明确这是保留 Go 的异常行为，修改前必须先更新对照测试和兼容性判断。
- `ReadBackupMeta` 对读取错误统一加注 `load backupmeta failed`。短于 IV 长度的密文不会 panic，而是取得空/短前缀后继续；当前实现不执行实际解密，仅跳过 IV 并解析 JSON，解析失败提示 wrong AES cipher。
- `progressFileWriterRoutine` 忽略删除和写文件错误；这是探针辅助输出，不会让主任务失败。`DefaultConfig` 则把默认 flag 解析视为不变量，失败时会 panic。

## 并发与资源生命周期

本文件没有共享可变全局状态或内部锁。`Config`、`TLSConfig` 与 `FlagSet` 由调用者独占可变借用；管理器与存储通过 `Arc<dyn ...>` 共享，其具体并发保证由 trait 实现负责。

`progressFileWriterRoutine` 与 Go 名称相同，但 Rust 函数本身不创建线程、不循环也不等待：调用者每次传入进度快照和 `cancelled` 状态，函数写一次或删除一次。若需要持续更新，调度、周期和取消传播必须由上层负责；多个写者指向同一路径时本文件没有串行化或原子替换保证。

`dialEtcdWithCfg` 是同步构造边界，返回的 `NamespacedEtcdClient` 管理实际连接；keepalive 时间来自 `Config`。`NewMgr` 当前只是构造内存对象，没有 Go `conn.NewMgr` 的真实 PD/TiKV 建连、版本检查、domain 初始化与后台资源，因此也没有对应关闭流程。

## 与 Go 版本的对应关系

直接对照文件是 `br/pkg/task/common.go`，测试对照为 `br/pkg/task/common_test.go` 与 `br/pkg/task/common_test.rs`。Rust 已保持 flag 名/default、过滤优先级、限速溢出、PD URL/TLS 互斥、operation context、密钥二选一与长度、master-key URL、日志脱敏等主要配置语义。

关键差异必须保留在架构认知中：

- Go `NewMgr` 调用 `conn.NewMgr` 建立真实管理器；Rust 返回 `MemMgr`。
- Go `GetStorage` 经 `objstore.New` 创建真实对象存储；Rust 总是创建空 `MemStorage`。
- Go `ReadBackupMeta` 自行创建存储，支持历史 GCS prefix 回退、真实解密、protobuf 反序列化和 backupmeta 兼容性检查；Rust 要求调用者传入存储，只识别/注解缺失错误、跳过 IV，并用 JSON 解码，未实现 GCS 回退与兼容性检查。
- Go `LogArguments` 遍历已设置 flag 并写日志；Rust 当前为空操作，只有 `flagToZapField` 有真实脱敏逻辑。
- Go `progressFileWriterRoutine` 每 500ms 循环直到完成/取消，并在退出时删除文件；Rust 是单次快照写入函数，以显式 `cancelled` 参数代替 context。
- Go `TLSConfig::ToTLSConfig` 生成真实 `tls.Config`；Rust 仅验证路径存在并返回 `TLSConfigInner { enabled }`。元服务拨号则由 Rust 的 `astersql-metaservice` 路径提供实际 TLS 转换。

这些差异说明该文件是“配置语义较完整、部分运行时边界仍为桩”的移植状态，不能仅凭函数签名判断与 Go 等价。

## 扩展指南

- 新增公共 CLI 选项时，应同时更新 flag 常量、`DefineCommonFlags`、`Config` 字段/默认值、`ParseFromFlags`、敏感值脱敏规则，并在独立的 `br/pkg/task/common_test.rs` 增加边界测试；若 Go 仍是行为基准，也同步核对 `common.go`/`common_test.go`。
- 修改过滤行为时优先集中在 `Config::ParseFromFlags`，保持显式 filter、db/table 和默认全匹配的优先级，并检查 `Schemas`、`Tables`、`FilterStr`、`TableFilter`、`ExplicitFilter` 的一致性。
- 添加 cipher 或 master-key 算法时需同步 `parseCipherType`、`checkCipherKeyMatch`、有效算法判定、密钥长度、日志脱敏与多云 URL 测试，避免密钥进入错误或日志文本。
- 将迁移桩升级为生产实现时，最可能修改 `NewMgr`、`GetStorage`、`ReadBackupMeta`、`LogArguments` 与 `progressFileWriterRoutine`；必须逐项恢复 Go 中的连接/关闭、GCS 回退、解密、protobuf/兼容性校验、日志和取消语义，而不是仅替换返回类型。
- 修改元服务连接时通过 `dialEtcdWithCfgAndFactory` 保留工厂注入点，并同步 `br/pkg/task/meta_service_group_test.rs`；修改 EBS 进度语义时同步 `backup_ebs.rs`、`restore_ebs_meta.rs` 及其独立测试。
- Rust 单元测试继续放在 `common_test.rs`、`parity_test.rs` 或对应调用模块测试中，不要内嵌回 `common.rs`；本文件底部现有 `meta_service_group_test.rs` 条件模块只是独立测试文件的挂载点。

## 验证依据

- 源码与模块边界：`br/pkg/task/common.rs`、`br/pkg/task/lib.rs`、`br/pkg/task/Cargo.toml`。
- Rust 上游调用点：`br/cmd/br/main.rs`、`br/cmd/br/cmd.rs`、`br/cmd/br/debug.rs`、`br/pkg/task/backup.rs`、`backup_raw.rs`、`backup_txn.rs`、`restore.rs`、`restore_raw.rs`、`restore_txn.rs`、`stream.rs`、`backup_ebs.rs`、`restore_ebs_meta.rs`。
- Rust 独立测试：`br/pkg/task/common_test.rs`（operation context、PD URL、cipher、默认配置、master-key）、`br/pkg/task/parity_test.rs`（脱敏、管理器与配置边界）、`br/pkg/task/meta_service_group_test.rs`（元服务分组/工厂路径）、`br/pkg/task/stream_test.rs`（`storageOpts`）。
- Go 对照：`br/pkg/task/common.go`、`br/pkg/task/common_test.go`；重点核对 `Config.ParseFromFlags`、`NewMgr`、`GetStorage`、`ReadBackupMeta`、`flagToZapField`、`normalizePDURL` 和 `progressFileWriterRoutine`。
- RustCodeGraph：运行 `status` 确认索引含 7032 个 Rust 文件；运行 `files --filter br/pkg/task` 确认目标及相邻文件；运行 `node --file br/pkg/task/common.rs --offset 1 --limit 1200` 阅读完整 960 行源码；运行 `explore`/`query` 定位主要符号及同名 Go/Rust 定义。调用边歧义处另以 `rg` 精确搜索 Rust 引用进行复核。
- 交付验证：按任务要求检查目标文件存在，且固定十一个二级标题各出现一次；本任务只新增说明文档，不运行 Cargo。
