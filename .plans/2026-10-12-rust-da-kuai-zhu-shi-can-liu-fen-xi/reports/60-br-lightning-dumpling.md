# BR、Lightning 与 Dumpling 大块注释残留审计

## 范围与结论

- 输入：`00-candidates.tsv` 中 `br/`、`lightning/`、`dumpling/` 前缀的全部候选。
- 覆盖：BR 36 块、Lightning 20 块、Dumpling 0 块，共 56 块；同一路径、起止行组合均只出现一次。
- 分类：合法说明/示例误报 34 块；部分实现且注释仍描述缺口 16 块；非执行测试档案 2 块；扫描器词法误报 4 块；完全未实现占位与已实现重复代码档案均为 0 块。
- 总体判断：没有发现被注释掉、可直接恢复执行的 Go/Rust 代码。高风险项来自可执行 Rust 周围明确声明的本地 stand-in，而不是注释本身；它们在真实 PD/TiKV/gRPC、SQL 持久化、TLS、对象存储和取消/重试边界上不能由内存测试替代。
- Dumpling：候选全集在该前缀下为空；这只证明本版启发式没有命中大块注释/原始字符串，不证明 Dumpling 的实现完整。

统一字段为：来源意图、Rust 对应实现、缺失行为、测试证据、风险、建议。行号来自候选快照，仅用于和 `00-candidates.tsv` 机械核对。

## BR

候选：`br/pkg/backup/schema_test.rs:16-46`

- 分类：合法说明/示例误报。来源意图：索引 Go `schema_test.go` 的备份 schema 场景；Rust 对应实现：其后 4 个可执行测试；缺失行为：本块无，注释明确说明 MemMeta/checksum 注入边界；测试证据：缺库、全量 policy、stats 与系统表改名测试；风险：内存夹具不证明真实 SQL/元数据交互；建议：保留说明，真实边界另建集成验证。

候选：`br/pkg/checkpoint/checkpoint.rs:16-111`

- 分类：部分实现且注释仍描述缺口。来源意图：说明 Go checkpoint runner 的聚合、刷盘、锁与加密；Rust 对应实现：线程、通道、storage trait 与 checksum/lock 路径均为可执行代码；缺失行为：注释明确称生产路径尚未从桩切到真实客户端；测试证据：同目录 17 个 checkpoint 测试覆盖 flush、retry、lock、remove；风险：真实对象存储原子性、取消时刷盘和锁续租失败仍可能漂移；建议：保留为缺口说明，后续以真实 storage/RPC 集成任务处理。

候选：`br/pkg/checkpoint/checkpoint_test.rs:16-142`

- 分类：合法说明/示例误报。来源意图：测试矩阵与夹具约束；Rust 对应实现：17 个可执行测试；缺失行为：本块无，且主动声明不覆盖真实 PD/TiKV；测试证据：backup/restore/log-restore、重试、锁冲突与清理；风险：短 tick 和内存存储不能证明外部持久化；建议：保留测试说明。

候选：`br/pkg/conn/conn_test.rs:16-214`

- 分类：合法说明/示例误报。来源意图：中文索引连接、配置探测和地址处理场景；Rust 对应实现：13 个 FakePD/本地 HTTP/failpoint 测试；缺失行为：注释块本身无代码；测试证据：可执行测试覆盖错误与重绑分支；风险：未连接真实 PD/TiKV/gRPC；建议：保留索引，真实连接行为由独立集成测试证明。

候选：`br/pkg/conn/parity_test.rs:1-150`

- 分类：合法说明/示例误报。来源意图：解释 Go/Rust 公共契约；Rust 对应实现：1 个总入口调用分组契约断言；缺失行为：无被归档代码；测试证据：MemPd、store/filter 与错误合同；风险：契约测试使用内存对象；建议：保留说明。

候选：`br/pkg/conn/util/parity_test.rs:3-108`

- 分类：合法说明/示例误报。来源意图：索引连接工具的正常、边界和错误合同；Rust 对应实现：5 个可执行测试；缺失行为：无；测试证据：store 过滤、TLS/地址和错误路径；风险：不覆盖真实网络超时；建议：保留说明。

候选：`br/pkg/conn/util/stubs.rs:3-34`

