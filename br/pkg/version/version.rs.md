# `br/pkg/version/version.rs`

## 文件定位

本文件是 Cargo crate `astersql-br-pkg-version` 的核心实现，源码由同目录的 `lib.rs` 通过 `#[path = "version.rs"] pub mod version` 装配并整体再导出。crate 的 `Cargo.toml` 将其标为对应 Go 包 `br/pkg/version` 的 library port，直接依赖 `astersql-br-pkg-errors`、`astersql-br-pkg-version-build`、`astersql-errors`、`regex` 与 `semver`。

它位于 BR 建立集群连接和执行备份恢复动作之前的兼容性门禁层。当前 Rust 生产接线中，`br/pkg/conn/conn.rs::NewMgr` 通过 `VersionPdAdapter` 调用 `CheckClusterVersion`：普通 BR 使用 `CheckVersionForBR`，流式/PiTR 使用 `CheckVersionForBRPiTR`，需要 Domain 时再使用 `CheckVersionForDDL`。文件还提供数据库服务器版本识别、备份元数据版本规范化和通用半开版本区间检查。

本文件没有直接依赖真实 PD protobuf、PD client 或 SQL driver；`Store`、`PdClient`、`QueryExecutor` 是为迁移边界保留的最小本地抽象。因而它实现兼容性策略和编排，但网络请求、PD 适配及数据库连接生命周期由调用方负责。

## 核心职责

1. 规范化 `vX.Y.Z-N-gHASH-dirty` 一类发布串，并用 `semver::Version` 做稳定比较（`removeVAndHash`、`NextMajorVersion`）。
2. 遍历 PD 返回的存储节点，将 TiFlash 与普通 TiKV 分流，并把普通 TiKV 交给可注入检查器（`CheckClusterVersion`、`check_tiflash_version`、`VerChecker`）。
3. 实现 BR、PiTR、DDL、Keyspace BR 和备份恢复的版本门槛（`CheckVersionForBR*`、`CheckVersionForDDL`、`CheckVersionForKeyspaceBR`、`CheckVersionForBackup`）。
4. 记录 checkpoint 与 PiTR batch-KV 能力探测结果，供后续流程查询（`CheckCheckpointSupport`、`CheckPITRSupportBatchKVFiles`）。
5. 从 `version()` / `tidb_version()` 文本中判断 MySQL、MariaDB、TiDB 或未知服务，解析 Classic/Cloud TiDB 版本，并实现 SQL 查询回退（`ParseServerInfo`、`FetchVersion`）。
6. 维持与 Go `br/pkg/version/version.go` 的版本阈值、错误语义、查询顺序和测试契约一致。

## 主要符号

- `CURRENT_BACKUP_SUPPORT_TABLE_INFO_VERSION: u16 = 5`：BR 当前支持的 TableInfo 版本上限；`version_test.rs::test_ensure_support_version` 还会读取 `pkg/meta/model/table.rs`，交叉核对 `TableInfoVersion5` 和最新别名。
- `StoreLabel`、`Store`：`metapb.StoreLabel` / `metapb.Store` 的最小替身，只保留引擎识别、地址和版本检查所需字段。`Store::{GetId, GetAddress, GetPeerAddress, GetVersion}` 保持 Go getter 风格。
- `PdClient: Send + Sync`：只要求 `GetAllStores(exclude_tombstone)`；`CheckClusterVersion` 固定传 `true`。`QueryExecutor: Send + Sync`：只要求按 SQL 返回单行字符串。
- `VerChecker`：线程安全的共享闭包类型 `Arc<dyn Fn(&Store, &Version) -> Result<...> + Send + Sync>`；具名函数也能以同等签名传入 `CheckClusterVersion`。
- `removeVAndHash`：依次去除 Git describe 的 `-N-gHASH`、尾部 `-dirty` 和开头 `v`。顺序是契约的一部分，例如 `v3.0.0-beta-211-g...-dirty` 最终为 `3.0.0-beta`。
- `NextMajorVersion`：正常版本清除 pre/build 并进位 major；不可解析的构建版本返回 `i64::MAX as u64.0.0-nightly`，表示“无限新”的 nightly。
- `CheckClusterVersion`：集群级编排入口；TiFlash 只做其专用 3.x/4.x 下限检查，不再执行传入的普通 TiKV checker。
- `CheckVersionForBR`：最低 TiKV 为 `3.1.0-beta.2`；BR major 不得小于 TiKV major，领先不得超过 2；另处理 3.1.0 与 4.0.0-rc.1 的历史不兼容边界，并写入 checkpoint 能力缓存。
- `CheckVersionForBRPiTR`：要求 TiKV 至少 6.1；BR 6.1 只配 TiKV 6.1，其余较新 BR 不接受 TiKV 6.1；TiKV 6.5 起设置 batch-KV 支持。
- `CheckVersionForDDL` / `CheckVersionForKeyspaceBR`：阈值分别为 `6.2.0-alpha` 与 `6.6.0-alpha`。
- `CheckVersionForBackup`：返回捕获备份版本的 checker；备份 major 比目标集群高超过 1 才拒绝。
- `CheckVersion`：检查 `[required_min, required_max)`；下界比较完整 semver，上界故意只比较 major，使触及上界 major 的 beta 版本也被视为过新。
- `ExtractTiDBVersion` / `NormalizeBackupVersion`：前者按 TiDB `version()` 的段数抽取版本，后者按 Go `strconv.Unquote` 的相关语义解引号、去空白并尝试解析，失败返回 `None`。
- `ServerType`、`ServerInfo`、`ParseServerInfo`：识别服务器类型并解析版本；解析失败统一回落 `0.0.0`，`HasTiKV` 只是 Go 结构对齐字段，本文件不设置它。
- `FetchVersion`：先执行 `SELECT tidb_version();`，仅在返回内容含合法 Release Version / CLOUD 行时采用；否则执行 `SELECT version();`。
- `SetReleaseVersionForTest` 与测试专用 `SetPITRSupportBatchKVFilesForTest`：前者使用线程本地覆盖构建版本，后者仅在 `cfg(test)` 下重置共享能力标志。

