# `br/pkg/task/backup_ebs.rs`

## 文件定位

[`backup_ebs.rs`](backup_ebs.rs) 属于 `astersql-br-pkg-task` library crate；crate 根在 [`lib.rs`](lib.rs) 中以 `pub mod backup_ebs` 挂载模块，并通过 `pub use backup_ebs::*` 把公开符号平铺到任务层 API。其上游命令入口是 [`br/cmd/br/backup.rs`](../../cmd/br/backup.rs) 的 `newFullBackupCommand`：该函数给 `backup full` 注册 `DefineBackupEBSFlags`，而 `runBackupCommand` 在解析出 `FullBackupTypeEBS` 后调用 `RunBackupEBS`。

这个文件是 Rust 迁移期的 EBS 卷备份辅助与轻量编排实现。当前实现并不是 Go `backup_ebs.go` 的完整生产快照流程：真实 AWS EC2、PD/TiKV 调度与 GC safepoint 边界尚未接入，文件注释也将这些边界明确交给桩和上层 glue。因而它目前主要保证 CLI 契约、输入校验、region 连续性算法、进度形状以及 `backupmeta.json` 落盘形状。

crate 边界由 [`Cargo.toml`](Cargo.toml) 定义：`astersql-br-pkg-task` 直接依赖 `astersql-br-pkg-aws`（提供 `EBSBasedBRMeta`）、`astersql-br-pkg-common`（提供 `MaxStoreConcurrency`）与 `serde_json`，并以 `br/pkg/task` 为 Go 对照包。

## 核心职责

1. `DefineBackupEBSFlags` 注册并隐藏 EBS 全量备份的六个内部/高级 flag，默认值与 Go 保持一致。
2. `isRegionsHasHole` 对 region 按 `StartKey` 原地排序，再验证相邻 `EndKey`/`StartKey` 是否首尾相接，为安全快照提供纯算法检查。
3. `parseGoDuration`、`getMockSleepTimeFromEnv` 与 `getMockSleepTime` 模拟 Go `time.ParseDuration` 的本任务所需语义，支持复合单位、微秒别名、零值与负值立即到期。
4. `saveMetaFile` 将 `EBSBasedBRMeta` JSON 序列化后写到固定对象名 `backupmeta.json`。
5. `RunBackupEBS` 完成配置校正、备份类型校验、进度初始化、占位元数据写入、进度结束和成功状态设置。
6. `waitAllScheduleStoppedAndNoRegionHole` 保留 Go 同名流程的最小前置契约：拒绝空 store 集合与存在 region 空洞的拓扑。

## 主要符号

- `flagBackupVolumeFile: &str = "volume-file"`：卷清单 flag 名；`DefineBackupEBSFlags` 给出的默认值是 `./backup.json`。当前 `RunBackupEBS` 本身不读取该文件，字段只会由 [`backup.rs`](backup.rs) 的 `BackupConfig::ParseFromFlags` 保存到 `VolumeFile`。
- `flagProgressFile: &str = "progress-file"`：进度文件 flag 名，默认 `progress.txt`；非空时 `RunBackupEBS` 调用公共 `progressFileWriterRoutine`。
- `DefineBackupEBSFlags(&mut FlagSet)`：公开 CLI 注册函数。除上述两个 flag 外，还注册 `full-backup-type=kv`、`skip-aws=false`、公共云 API 并发默认值以及 `operator-paused-gc-and-schedulers=false`，随后逐项 `MarkHidden`；隐藏失败被刻意忽略，与 Go `_ = flags.MarkHidden(...)` 一致。
- `isRegionsHasHole(&mut [Region]) -> bool`：公开、会修改输入顺序的连续性检查。0 或 1 个 region 因 `saturating_sub(1)` 不进入循环，返回 `false`；重叠、间隙或非末尾空 `EndKey` 都会因边界不相等返回 `true`。
- `parseGoDuration(&str) -> Option<Duration>`：私有解析器，逐段读取浮点数和 `ns/us/µs/μs/ms/s/m/h` 单位；非法、空字符串、非有限值或溢出返回 `None`。负持续时间先验证绝对值格式，再映射为 `Duration::ZERO`，以模拟 Go `time.After` 的立即到期效果。
- `getMockSleepTimeFromEnv(Option<&str>) -> Duration`：crate 内可见的可测试入口；缺失或解析失败回退为 800ms。
- `getMockSleepTime() -> Duration`：读取 `br_ebs_backup_mocking_wait_snapshot_duration` 环境变量并委托上一函数。当前 `RunBackupEBS` 没有调用它，这是与 Go 模拟快照分支的重要差异。
- `saveMetaFile(&EBSBasedBRMeta, &dyn Storage) -> Result<()>`：公开持久化函数。序列化错误转换成任务层 `Error`，存储写错误原样传播。
- `RunBackupEBS(&dyn Glue, &mut BackupConfig, Arc<dyn Storage>) -> Result<()>`：公开任务入口；会原地调整 `cfg`，并通过 trait 对象接入进度与存储。
- `waitAllScheduleStoppedAndNoRegionHole(&[Store], &mut [Region]) -> Result<()>`：公开轻量校验入口；名称对应 Go 的轮询函数，但 Rust 当前没有连接 PD、没有重试，也没有并行收集 region。

