# `br/pkg/task/restore_ebs_meta.rs`

## 文件定位

本文件是 `astersql-br-pkg-task` crate 中 EBS 快照恢复的“元数据准备阶段”实现，对照 Go 文件 [`br/pkg/task/restore_ebs_meta.go`](./restore_ebs_meta.go)。crate 根 [`br/pkg/task/lib.rs`](./lib.rs) 通过 `pub mod restore_ebs_meta` 挂载并通配重导出其公开符号；[`br/cmd/br/restore.rs`](../../cmd/br/restore.rs) 在 `full-backup-type = aws-ebs` 且 `prepare = true` 时调用 `RunRestoreEBSMetaWithDefaults`，否则转入后续数据恢复路径。

它负责注册隐藏的 EBS 恢复参数、读取并校验 `backupmeta`、通知 PD 进入恢复状态并重置时间戳、可选地通过 AWS 从快照创建卷、写出带恢复卷 ID 的元数据。它不负责把卷内 KV 数据导入 TiKV。当前 CLI 接线使用内存存储的默认入口，且生产默认 `ConfiguredPDController` 是无操作适配器；因此“真实外部存储读取”和“真实 PD RPC”尚未在这个 Rust 文件内接通，不能把接口顺序等同于已完成真实集群操作。

## 核心职责

- `DefineRestoreSnapshotFlags` 注册并隐藏 11 个高级参数，默认值与 Go 入口保持一致，包括 `prepare`、`output-file`、`skip-aws`、云 API 并发、卷规格、进度文件和目标可用区。
- `RunRestoreEBSMeta*` 建立一次恢复的配置、存储和 controller 生命周期；`RunRestoreEBSMetaWithController` 保证 `helper.restore()` 返回后总会调用 `helper.close()`。
- `RestoreEBSMetaHelper::preRestore` 从抽象 `Storage` 读取固定键 `backupmeta`，解析 JSON、执行语义校验，并拒绝空 PD 地址。
- `doRestore` 保持 Go 的关键顺序：先 `MarkRecovering`，再用备份的 `resolved_ts` 调用 `ResetTS`，最后走模拟或 AWS 卷恢复分支。
- `restore` 管理进度对象和可选进度写线程；仅当核心恢复成功后才写输出元数据。

## 主要符号

- `META_FILE: &str = "backupmeta"`：输入元数据在 `Storage` 中的固定名字。
- `flagPrepare` 至 `flagTargetAZ`：与 Go 同名语义的公开 CLI 标志常量；命名保留 Go 风格以便迁移对照。
- `DefineRestoreSnapshotFlags(&mut FlagSet)`：定义所有快照恢复标志，并逐一调用 `MarkHidden`；隐藏失败被有意忽略，与 Go 代码一致。
- `RestoreEBSMetaHelper<'a>`：单次运行上下文，借用 `Glue` 和可变 `RestoreConfig`，持有共享存储、已解析的可变 JSON 元数据及 controller。
- `RestoreEBSController`：可注入的 PD 边界，要求 `Send + Sync`，暴露 `MarkRecovering`、`ResetTS`、`Close`。`ConfiguredPDController` 当前三个方法均无真实副作用；测试通过 `RecordingController` 验证调用顺序。
- `AwsProgressAdapter`：把 task 层 `Progress::IncBy` 适配为 AWS crate 的 `Progress` trait。
- `preRestore`、`doRestore`、`restore`、`writeOutputFile`：helper 的准备、核心动作、总编排及结果落盘阶段。
- `validate_meta`、`valid_cluster_version`：输入 JSON 的结构和关键字段校验。版本接受可选 `v` 前缀、预发布/构建后缀，以及一至三段纯数字核心版本。
- `stores`、`store_count`、`volume_count_per_store`：从 `/tikv/stores` 读取拓扑。总进度使用“store 数 × 第一台 store 的卷数”，隐含所有 store 卷数相同的备份格式约束。
- `set_restore_volume_ids`：按原 `volume_id` 查表，将 `restore_volume_id` 写回每个卷；缺少映射时写空字符串而不是报错。
- `RunRestoreEBSMeta`：公开的可注入存储入口；`RunRestoreEBSMetaWithController` 是 crate 内测试接缝；`RunRestoreEBSMetaWithDefaults` 构造最小合法内存元数据，供当前 CLI/对齐测试使用。

## 执行流程