## 执行流程

集群兼容检查主流程如下：

1. `br/pkg/conn/conn.rs::NewMgr` 构造 `VersionPdAdapter`，根据 `VersionCheckerType` 选择 BR 或 PiTR checker；需要 Domain 时另选 DDL checker。
2. `CheckClusterVersion` 调用 `PdClient::GetAllStores(true)`，PD 错误直接向上传播。
3. 每个 Store 先由 `is_tiflash` 检查 `engine=tiflash` 或 `engine=tiflash_compute`。TiFlash 版本经 `removeVAndHash` 后解析，仅对 major 3/4 检查各自最低版本，然后跳过普通 checker。
4. 普通 Store 的版本同样先净化再解析；解析失败生成带地址和净化后版本串的 `ErrVersionMismatch`。
5. 解析成功后调用传入 checker。任何节点失败都会立即终止遍历；全部通过才返回 `Ok(())`。

BR/PiTR checker 都先读取 `effective_release_version`。测试特殊值 `build::ReleaseVersionForTest` 会短路检查；否则先解析 BR 自身版本，再按各自阈值检查。`CheckVersionForBR` 在主要兼容条件通过后更新 checkpoint 缓存，`CheckVersionForBRPiTR` 在最低版本检查通过后更新 batch-KV 标志，再检查 BR/TiKV 6.1 配对规则。

服务器版本探测分为获取和解析两层：`FetchVersion` 负责 `tidb_version()` → `version()` 的查询策略；`ParseServerInfo` 先按 Release Version、TiDB、MariaDB、普通数字版本的顺序分类，再抽取 Classic 版本或把 `CLOUD.YYYYMM.patch` 映射为 `(year-2000).month.patch`。Cloud 年份必须在 2025..=2099、月份在 1..=12，否则最终得到 `0.0.0`。

## 数据与状态

- 版本阈值由小函数即时构造 `Version`；正则表达式通过 `OnceLock<Regex>` 惰性初始化并在进程内复用。
- `checkpoint_support_error` 是 `OnceLock<Mutex<Option<SharedError>>>`。`None` 表示支持；`CheckVersionForBR` 对每个成功走到该阶段的 TiKV 先清空，再在版本低于 6.5 时写入错误。
- `pitr_support_batch_kv_files` 是 `OnceLock<Mutex<bool>>`，初始为 `false`；每个通过 PiTR 最低版本检查的 TiKV 都会按是否达到 6.5 覆盖它。
- 因上述两个能力值按节点覆盖，完整成功遍历后反映的是最后一个相关普通 TiKV 节点，而不是显式的“所有节点 AND”。若遍历在写入前因 PD、解析或早期版本错误返回，已有缓存值也可能保留。新增消费者不能把它们误认为与某次检查绑定的不可变快照。
- `RELEASE_OVERRIDE` 是线程本地 `RefCell<Option<String>>`，避免并行 Rust 测试互相覆盖；生产路径没有覆盖时读取 `build::ReleaseVersion()`。
- `ServerInfo::ServerVersion` 类型虽为 `Option<Version>`，但 `ParseServerInfo` 当前总会写入 `Some`，失败为 `Some(0.0.0)`；默认构造本身仍是 `None`。

## 依赖与调用关系