- 分类：部分实现且注释仍描述缺口。来源意图：说明 kvproto `Store` 等本地替身；Rust 对应实现：字段载体与最小 getter；缺失行为：真实 protobuf/gRPC、连接生命周期和远端错误语义；测试证据：`util/parity_test.rs` 的 5 个内存契约测试；风险：store 状态、TLS 和网络错误可能只保留形状；建议：保留警示，生产接线需独立移植真实依赖。

候选：`br/pkg/conn/util/util.rs:16-214`

- 分类：部分实现且注释仍描述缺口。来源意图：描述 Go 连接 helper 与当前 trait 边界；Rust 对应实现：TS、store 过滤和控制器 helper；缺失行为：依赖注入替代真实 PD/TiKV 客户端；测试证据：相邻 parity 测试覆盖纯逻辑；风险：取消、超时、重试和连接关闭无法由 trait 夹具充分证明；建议：保留边界说明并补真实客户端集成任务。

候选：`br/pkg/encryption/master_key/file_backend_test.rs:1-76`

- 分类：合法说明/示例误报。来源意图：许可证和文件密钥测试说明；Rust 对应实现：2 个可执行测试；缺失行为：无；测试证据：AES-GCM 与认证失败、临时文件清理；风险：低，仅文件系统平台差异；建议：保留。

候选：`br/pkg/encryption/master_key/kms_backend_test.rs:1-88`

- 分类：合法说明/示例误报。来源意图：KMS mock 测试说明；Rust 对应实现：3 个可执行测试；缺失行为：无被注释代码；测试证据：provider 调用、错误与缓存合同；风险：mock 不证明云 KMS 权限、重试和网络；建议：保留并由外部集成覆盖云边界。

候选：`br/pkg/encryption/master_key/master_key.rs:1-36`

- 分类：合法说明/示例误报。来源意图：许可证、vendor 常量和 backend 抽象说明；Rust 对应实现：可执行常量/trait；缺失行为：本块无；测试证据：同目录 file/KMS/multi-backend 测试；风险：低；建议：保留。

候选：`br/pkg/encryption/master_key/multi_master_key_backend_test.rs:1-84`

- 分类：合法说明/示例误报。来源意图：多主密钥回退测试说明；Rust 对应实现：1 个可执行测试与 MockBackend；缺失行为：无；测试证据：回退/错误聚合合同；风险：mock 不证明真实 KMS 延迟与轮换；建议：保留。

候选：`br/pkg/gluetidb/mock/mock.rs:38-103`

- 分类：扫描器词法误报。来源意图：普通 Rust 字符串常量 `"br"` 后的可执行 trait/struct；Rust 对应实现：`WithInternalSourceType`、`RecordSet`、RAII close 和 mock session；缺失行为：候选并非原始字符串或代码档案；测试证据：调用方单测可观察 source 与 close；风险：扫描器把普通字符串跨行延展；建议：不改代码，修订未来扫描器的普通字符串终止识别。

候选：`br/pkg/metautil/metafile.rs:1548-1719`

- 分类：合法说明/示例误报。来源意图：解释 MetaReader/Writer、加密、分片和兼容约束；Rust 对应实现：相邻可执行 metafile 逻辑；缺失行为：无被注释代码；测试证据：仓库中的 metautil 测试及 JSON 夹具覆盖编码兼容；风险：长说明可能漂移，但不是实现占位；建议：保留并在格式变更时同步。

候选：`br/pkg/mock/backend.rs:16-88`

- 分类：合法说明/示例误报。来源意图：说明由 Go mockgen 映射的测试 backend；Rust 对应实现：可执行 MockBackend；缺失行为：作为测试 mock 不要求真实导入；测试证据：调用方通过注入断言调用和错误；风险：不能作为生产 backend 完整性证据；建议：保留。

候选：`br/pkg/registry/parity_test.rs:3-130`

- 分类：合法说明/示例误报。来源意图：公共 registry 合同索引；Rust 对应实现：4 个可执行契约测试；缺失行为：无；测试证据：task row、状态转换、错误与清理；风险：使用内存 session；建议：保留。

候选：`br/pkg/registry/stubs.rs:16-189`

- 分类：部分实现且注释仍描述缺口。来源意图：声明 glue/session/sqlexec/domain 等本地 stand-in；Rust 对应实现：轻量 Error、Context、MemSession 与字段载体；缺失行为：真实 SQL 事务、并发隔离、domain 生命周期和持久化；测试证据：`registry/parity_test.rs` 4 个契约测试；风险：错误恢复和资源关闭仅在内存模型中成立；建议：保留警示并把真实 registry SQL 接线列为后续高优先级。