1. CLI 在 `br/cmd/br/restore.rs::runRestoreCommand` 解析配置；只有 EBS + prepare 分支进入 `RunRestoreEBSMetaWithDefaults`。普通 full restore 命令由 `DefineRestoreSnapshotFlags` 装配相关参数。
2. `RunRestoreEBSMetaWithDefaults` 创建 `MemStorage`，写入一份最小合法 `backupmeta`，然后进入 `RunRestoreEBSMeta`。直接调用者也可传入自己的 `Storage`。
3. `RunRestoreEBSMetaWithController` 先执行 `RestoreConfig::Adjust`，构造 helper，再调用 `restore`；无论结果成功或失败，随后都调用 `close`。
4. `restore` 调用 `preRestore`。后者读取、反序列化和校验元数据，检查 PD 地址，然后把原始 `serde_json::Value` 保存在 `meta_info`，以保留未知字段。
5. 进度总量由 store 数乘第一台 store 的卷数得到。`Glue::StartProgress` 返回共享进度对象；若 `ProgressFile` 非空，则启动线程，每 500ms 调用 `progressFileWriterRoutine`，直到取消或达到总量。
6. `doRestore` 先向 controller 发出 `MarkRecovering` 和 `ResetTS(resolved_ts)`。`SkipAWS` 为真时每个 store 增加一次进度并休眠 800ms，返回模拟大小 `1234`，不会改写卷 ID。
7. 真实 AWS 分支把 JSON 转为 `EBSBasedBRMeta`，按元数据 `Region` 和配置并发度创建 EC2 会话；可选启用 FSR，然后创建卷、等待卷就绪并把新卷 ID 写回元数据。启用、创建或等待失败时执行相应的 FSR/卷清理。
8. `doRestore` 后总是关闭进度并设置取消标记，再 join 写线程。只有 `doRestore` 成功才调用 `writeOutputFile`；它把完整 JSON 写到 `cfg.OutputMetaFile` 对应的同一 `Storage`。

## 数据与状态

输入和输出均以 `serde_json::Value` 保存，避免 Rust 强类型反序列化丢弃未来新增字段；测试中的 `unknown_future_field` 证明 SkipAWS 输出应与输入完全相等。必需结构为：`cluster_info` 对象包含合法版本、非零 `resolved_ts`、值为 `aws-ebs` 的 `full_backup_type`，且 `/tikv/stores` 非空。

helper 的 `meta_info` 在 `preRestore` 成功前为 `None`，之后的方法用 `expect` 表达内部调用顺序不变量。AWS 成功后它原地增加每个卷的 `restore_volume_id`。配置由调用方可变借用，并在入口首先 `Adjust`；资源对象通过 `Arc` 跨 helper 和进度线程共享。`total_size` 当前只用于成功判定边界，Rust 版本没有像 Go 版本一样写入 summary 日志。

## 依赖与调用关系

上游调用链由 RustCodeGraph 和源码共同确认：`br/cmd/br/restore.rs::runRestoreCommand` → `RunRestoreEBSMetaWithDefaults` → `RunRestoreEBSMeta` → `RunRestoreEBSMetaWithController` → `RestoreEBSMetaHelper::restore`。crate 根还把本模块公开 API 重导出；`br/pkg/task/parity_test.rs` 直接覆盖默认入口。

主要下游依赖是：本 crate 的 `RestoreConfig`、`Glue`/`Progress`/`Storage`/`MemStorage` 桩接口和 `progressFileWriterRoutine`；`serde_json` 负责宽松 JSON 保存与强类型 AWS 元数据转换；`astersql-br-pkg-aws` 提供 `NewEC2Session`、FSR、卷创建/等待/删除能力。[`br/pkg/task/Cargo.toml`](./Cargo.toml) 将该包声明为 `kind = "library"`、Go 对照包 `br/pkg/task`，并以路径依赖连接 `astersql-br-pkg-aws` 等瘦身 BR crate；注释明确 arm64 Darwin 路径不接入 kv/domain/kvproto/grpcio。

## 错误处理与边界

存储读取、JSON 编解码、controller 调用和 AWS 操作均通过 `Result` 向上传播。显式拒绝：缺少 `cluster_info`、非法/空版本、零 `resolved_ts`、空 store 列表、非 `aws-ebs` 备份，以及空 PD 地址。校验发生在启动进度前，所以无效输入不会产生进度资源。

AWS 清理按失败阶段区分：启用 FSR 失败时尝试禁用已启用项；创建卷失败时删除已经返回的卷并禁用 FSR；等待失败也删除卷并禁用 FSR；成功后禁用 FSR。清理错误被忽略，不覆盖原始失败。`set_restore_volume_ids` 对结构缺失或 ID 映射缺失采取容错写空值；若要把它改为强校验，必须同步评估 Go 兼容性和失败后的卷清理。

`ConfiguredPDController` 当前不执行真实 RPC，`RunRestoreEBSMetaWithDefaults` 也不读取用户配置的外部存储；这是明确的迁移接线边界。另一个边界是 `volume_count_per_store` 只查看第一台 store，异构卷数会使进度总量不准确，但不直接控制实际 AWS 创建数量。