## 执行流程

CLI 主链为：`NewBackupCommand` → `newFullBackupCommand` → `DefineBackupEBSFlags`；执行时 `runBackupCommand` 调用 `BackupConfig::ParseFromFlags`，仅当解析出的 `FullBackupType` 为 `aws-ebs` 才构造存储并进入 `RunBackupEBS`。

`RunBackupEBS` 的实际步骤如下：

1. 调用 `BackupConfig::Adjust`，校正公共备份并发、GC TTL、压缩类型和云 API 并发默认值等配置。
2. 调用 `Summary("EBS Backup")` 建立摘要；该动作发生在类型校验之前。
3. 若 `FullBackupType` 为空，先回落为 KV；随后用 `Valid` 拒绝未知类型，再显式拒绝任何非 `aws-ebs` 类型。因此空值最终也会以“requires aws-ebs”失败，而不会误入快照路径。
4. 若 `CloudAPIConcurrency == 0`，设置为 `defaultCloudAPIConcurrency`；`MaxStoreConcurrency` 目前只被触碰以保留符号形状，没有参与并发控制。
5. 通过 `Glue::StartProgress("EBS Backup", 100, !LogProgress)` 创建总量固定为 100 的进度对象；配置了 `ProgressFile` 时调用公共写入例程。
6. 构造仅设置 `Region = "us-west-2"` 的默认 `EBSBasedBRMeta`。`SkipAWS` 两个分支目前都调用同一 `saveMetaFile`，所以该开关不改变 Rust 路径的实际 I/O。
7. 元数据写入成功后一次性 `IncBy(100)`、`Close`，再调用 `SetSuccessStatus(true)`。最后构造未使用的 `MemStorage` 与默认 `Store` 以保留迁移期符号引用，然后返回成功。

独立的 `waitAllScheduleStoppedAndNoRegionHole` 先验证至少存在一个 store，再调用 `isRegionsHasHole`；任一条件失败立即返回错误。它当前不在 `RunBackupEBS` 的调用链内。

## 数据与状态

- `BackupConfig` 是主要可变输入。`RunBackupEBS` 会通过 `Adjust` 和空类型回落修改其中的并发、TTL/压缩默认值及 `FullBackupType`；调用者不能假定配置保持原样。
- `EBSBasedBRMeta` 是唯一持久化业务数据，但当前只填入固定 region，其余字段保持默认。因此输出不含 Go 版本会设置的集群版本、全量备份类型、resolved TS、snapshot ID 和可用区映射。
- `Region.StartKey`/`EndKey` 以字节序比较。`isRegionsHasHole` 原地排序切片，调用者若依赖原始顺序必须事先复制。
- 进度总量与增量均为固定 100；`Progress` trait 要求 `Send + Sync`，内存实现使用原子计数和原子关闭标志。
- `Storage` trait 要求 `Send + Sync`；`saveMetaFile` 固定覆盖 `backupmeta.json`。当前 CLI 上游在 [`br/cmd/br/backup.rs`](../../cmd/br/backup.rs) 中传入 `MemStorage`，并非 S3/GCS/Azure/local 生产后端。
- 进程级环境变量只在调用 `getMockSleepTime` 时读取，没有缓存；摘要成功状态由全局/桩接口记录。

## 依赖与调用关系

RustCodeGraph 给出的主要上游边包括：`newFullBackupCommand → DefineBackupEBSFlags`、`runBackupCommand → RunBackupEBS`、`RunBackupEBS → saveMetaFile`；测试侧 `backup_ebs_test.rs` 调用 flag、时长和 region 函数，`parity_test.rs` 调用 `isRegionsHasHole` 与 `RunBackupEBS`。crate 根的通配导出使 CLI 能从 `astersql_br_pkg_task` 直接导入这些公开符号。