候选：`br/pkg/restore/data/lib.rs:3-34`

- 分类：部分实现且注释仍描述缺口。来源意图：说明 restore/data 模块重导出及 stubs 边界；Rust 对应实现：模块接线可执行；缺失行为：注释指出部分外部依赖由 stubs 承接；测试证据：该域相邻 data 测试覆盖恢复计划纯逻辑；风险：真实 region、import 与清理链路不能由重导出证明；建议：保留说明，按外部边界拆分集成任务。

候选：`br/pkg/restore/ingestrec/ingest_recorder_test.rs:16-128`

- 分类：合法说明/示例误报。来源意图：ingest recorder 测试矩阵；Rust 对应实现：5 个可执行测试；缺失行为：无；测试证据：记录、去重、SQL/状态与错误分支；风险：测试 session 为内存替身；建议：保留。

候选：`br/pkg/restore/internal/import_client/import_client.rs:16-113`

- 分类：部分实现且注释仍描述缺口。来源意图：描述 ImportSST 客户端缓存和能力探测；Rust 对应实现：trait 化 transport、缓存和 backoff；缺失行为：真实 gRPC transport/连接关闭和远端 capability 交互由边界注入；测试证据：相邻 parity 测试以 MemStoreClient 覆盖合同；风险：网络取消、重连、重试上限与资源释放可能漂移；建议：保留说明并补真实 gRPC 集成。

候选：`br/pkg/restore/internal/import_client/parity_test.rs:5-101`

- 分类：合法说明/示例误报。来源意图：ImportSST 公共合同说明；Rust 对应实现：1 个聚合测试；缺失行为：无被归档代码；测试证据：MemStoreClient 覆盖缓存、探测和错误；风险：未覆盖真实 RPC；建议：保留。

候选：`br/pkg/restore/internal/prealloc_db/db_test.rs:10-238`

- 分类：合法说明/示例误报。来源意图：prealloc DB 长测试的场景索引；Rust 对应实现：7 个可执行测试；缺失行为：无；测试证据：allocator、session、并发/边界与回收；风险：内存 SQL 不证明真实事务冲突；建议：保留。

候选：`br/pkg/restore/internal/prealloc_db/parity_test.rs:5-111`

- 分类：合法说明/示例误报。来源意图：Go/Rust 预分配合同说明；Rust 对应实现：1 个聚合契约测试；缺失行为：无；测试证据：RecSession 记录 SQL 顺序和错误；风险：真实持久化/回滚未覆盖；建议：保留。

候选：`br/pkg/restore/log_client/parity_test.rs:3-59`

- 分类：合法说明/示例误报。来源意图：log restore 公共合同索引；Rust 对应实现：1 个契约测试入口；缺失行为：无；测试证据：正常、边界、错误和清理合同；风险：依赖本地 stubs；建议：保留。

候选：`br/pkg/restore/log_client/stubs.rs:2201-2722`

- 分类：扫描器词法误报。来源意图：普通字符串 `"br"` 后的可执行 KV/domain/PD/import client 等 stand-in；Rust 对应实现：trait、内存状态和错误路径均为代码而非字符串；缺失行为：候选载体识别错误，但文件整体确实缺真实 RPC/存储；测试证据：log-client parity/flow-control 测试覆盖内存合同；风险：扫描误报之外，真实 apply/clear、region scan、取消和 close 仍属高风险外部边界；建议：修扫描器，并另以依赖移植任务处理 stand-in，不能删除此区段。

候选：`br/pkg/restore/misc.rs:16-81`

- 分类：合法说明/示例误报。来源意图：restore helper 与 Go 对照说明；Rust 对应实现：granularity、table/domain helper 等可执行逻辑；缺失行为：无被注释代码；测试证据：调用方与 parity 测试覆盖公共合同；风险：低；建议：保留。

候选：`br/pkg/restore/parity_test.rs:3-51`

- 分类：合法说明/示例误报。来源意图：restore 公共 API 合同说明；Rust 对应实现：2 个可执行测试；缺失行为：无；测试证据：Import/接口边界和错误合同；风险：非端到端恢复；建议：保留。

候选：`br/pkg/restore/split/parity_test.rs:3-48`