## 并发与资源生命周期

唯一显式并发是进度文件线程。它持有 `Arc<dyn Progress>` 和 `Arc<AtomicBool>`，采用 `SeqCst` 读取/写入取消状态；主线程在 `doRestore` 返回后先 `Progress::Close`，再置取消标记并 join，因此返回前不会遗留该线程。循环每 500ms 唤醒一次，取消后的 join 最多可能等待一个睡眠周期。

controller 由 `Arc<dyn RestoreEBSController + Send + Sync>` 持有，并在 helper 运行结束后同步 `Close`；测试证明成功路径顺序是 `mark → reset:42 → close`，入口代码也保证 restore 错误路径执行 close。AWS 会话为局部值，卷和 FSR 资源用显式补偿清理管理；没有 Rust `Drop` 守卫，因此新增早退点时必须同时维护所有清理分支。

## 与 Go 版本的对应关系

Rust 的标志名、默认值、隐藏策略、helper 阶段划分、PD 操作顺序、SkipAWS 的每 store 进度与 `1234` 返回值、FSR/卷创建清理，以及最终元数据写出，均直接对应 `restore_ebs_meta.go` 中的同名逻辑。

当前差异必须保留为事实：Go `preRestore` 通过 `GetStorage`/`config.NewMetaFromStorage` 读取真实外部存储并创建真实 `pdutil.PdController`，Rust 默认 controller 仍是无操作实现；Go CLI 直接传配置进入真实入口，Rust CLI 调用 `WithDefaults` 的内存 fixture；Go 输出使用 `os.WriteFile(..., 0600)` 写本地路径，Rust写入抽象 `Storage`；Go 还带 context 取消、tracing span、成功/失败 summary 日志和真实 TLS/PD controller 配置，Rust文件未实现这些部分。Go AWS 会话使用 `cfg.S3.Region`，Rust使用已解析元数据的 `Region`。仓库中未找到直接针对该 Go 文件的 `*_test.go`，因此 Go 行为依据来自生产实现，边界回归主要由独立 Rust 测试承载。

## 扩展指南

- 接入真实 Rust CLI 前，优先替换 `ConfiguredPDController` 和 `RunRestoreEBSMetaWithDefaults` 的临时接线：从配置创建实际外部 `Storage` 与 PD controller，同时保持 `Adjust → restore → close` 和失败清理顺序。
- 新增或调整元数据字段时，在 `validate_meta`、AWS 强类型 `EBSBasedBRMeta` 与 `set_restore_volume_ids` 三处检查兼容性；继续用 `Value` 保存未知字段，避免旧版本中转时损坏新字段。
- 修改卷创建阶段时，把清理设计为一等约束；任何新增早退都应删除已创建卷并在必要时禁用 FSR。若引入并发创建，需保持进度计数、ID 映射和清理集合线程安全。
- 调整进度算法时，不要继续假定每台 store 卷数相同；可对所有 stores 的卷数求和，并新增异构卷数回归测试。
- 测试逻辑继续放在独立的 [`br/pkg/task/restore_ebs_meta_test.rs`](./restore_ebs_meta_test.rs)，不要内嵌进生产文件。至少同步覆盖非法元数据、空 PD、controller 失败仍关闭、AWS 各阶段清理、未知字段保留以及进度线程终止。
- 与 Go 语义发生有意差异时，应先核对 `restore_ebs_meta.go` 并记录原因；不要为了让测试通过而省略真实 PD、存储或 AWS 生命周期。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；查询 `restore_ebs_meta` 定位了本文件的 7 个常量、3 个类型/trait、公开入口和内部 helper 函数。
- RustCodeGraph 流图确认：`RunRestoreEBSMetaWithDefaults → RunRestoreEBSMeta → RunRestoreEBSMetaWithController`，以及 `restore → preRestore → validate_meta`；对文件节点的分段读取覆盖了 399 行源码。
- 源码与配置证据：[`restore_ebs_meta.rs`](./restore_ebs_meta.rs)、[`restore_ebs_meta.go`](./restore_ebs_meta.go)、[`Cargo.toml`](./Cargo.toml)、[`lib.rs`](./lib.rs)、[`br/cmd/br/restore.rs`](../../cmd/br/restore.rs)。
- 独立测试证据：[`restore_ebs_meta_test.rs`](./restore_ebs_meta_test.rs) 覆盖缺失输入/非 EBS 类型、SkipAWS 的进度总量和未知字段保留、空 PD 在进度前失败、controller 的 mark/reset/close 顺序；[`parity_test.rs`](./parity_test.rs) 覆盖默认入口能完成。
- 本任务是纯文档分析，未运行 Cargo 或代码测试；交付验证仅检查固定 11 章节、链接与事实证据，并人工复核当前未接线能力没有被描述成已支持。