主要下游依赖为：

- [`backup.rs`](backup.rs)：提供 `BackupConfig::Adjust`、EBS 字段与 flag 解析；
- [`common.rs`](common.rs)：提供 `FullBackupType`、EBS/KV 字面量、公共 flag、云 API 默认并发及进度文件写入例程；
- [`stubs.rs`](stubs.rs)：提供 `Glue`、`Progress`、`Storage`、`Region`、`Store`、摘要/成功状态与内存实现；
- `astersql-br-pkg-aws::EBSBasedBRMeta`：定义输出 JSON 的数据形状；
- `serde_json`：执行序列化；
- `astersql-br-pkg-common::MaxStoreConcurrency`：当前仅为未接线的并发边界占位。

下游恢复侧会读取 `backupmeta.json` 并校验 `full_backup_type`（见 [`restore_data.rs`](restore_data.rs)）。由于本文件当前生成的占位 meta 未设置该字段，不能据此推断它已经与完整 Rust EBS 恢复链端到端兼容。

## 错误处理与边界

- 配置类型有两级检查：`FullBackupType::Valid` 区分未知字面量，随后严格要求 `aws-ebs`；两种失败分别返回 `invalid full backup type` 和 `RunBackupEBS requires aws-ebs full backup type`。
- JSON 序列化和 `Storage::WriteFile` 是 `RunBackupEBS` 当前主要可失败 I/O；失败会在进度 `IncBy`/`Close` 和成功标记之前通过 `?` 返回。函数没有作用域守卫，因此失败后是否需要关闭进度由后续实现特别处理。
- `parseGoDuration` 要求每个数值后都有受支持单位，唯独裸 `"0"` 特判为零；负值不会保留符号，而是归一为零。它使用 `f64` 累加，极大值或无法转成 `Duration` 的值会回退 800ms。
- `isRegionsHasHole` 只验证相邻边界，不单独验证第一个 `StartKey` 为空或最后一个 `EndKey` 为空；这与 Go 算法一致。空 region 集合本身返回“无洞”，需要由 `waitAllScheduleStoppedAndNoRegionHole` 的 store 检查或更高层约束保证上下文有效。
- `waitAllScheduleStoppedAndNoRegionHole` 的 `no stores` 与 `region hole found` 是本地字符串错误；它不具备 Go 版的上下文取消、约 10 分钟指数退避、pending admin 分类日志或 PD/TiKV 网络错误传播。
- `SkipAWS` 当前两个分支完全等价，且仍写 meta；不要将它理解为完整 Go 路径中“跳过 AWS 后按 store 次数等待并推进进度”的实现。

## 并发与资源生命周期

本文件自身不创建线程、异步任务、锁、通道或网络连接。共享对象通过 `&dyn Glue` 和 `Arc<dyn Storage>` 传入，trait 都要求 `Send + Sync`；进度对象也是线程安全 trait 对象，但这里同步使用。

正常路径的生命周期是“创建进度 → 可选写进度文件 → 写 meta → 增加 100 → 关闭 → 标记成功”。元数据写入失败会提前返回，当前代码不会显式关闭进度。`progressFileWriterRoutine` 的具体资源策略属于 [`common.rs`](common.rs)，本文件只传入 progress 引用、总量、路径和 `false` 标志。

与之相比，Go 实现包含 cancellable context、`sync.Once` 清理、scheduler 恢复、GC safepoint keeper、并发 store RPC、AWS 异步快照及失败删除快照；这些生命周期均未移植到本 Rust 文件。扩展时必须以作用域守卫或等价机制保证“暂停/恢复”“创建/失败删除”“进度创建/关闭”成对，不能只把成功路径接上。

## 与 Go 版本的对应关系

直接对照文件是 [`backup_ebs.go`](backup_ebs.go)，相关 Go 测试是 [`backup_ebs_test.go`](backup_ebs_test.go)。已对齐部分包括：六个隐藏 flag 的名称与默认值、`isRegionsHasHole` 的排序和相邻边界算法、模拟等待环境变量名及 800ms 回退、JSON meta 文件名/写入意图。