- 分类：合法说明/示例误报。来源意图：split 公共合同说明；Rust 对应实现：4 个可执行测试；缺失行为：无；测试证据：range、错误、重试和边界；风险：不证明真实 PD region 变化；建议：保留。

候选：`br/pkg/stream/crr/internal/checkpoint/doc.rs:16-106`

- 分类：合法说明/示例误报。来源意图：对应 package `doc.go`，解释 CRR checkpoint 算法与不变量；Rust 对应实现：该 package 的可执行模块；缺失行为：无；测试证据：相邻 integration test 10 个场景；风险：文档可能随算法变更漂移；建议：保留并与已读取的 `doc.go` 同步维护。

候选：`br/pkg/stream/crr/internal/checkpoint/integration_test.rs:16-57`

- 分类：合法说明/示例误报。来源意图：CRR 集成场景索引；Rust 对应实现：10 个可执行测试；缺失行为：无；测试证据：partial replication、checkpoint 与 restore validation；风险：测试环境并非完整下游集群；建议：保留。

候选：`br/pkg/utils/json_test.rs:107-242`

- 分类：非执行测试档案。来源意图：保存 Go 兼容 BackupMeta JSON 样本；Rust 对应实现：常量被 7 个 JSON 测试消费；缺失行为：不是待执行代码；测试证据：反序列化、字段兼容和加密元数据用例；风险：夹具过旧会漏掉新字段；建议：保留为协议回归数据，格式演进时追加样本而非删除。

候选：`br/pkg/utils/json_test.rs:353-485`

- 分类：非执行测试档案。来源意图：保存 MetaFile JSON 样本；Rust 对应实现：常量被同文件测试消费；缺失行为：不是实现；测试证据：data_files、ddls、backup_ranges 与 raw range 解析；风险：静态样本不覆盖所有版本组合；建议：保留测试档案。

候选：`br/pkg/utils/key_test.rs:4-24`

- 分类：合法说明/示例误报。来源意图：key range 测试说明；Rust 对应实现：7 个可执行测试；缺失行为：无；测试证据：范围比较、合并和边界；风险：低；建议：保留。

候选：`br/pkg/utils/memory_monitor.rs:89-120`

- 分类：扫描器词法误报。来源意图：`to_string()` 等普通字符串调用后的可执行 `RunMemoryMonitor`；Rust 对应实现：配置 provider、日志和后台 alarm 启动；缺失行为：候选不是原始字符串；测试证据：相邻 memory monitor 测试/调用方验证配置；风险：扫描器错误跨行，且后台线程清理仍需单独行为测试；建议：修扫描器，不删除代码。

候选：`br/pkg/utils/register_test.rs:16-54`

- 分类：合法说明/示例误报。来源意图：register/etcd 测试说明；Rust 对应实现：5 个可执行测试；缺失行为：无；测试证据：MemEtcd 覆盖注册、错误与清理；风险：真实租约和网络分区未覆盖；建议：保留。

候选：`br/pkg/utils/stubs.rs:27-61`

- 分类：扫描器词法误报。来源意图：普通 `"br"` 字符串之后的可执行 KV stand-in；Rust 对应实现：Context、Storage、Transaction 和共享 meta transaction；缺失行为：候选并非原始字符串，但 stand-in 不执行真实 KV 提交/重试；测试证据：utils 测试覆盖编码与内存合同；风险：真实事务隔离、取消、冲突重试和持久化未证明；建议：修扫描器，真实 KV 接线另建任务。

## Lightning

候选：`lightning/pkg/checkpoints/checkpoints.rs:200-223`

- 分类：合法说明/示例误报。来源意图：可执行 MySQL checkpoint DDL 模板；Rust 对应实现：字符串常量由 checkpoint SQL 路径格式化执行；缺失行为：不是代码归档；测试证据：checkpoint 与 server checkpoint-control 测试覆盖 schema/backend 路由；风险：SQL 方言、索引长度和事务行为需真实 MySQL/TiDB 验证；建议：保留协议模板。

候选：`lightning/pkg/importinto/checkpoint.rs:16-136`

- 分类：部分实现且注释仍描述缺口。来源意图：说明 noop/file/MySQL checkpoint manager；Rust 对应实现：状态、序列化和 manager trait；缺失行为：注释标明 MySQL/外部边界通过当前抽象或替身承接；测试证据：同目录 6 个 checkpoint 测试；风险：崩溃恢复、原子持久化和并发更新可能只在文件/内存模型成立；建议：保留说明并安排真实 MySQL 恢复测试。