RustCodeGraph 的直接调用边包括：

- `br/pkg/conn/conn.rs::NewMgr → CheckClusterVersion`，并注入 `CheckVersionForBR`、`CheckVersionForBRPiTR` 或 `CheckVersionForDDL`。
- `CheckClusterVersion → PdClient::GetAllStores / is_tiflash / check_tiflash_version / removeVAndHash / checker`。
- `CheckVersionForBR → min_tikv_version / incompatible_tikv_major3 / incompatible_tikv_major4 / checkpoint_support_error / effective_release_version / git_branch`。
- `CheckVersionForBRPiTR → effective_release_version / removeVAndHash / pitr_support_batch_kv_files`。
- `CheckTiDBVersion → ParseServerInfo → CheckVersion`；`FetchVersion → QueryExecutor::QueryRow`。

crate 级依赖职责为：`semver` 提供解析和顺序；`regex` 处理 Git hash、服务端版本和 Cloud 格式；`astersql-br-pkg-version-build` 提供 BR release/branch；`astersql-br-pkg-errors::ErrVersionMismatch` 与 `astersql-errors` 提供共享错误、构造和注解。

`CheckVersionForKeyspaceBR`、备份 checker、服务器解析/查询和能力查询均通过 `lib.rs` 公开再导出；本次索引中它们的主要直接 Rust 使用证据来自同 crate 的 `version_test.rs`、`parity_test.rs`，不能据此声称所有 Go 生产调用点都已迁移为 Rust 接线。

## 错误处理与边界

- `mismatch` 用 `ErrVersionMismatch` 作为根错误并附加上下文，适用于多数版本格式和兼容性失败；DDL/Keyspace、非 TiDB 和缺失版本分支使用普通 `astersql_errors::New`，与 Go 对照保持现有差异边界。
- PD 获取错误和 checker 错误原样向上传播；`FetchVersion` 会忽略首个查询的失败或不规范成功结果，只有回退查询失败才返回带 `sql: SELECT version();` 注解的错误。
- `ExtractTiDBVersion` 只接受拆分后 3/4/5/6 段的已知 Git describe 形态；结构不合法走版本不匹配错误，核心 semver 不合法则返回解析错误。
- `NormalizeBackupVersion` 的解引号实现支持 Go 风格双引号转义、反引号 raw string、十六进制/Unicode/八进制；解引号失败时并不报错，而是回退到原文解析，最终以 `None` 表示失败。
- TiFlash 错误使用 `PeerAddress`，普通 TiKV 错误使用 `Address`；`version_test.rs::test_tiflash_error_uses_peer_address` 锁定了这一差异。
- 固定阈值和静态正则均以源码常量创建，内部 `unwrap` 只会在开发者写入非法字面量时触发。共享 Mutex 若发生 poison，后续 `.lock().unwrap()` 会 panic；当前实现没有恢复策略。
- Rust 版本只保留 Go 的 `git_branch()` 调用位点，没有实现 Go 在非 master 且 TiKV 新于 BR 时的告警日志；这是当前已知迁移差异，不能描述为已有告警行为。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、事务或网络连接。`PdClient` / `QueryExecutor` 的外部资源由调用方持有；函数只在调用栈内借用 trait object。`CheckVersionForBackup` 返回的 `Arc` checker 拥有捕获的 `Version`，因此可以跨线程共享。

线程安全边界由 trait 的 `Send + Sync`、`VerChecker` 的 `Send + Sync` 和两个全局 `Mutex` 保证。正则表达式只初始化一次。测试发布版本覆盖是线程局部的，作用域清理由 `version_test.rs::ReleaseVersionGuard::drop` 或测试末尾显式调用完成；它不会跨测试线程传播。

两个全局能力缓存的生命周期是整个进程，并无 generation、集群 ID 或检查会话标识。并发对不同集群执行检查不会产生数据竞争，但可能发生逻辑上的最后写入者覆盖；如未来允许并发管理多个集群，应优先把能力结果改为每次检查的返回值或绑定到 manager，而不是继续扩大全局状态。

## 与 Go 版本的对应关系

直接对照文件是 `br/pkg/version/version.go`，独立测试为 `br/pkg/version/version_test.go`。Rust 保留了 Go 的主要策略：相同版本阈值、排除 tombstone、TiFlash 分流、BR/PiTR/backup/DDL/Keyspace 判断、checkpoint 和 batch-KV 副作用、版本字符串净化、SQL 回退顺序、ServerType 数值顺序及 Cloud 年月映射。