Rust 独立测试 [`backup_ebs_test.rs`](backup_ebs_test.rs) 复刻 Go `TestIsRegionsHasHole` 的六类用例：单 region、两个连续 region、多段连续、错乱/重叠边界、提前空终点和明显空洞；此外增加了 flag 默认值与 Go duration 解析测试。[`parity_test.rs`](parity_test.rs) 还验证 EBS 类型 + `SkipAWS` 路径会生成 `backupmeta.json`。

未对齐部分是实质性的：Go `RunBackupEBS` 会读取卷清单、拒绝零 store、建立真实外部存储和备份锁、获取 TS、暂停/恢复 scheduler 与 GC、检查所有 TiKV leader region、取得集群版本、创建并等待 AWS snapshots、填充完整 meta，并在失败时清理 snapshots；Rust 当前只写固定 region 的默认 meta。Go `waitAllScheduleStoppedAndNoRegionHole` 也包含 store 获取、RPC 并行收集、pending admin 检测、取消与重试，而 Rust 同名函数只是对已提供切片进行一次检查。文档因此将 Rust 状态标为迁移期骨架，而非功能等价实现。

## 扩展指南

- 补齐生产 EBS 链时，首要修改点是 `RunBackupEBS`。应先为 PD/AWS/外部存储定义可注入 trait，再按 Go 顺序接入卷清单、备份锁、TS、scheduler/GC、region 检查、snapshot 创建/等待、完整 meta 和失败清理；不要把真实客户端硬编码进单测。
- 扩展 `waitAllScheduleStoppedAndNoRegionHole` 时，应将纯 `isRegionsHasHole` 保持为无 I/O 算法，并把 store 枚举、RPC、重试和取消放在独立可测试控制器中。并发上限应真正使用 `MaxStoreConcurrency`，同时保证连接关闭和错误组取消。
- 修改 meta 时必须同步检查 `restore_data.rs`、`restore_ebs_meta.rs` 对 `full_backup_type`、resolved TS、snapshot ID、AZ 和集群版本的读取契约，避免写端“成功”但恢复端拒绝。
- 修改 flag 或默认值时同步更新 `BackupConfig::ParseFromFlags`、`br/cmd/br/backup.rs` 和 `backup_ebs_test.rs`；公开兼容风险主要是隐藏 flag 被 operator/自动化脚本依赖。
- 修改 duration 解析时增加 `backup_ebs_test.rs` 的独立单元测试，尤其覆盖多段单位、微秒拼写、非法数字、负数与溢出。不要把测试内嵌回生产源文件。
- 修改 region 算法时保持 Go 表驱动用例，并新增乱序、空切片、重复起点和首尾不覆盖等明确契约用例；排序成本是 `O(n log n)`，大集群下应避免不必要复制。
- 为 `RunBackupEBS` 增加失败注入测试，验证序列化/写入失败时进度关闭、全局成功状态、scheduler/GC 恢复及 snapshot 清理。相关 Rust 测试应继续放在同目录独立 `*_test.rs` 文件。

## 验证依据

- RustCodeGraph `status`：索引覆盖目标文件，报告 `br/pkg/task/backup_ebs.rs` 含 19 个符号。
- RustCodeGraph `explore "br/pkg/task/backup_ebs.rs DefineBackupEBSFlags isRegionsHasHole saveMetaFile RunBackupEBS"`：确认 `RunBackupEBS → saveMetaFile`，以及 CLI、parity/独立测试调用边。
- RustCodeGraph `node --file`：完整阅读 `br/pkg/task/backup_ebs.rs`，并核对 `br/pkg/task/lib.rs`、`br/cmd/br/backup.rs`、`br/pkg/task/backup.rs`、`br/pkg/task/stubs.rs`、`br/pkg/task/backup_ebs_test.rs`、`br/pkg/task/parity_test.rs`、`br/pkg/task/backup_ebs.go`、`br/pkg/task/backup_ebs_test.go` 的直接证据。
- 原始配置读取：[`Cargo.toml`](Cargo.toml)，确认 crate 名、library 入口、Go 包映射和直接依赖。
- 测试证据：Rust 独立测试覆盖 flag 默认值、模拟 duration 与 region 连续性；parity 测试覆盖 `SkipAWS` 写 meta。Go 独立测试只覆盖 region 算法，不能作为完整 Go 编排的行为验证。
- 本任务是纯文档分析，按计划不运行 Cargo；结论仅描述当前源码和索引可见调用关系，不声称真实 AWS/PD EBS 备份已在 Rust 端完成端到端验证。