候选：`lightning/pkg/importinto/checkpoint_test.rs:16-149`

- 分类：合法说明/示例误报。来源意图：checkpoint 测试矩阵；Rust 对应实现：6 个可执行测试；缺失行为：无；测试证据：not-found、更新、destroy、file/noop/MySQL 合同；风险：MySQL 多为替身；建议：保留。

候选：`lightning/pkg/importinto/importer.rs:16-70`

- 分类：部分实现且注释仍描述缺口。来源意图：Importer 生命周期、取消超时和清理顺序；Rust 对应实现：可执行 importer orchestration；缺失行为：真实 executor/SQL 服务由接口注入；测试证据：parity 与 server 测试覆盖生命周期合同；风险：60 秒取消、远端 job 遗留和资源释放在真实服务上未证明；建议：保留缺口说明并补真实 IMPORT INTO 集成。

候选：`lightning/pkg/importinto/job_monitor.rs:16-78`

- 分类：部分实现且注释仍描述缺口。来源意图：说明 job poll/状态映射；Rust 对应实现：JobMonitor trait 与轮询逻辑；缺失行为：真实 SQL 状态源和网络取消由边界注入；测试证据：importinto parity/mock 测试覆盖状态合同；风险：轮询间隔、终态竞态和错误恢复；建议：保留并补真实 job 生命周期测试。

候选：`lightning/pkg/importinto/job_orchestrator.rs:16-118`

- 分类：部分实现且注释仍描述缺口。来源意图：说明并发提交与监控编排；Rust 对应实现：可执行 orchestrator 与并发限制；缺失行为：真实服务端 job 的持久化/取消语义未由本地 mock 证明；测试证据：mock/parity 测试覆盖顺序和错误传播；风险：部分提交后失败可能遗留远端任务；建议：保留说明，集成测试必须检查失败清理。

候选：`lightning/pkg/importinto/job_submitter.rs:16-70`

- 分类：部分实现且注释仍描述缺口。来源意图：说明 IMPORT INTO 提交与 job id 获取；Rust 对应实现：ImportJob 和 submitter trait/实现；缺失行为：真实 SQL 执行、重试与幂等由外部注入；测试证据：mock/parity 测试覆盖参数和错误；风险：重复提交与连接中断后的未知结果；建议：保留说明并补幂等/断线集成验证。

候选：`lightning/pkg/importinto/lib.rs:1-133`

- 分类：部分实现且注释仍描述缺口。来源意图：模块总览与 stubs 边界清单；Rust 对应实现：checkpoint/importer/monitor/orchestrator/submitter 模块均已接线；缺失行为：注释明确生产外部依赖仍有本地替身；测试证据：checkpoint、mock 和 parity 测试；风险：整条 IMPORT INTO 路径可能“形状完整但外部效果不完整”；建议：保留总览，把真实依赖替换作为单独实施计划。

候选：`lightning/pkg/importinto/mock/import_mock.rs:16-203`

- 分类：合法说明/示例误报。来源意图：解释 Go mockgen 对应的 MockCheckpointManager 等测试双；Rust 对应实现：可执行 mock；缺失行为：mock 文件不承担生产实现；测试证据：同目录 3 个 mock parity 测试；风险：不可作为真实 backend 证据；建议：保留。

候选：`lightning/pkg/importinto/mock/parity_test.rs:1-88`

- 分类：合法说明/示例误报。来源意图：mock 公共合同说明；Rust 对应实现：3 个可执行测试；缺失行为：无；测试证据：调用记录、返回错误与边界；风险：低；建议：保留。

候选：`lightning/pkg/importinto/mock/stubs.rs:1-44`

- 分类：部分实现且注释仍描述缺口。来源意图：给 mock 提供最小 Error/Result/Context；Rust 对应实现：轻量类型；缺失行为：完整 context 取消树和错误栈；测试证据：mock parity 测试；风险：仅测试范围，若被生产代码复用会掩盖取消/错误语义；建议：保留警示并限制可见性/用途。

候选：`lightning/pkg/importinto/parity_test.rs:1-124`

- 分类：合法说明/示例误报。来源意图：IMPORT INTO 公共合同索引；Rust 对应实现：1 个聚合契约测试；缺失行为：无被注释代码；测试证据：正常、边界、错误、资源清理；风险：主要由 mock 驱动；建议：保留。