为避开完整 grpcio/protobuf/dbutil 依赖，Rust 用本地 `Store`、`PdClient`、`QueryExecutor` 替代 Go 的 `metapb.Store`、`pd.Client` 和 `dbutil.QueryExecutor`；context、PD option、SQL row scan 与日志由适配层或调用方处理。Rust 使用 `semver` crate，Go 使用 `coreos/go-semver`，相关边界由两侧同名测试矩阵校准。

明确差异包括：Rust 测试版本覆盖是线程本地而非修改 Go 包全局变量；全局能力值用 Mutex 包装；Rust 的 `ParseServerInfo` 用 `Option<Version>` 表达字段但实际解析仍回落 `0.0.0`；Go 会记录解析/查询回退及 BR 过旧告警，Rust 当前没有等价日志；Rust `FetchVersion` 接口不携带 context。

Rust `version_test.rs` 对齐 Go 的 `TestCheckClusterVersion`、版本比较、NextMajor、TiDB 抽取、区间检查、备份版本规范化、服务识别、查询回退和 TableInfo 常量测试；`parity_test.rs::go_rust_public_contract_matches` 另做公开契约冒烟。Rust 还增加了 TiFlash peer address、转义换行和线程隔离等迁移边界覆盖。

## 扩展指南

- 新增集群兼容策略时，优先实现符合 checker 签名的独立函数，并由真实入口注入 `CheckClusterVersion`；同时扩展 `version_test.rs` 表驱动矩阵及 Go 对照测试，不要把策略硬编码进遍历器。
- 修改 TiKV/TiFlash、DDL、Keyspace、PiTR 或 checkpoint 阈值时，必须同步审查 `version.go`、错误文案、`version_test.rs::test_check_cluster_version`、Go `TestCheckClusterVersion` 和 parity 冒烟；TableInfo 上限变化还要核对 `pkg/meta/model/table.rs`。
- 支持新的版本文本格式时，分别判断它影响 `FetchVersion` 的接受正则、`ParseServerInfo` 的分类/抽取正则还是 `ExtractTiDBVersion` 的 Git describe 分段；新增正常、非法、边界年份/月和回退查询测试。
- 扩充 `Store` / `PdClient` / `QueryExecutor` 前先确认是版本策略不可缺少的字段；真实网络、context、日志和重试宜保留在适配层，避免本 crate 重新引入重依赖。
- 若修正能力聚合语义，应设计为显式的“所有节点支持”归约并消除陈旧缓存，验证多节点异构版本、早退、重复检查和并发集群；这是行为变更，不能只改文档或测试。
- 单元测试继续放在独立的 `version_test.rs` / `parity_test.rs`，不要内嵌到生产源文件；新增公开 API 还应核对 `lib.rs` 再导出和 Cargo/BUILD 装配。
- 性能风险主要来自每次检查重新解析版本和遍历全部 Store，当前阈值函数开销很小；不要以缓存版本对象为由引入难以失效的跨集群状态。兼容风险则集中在 semver pre-release 顺序、上界只比较 major、Cloud 年份映射和错误类型/文案。

## 验证依据

- 源码与模块边界：`br/pkg/version/version.rs`（774 行）、`br/pkg/version/lib.rs`、`br/pkg/version/Cargo.toml`。
- 直接生产接线：`br/pkg/conn/conn.rs::NewMgr` 中普通 BR、Stream/PiTR 和 Domain/DDL 三条 `CheckClusterVersion` 路径。
- Go 对照：`br/pkg/version/version.go`，包括常量、全部 checker、服务器解析与 `FetchVersion`。
- Rust 独立测试：`br/pkg/version/version_test.rs` 的 `test_check_cluster_version`、`test_tiflash_error_uses_peer_address`、`test_compare_version`、`test_next_major_version`、`test_extract_tidb_version`、`test_check_version`、`test_normalize_backup_version*`、`test_detect_server_info`、`test_fetch_version*`、`test_ensure_support_version`；`br/pkg/version/parity_test.rs::go_rust_public_contract_matches`。
- Go 独立测试：`br/pkg/version/version_test.go` 中对应的 `TestCheckClusterVersion`、`TestCompareVersion`、`TestNextMajorVersion`、`TestExtractTiDBVersion`、`TestCheckVersion`、`TestNormalizeBackupVersion`、`TestDetectServerInfo`、`TestFetchVersion*` 与 `TestEnsureSupportVersion`。
- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/pkg/version` 确认模块/测试集合；`explore "br/pkg/version/version.rs version compatibility symbols callers callees"` 给出 `NewMgr → CheckClusterVersion`、`CheckTiDBVersion → CheckVersion` 及主要内部调用边；`node --file` 核验了 Rust 与 Go 全文件实现。
- 本任务是纯文档分析，未运行 Cargo 或运行时代码测试；交付验证只检查固定章节、文件存在性、变更范围和人工事实一致性。