候选：`lightning/pkg/importinto/precheck.rs:16-54`

- 分类：部分实现且注释仍描述缺口。来源意图：说明 precheck runner；Rust 对应实现：可执行 runner 和检查接口；缺失行为：真实集群容量、权限和表状态检查由外部实现决定；测试证据：parity 测试覆盖调用/错误合同；风险：桩检查成功可能放过生产前置条件；建议：保留说明并补真实环境 precheck。

候选：`lightning/pkg/server/checkpoint_control.rs:16-102`

- 分类：合法说明/示例误报。来源意图：说明 legacy/import-into checkpoint backend 分派；Rust 对应实现：CheckpointControl trait 与构造逻辑；缺失行为：本块无；测试证据：4 个 checkpoint-control 测试及 server parity；风险：backend 自身持久化风险在其模块；建议：保留。

候选：`lightning/pkg/server/checkpoint_control_test.rs:53-181`

- 分类：合法说明/示例误报。来源意图：server checkpoint 控制测试索引；Rust 对应实现：4 个可执行测试；缺失行为：无；测试证据：backend 路由、schema conflict、destroy/dump 与清理；风险：外部 DB 多为临时/内存替身；建议：保留。

候选：`lightning/pkg/server/lightning.rs:16-283`

- 分类：部分实现且注释仍描述缺口。来源意图：说明 server 启停、HTTP、任务队列、校验和清理顺序；Rust 对应实现：可执行控制面主流程；缺失行为：注释和相邻 stubs 表明 TLS、PD/TiKV、对象存储、指标等仍是局部替身；测试证据：serial/parity 测试覆盖 HTTP、取消和错误分类；风险：真实导入失败后的资源清理、外部连接关闭及并发取消最高；建议：保留边界说明，按外部依赖逐项建立集成验证。

候选：`lightning/pkg/server/lightning_serial_test.rs:16-62`

- 分类：合法说明/示例误报。来源意图：server 生命周期/错误测试说明；Rust 对应实现：4 个可执行串行测试；缺失行为：无；测试证据：配置错误、系统要求、schema conflict 与失败清理；风险：stub 环境；建议：保留。

候选：`lightning/pkg/server/lightning_server_serial_test.rs:16-92`

- 分类：合法说明/示例误报。来源意图：真实 TCP 控制面测试说明；Rust 对应实现：3 个可执行测试与 socket fixture；缺失行为：Importer 被显式队列搬移替代；测试证据：HTTP 状态、TOML/JSON、取消和 listener 关闭；风险：协议可信度较高，但不证明导入后端；建议：保留。

候选：`lightning/pkg/server/parity_test.rs:3-59`

- 分类：合法说明/示例误报。来源意图：server 公共合同速查；Rust 对应实现：2 个可执行聚合测试；缺失行为：无；测试证据：正常、边界、错误和资源清理合同；风险：外部资源为替身；建议：保留。

候选：`lightning/pkg/server/stubs.rs:1-379`

- 分类：部分实现且注释仍描述缺口。来源意图：明确列出 HTTP/TLS/PD/TiKV/objstore/metric/config 本地 stand-in；Rust 对应实现：大量可执行最小接口和内存状态；缺失行为：真实 TLS 握手、PD/TiKV RPC、对象存储一致性、指标注册和完整 context/error 模型；测试证据：server serial/parity 测试验证控制面；风险：这是本分片最高外部边界风险，尤其是失败恢复、并发取消和资源关闭；建议：保留警示，禁止把控制面测试当作生产数据面完成证据，逐依赖移植并做集成测试。

## 外部边界与后续优先级

1. 高：`lightning/pkg/server/stubs.rs`、`lightning/pkg/importinto/*`、`br/pkg/restore/log_client/stubs.rs` 和 import client 的真实 RPC/SQL/对象存储接线；验证远端部分成功后的取消、幂等与清理。
2. 中：BR checkpoint 的对象存储原子写、锁续租、损坏分片恢复，以及 registry/prealloc 的真实 SQL 事务语义。
3. 低：修订候选扫描器，使普通 `"br"`、`.to_string()` 等普通字符串不会跨数百行误判为 raw string；保留 JSON/SQL 夹具和模块级说明。

本报告不建议直接删除任何候选块；“合法说明”可在后续文档质量任务中精简，但不能作为行为修复替代品。
