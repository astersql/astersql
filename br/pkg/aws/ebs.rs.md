# `br/pkg/aws/ebs.rs`

## 文件定位

[`br/pkg/aws/ebs.rs`](./ebs.rs) 是 BR 的 AWS EBS 适配实现，属于独立 library crate `astersql-br-pkg-aws`。`br/pkg/aws/Cargo.toml` 把 `lib.rs` 设为 crate 根，`br/pkg/aws/lib.rs` 通过 `#[path = "ebs.rs"] pub mod ebs` 装配本文件并 `pub use ebs::*`，因此本文件的公开类型和函数就是该 crate 对外 API。

在当前 Rust 生产调用链中，`br/pkg/task/restore_ebs_meta.rs::doRestore` 通过 task crate 对 `astersql-br-pkg-aws` 的路径依赖调用 `NewEC2Session`，然后按配置执行 `EnableDataFSR`、`CreateVolumes`、`WaitVolumesCreated`、`DisableDataFSR`，失败时调用 `DeleteVolumes` 清理。`CreateSnapshots`、`WaitSnapshotsCreated`、`DeleteSnapshots` 也已公开实现并有独立测试，但在排除测试后的 Rust 源码搜索中没有发现外部生产调用者；不能据此把它们描述成已接入 Rust 备份主链。

本文件不是门面或桩：它包含真实 AWS SDK 适配、可注入客户端接口、EBS 元数据模型、并发调度、轮询和错误处理。门面位于相邻的 `lib.rs`。

## 核心职责

- 用 `NewEC2Session` 加载指定 region 的 AWS 配置，配置 standard retry（最多 9 次），并以 WebPKI roots 构造 EC2 与 CloudWatch 客户端。
- 通过 `Ec2Client`、`CloudWatchClient` trait 隔离 AWS SDK，使编排逻辑可以注入内存客户端测试；`AwsEc2Client` 与 `AwsCloudWatchClient` 是生产同步适配器。
- 用 `CreateSnapshots`/`WaitSnapshotsCreated`/`DeleteSnapshots` 管理 TiKV 数据卷快照，包括实例反查、排除启动盘及非目标数据盘、pending 配额退避、增量进度和尽力清理。
- 用 `EnableDataFSR`/`DisableDataFSR` 管理 Fast Snapshot Restore：按 AZ 分组、每批最多 10 个快照、等待 CloudWatch credit 和 EC2 FSR 状态。
- 用 `CreateVolumes`/`WaitVolumesCreated`/`DeleteVolumes` 从快照恢复卷，包括 AZ 选择、IOPS/throughput/encryption 参数、标签继承、FSR 校验、容量汇总和失败清理。
- 用 `ErrorGroup` 与 `WorkerPool` 模拟 Go 版 `errgroup` 和有界 worker pool，并以共享映射汇聚并发结果。

## 主要符号

- `pub type Result<T> = std::result::Result<T, String>`：本 crate 的统一错误边界。AWS SDK service error 先经 `aws_error_to_string` 保留 code/message，再转为字符串。
- `POLLING_PENDING_SNAPSHOT_INTERVAL`、`ERR_CODE_TOO_MANY_PENDING_SNAPSHOTS`、`FsrApiSnapshotsThreshold`：分别定义 30 秒配额退避、可重试错误码，以及 FSR API 每批 10 个快照的上限。
- `EBSVolume`、`EBSStore`、`TiKVComponent`、`EBSBasedBRMeta`：与 BR EBS JSON 元数据相符的 serde 模型；字段通过 `rename` 对应 `volume_id`、`snapshot_id`、`volume_az`、`tikv` 等键。
- `Progress`：只要求线程安全的 `IncBy(i64)`，让本 crate 不依赖完整 glue crate；task 层以 `AwsProgressAdapter` 接入实际进度对象。
- `Snapshot`/`SnapshotState`、`Volume`/`VolumeState`、`Instance`、`BlockDeviceMapping` 等：AWS SDK 类型的精简内部视图，既承载生产 SDK 转换，也供注入测试使用。
- `Ec2Client`：覆盖 Describe/Create/Delete snapshot、Describe/Create/Delete volume、Describe instance，以及 FSR enable/disable/describe 的同步接口。
- `CloudWatchClient::GetFastSnapshotRestoreCreditsBalance`：查询指定 snapshot/AZ 最近五分钟的 FSR credit maximum。
- `AwsEc2Client`、`AwsCloudWatchClient`：持有官方 SDK client 与共享 Tokio `Runtime`，用 `block_on` 将异步 SDK 调用适配为同步 trait。
- `EC2Session`：核心编排对象，持有两个 trait object、并发度和 `EC2SessionTimers`。`NewEC2SessionWithClients` 是注入入口，`with_timers` 允许测试缩短等待。
- `NewEC2Session`：生产构造入口；创建 runtime、TLS connector、AWS config 和两个真实客户端。
- `EC2Session::{CreateSnapshots, WaitSnapshotsCreated, DeleteSnapshots}`：快照生命周期 API。
- `EC2Session::{EnableDataFSR, DisableDataFSR}` 与私有 `wait_data_fsr_enabled`、`get_fsr_credit_balance`：FSR 生命周期 API。
- `EC2Session::{CreateVolumes, WaitVolumesCreated, DeleteVolumes, HandleDescribeVolumesResponse}`：恢复卷生命周期 API。
- `create_snapshots_with_retry`：仅对包含 `PendingSnapshotLimitExceeded` 的错误循环退避；其他错误补充请求上下文后返回。
- `fetch_target_snapshots`：只选择 `Type == "storage.data-dir"` 的快照，并按指定 AZ 或卷自身 AZ 分组。
- `ErrorGroup`、`WorkerPool`：线程句柄与首错收集器、有界并发令牌实现。`WorkerPool` 还把 worker panic 转成 `"worker panicked"`。

## 执行流程

### 生产恢复主链

1. `restore_ebs_meta.rs::doRestore` 将 JSON 元数据反序列化为 `EBSBasedBRMeta`，用 `Region` 和 `CloudAPIConcurrency` 调用 `NewEC2Session`。
2. 若 `UseFSR` 为真，`EnableDataFSR` 调用 `fetch_target_snapshots`，只收集 `storage.data-dir` 快照；为空时返回 `empty backup meta`。每个 AZ 的列表按 `FsrApiSnapshotsThreshold` 切批，各批在线程中调用 `EnableFastSnapshotRestores`。
3. 每批启用请求成功后，`wait_data_fsr_enabled` 先确认快照存在，再逐个查询 CloudWatch credit。credit 大于等于 1 才推进；查询错误按无数据处理，连续无数据超过三次报错。之后轮询 EC2 FSR 状态；`disabled`/`disabling` 是冲突错误，查询结果中不再出现的 pending ID 被视为已完成。
4. `CreateVolumes` 遍历全部 store/volume，为每个旧卷启动受 `WorkerPool` 限制的任务。目标 AZ 非空时覆盖原 AZ；IOPS/throughput 仅在大于 0 时传入。它先查询源快照，生成 BR/CSI/来源标签，再把源快照中不以 `snapshot/` 开头的标签加前缀复制到新卷。
5. `WaitVolumesCreated` 按计时器轮询 `DescribeVolumes`。`HandleDescribeVolumesResponse` 累加 `Available` 卷的 GiB；非 `Available` 且 ID 非空的卷进入下一轮。若调用方要求 FSR，且 AWS 明确返回 `FastRestored == false`，立即失败。
6. task 层成功后把 old-volume-ID 到 new-volume-ID 映射写回元数据；不论成功还是可恢复的失败路径，已启用的 FSR 都会尝试禁用。建卷或等待失败时，task 层还调用 `DeleteVolumes` 尽力清理已创建卷。

### 快照生命周期

1. `CreateSnapshots` 按 store 收集目标 volume ID；空 volume 列表跳过。它用第一个卷 `DescribeVolumes` 反查 EC2 instance，再 `DescribeInstances` 枚举块设备。
2. 根设备由 `ExcludeBootVolume = true` 排除；实例上不属于当前 store 目标集合的其他 EBS 数据盘进入 `ExcludeDataVolumeIds`，防止误拍。
3. 每个 store 的 `CreateSnapshots` 请求通过有界池执行；遇 pending snapshot 配额错误按 timer 重试。结果写入共享 `volume_id -> snapshot_id` 映射；全部任务成功后再次 `DescribeVolumes` 建立 `volume_id -> availability_zone` 映射。
4. `WaitSnapshotsCreated` 每轮查询剩余 snapshot。`Completed` 累加 `VolumeSize`，`Error` 立即返回，其他状态保留到下一轮；进度字符串经 `extract_snap_progress` 解析，并只把相对历史值的正增量传给 `Progress::IncBy`。
5. `DeleteSnapshots` 并发尝试删除映射中的每个 snapshot，不把单项删除失败暴露给调用者。

## 数据与状态

- `EBSBasedBRMeta.TiKVComponent` 是 `Option`；`tikv_stores` 在缺失时返回空切片，避免解引用空组件。恢复的 FSR 入口把空分组视为错误，而创建快照/卷会自然得到空工作集。
- `CreateSnapshots` 的共享 `HashMap<String, String>` 由 `Arc<Mutex<_>>` 保护，键是源 volume ID，值是 snapshot ID；`VolumeAZs` 独立记录 volume 所在 AZ。
- `CreateVolumes` 的共享映射也是 `Arc<Mutex<_>>`，键是旧 volume ID，值是 AWS 返回的非空新 volume ID。AWS 未返回 ID时不会插入，但任务本身仍可返回成功。
- `WaitSnapshotsCreated` 保存每个 snapshot 已上报的百分比，保证 `IncBy` 只增加不回退；完成容量按每个本轮返回的 `VolumeSize` 累加。
- `EC2SessionTimers` 的生产默认值为：pending 配额重试 30 秒、快照和卷轮询各 5 秒、FSR credit 重试 3 分钟、FSR 状态轮询 1 分钟。测试通过 `with_timers` 改为毫秒级。
- FSR 分组只纳入 `storage.data-dir`，以 snapshot ID 的 `Vec` 保留元数据遍历顺序；每个 AZ 的批次独立执行。
- `SnapshotState::Other` 和 `VolumeState::Other` 保留未知 AWS 状态文本，避免转换时把未来状态误判为已完成。

## 依赖与调用关系

上游关系：

- `br/pkg/aws/lib.rs` 声明并公开再导出本模块。
- `br/pkg/task/Cargo.toml` 以路径依赖引入 `astersql-br-pkg-aws`。
- RustCodeGraph 与源码搜索共同确认 `br/pkg/task/restore_ebs_meta.rs::doRestore` 是 `NewEC2Session` 的生产 Rust 调用者，并串联 FSR 与卷恢复 API。
- `br/pkg/aws/ebs_test.rs` 和 `br/pkg/aws/parity_test.rs` 通过注入 trait 客户端覆盖内部编排；它们是独立测试文件，不与生产源混放。

下游关系：

- `aws-config`、`aws-types`：region、默认凭据/端点配置与 standard retry。
- `aws-sdk-ec2`：卷、实例、快照和 FSR API；`ProvideErrorMetadata` 用于保留 service error code/message。
- `aws-sdk-cloudwatch`：`AWS/EBS` 命名空间的 `FastSnapshotRestoreCreditsBalance` 指标。
- `aws-smithy-http-client`、`hyper-rustls`：使用内置 WebPKI roots 的 HTTP/TLS connector；TLS 验证仍开启。
- `tokio`：仅为官方异步 SDK 提供共享 multi-thread runtime，本文件的外层 API 仍是同步的。
- `serde`：EBS 元数据 JSON 序列化/反序列化。
- 标准库 `Arc`/`Mutex`/`Condvar`/`JoinHandle`：共享客户端、结果映射、有界并发和线程汇聚。

当前调用边的关键链为：`restore_ebs_meta.rs::doRestore -> NewEC2Session -> EnableDataFSR -> wait_data_fsr_enabled -> {CloudWatch, EC2}`，以及 `doRestore -> CreateVolumes -> Ec2Client::{DescribeSnapshots, CreateVolume} -> WaitVolumesCreated -> DescribeVolumes`。失败清理由 task 层显式调用 `DeleteVolumes` 和 `DisableDataFSR`。

## 错误处理与边界

- `NewEC2Session` 只在创建 runtime 或加载 AWS config/客户端所需配置失败时返回字符串错误；实际 API 访问发生在后续方法。
- `aws_error_to_string` 优先输出 `code: message`，没有 message 时保留 code，没有 service metadata 时退回 `Display`。这是识别 `PendingSnapshotLimitExceeded` 的必要条件，`ebs_test.rs::test_aws_service_error_preserves_code_and_message` 有对应断言。
- `CreateSnapshots` 对 `DescribeVolumes`/`DescribeInstances` 的空响应做显式错误处理；缺失 attachment 或 instance ID 报“specified volume ... is not attached”。并发创建失败时返回已经收集的部分 snapshot map、空 AZ map 和首错。
- 配额重试没有本地次数上限：只要错误字符串持续包含 pending-limit code，就会按 timer 一直重试；其他错误立即结束。
- `extract_snap_progress` 仅去除空格字符，不去除 tab；找不到 `%` 或数值解析失败返回 0，小数向零截断，超过 100 钳制到 100，`%` 后的附加文本被接受。独立 Rust 测试明确验证 tab 不被改写。
- `WaitSnapshotsCreated` 对 `SnapshotState::Error` 只用 snapshot ID 构造错误，不依赖可选 `StateMessage`；pending/unknown 状态持续轮询，没有超时参数。
- `EnableDataFSR` 的空 meta 是错误，`DisableDataFSR` 的空 map 则是成功 no-op。任一批次返回 unsuccessful item 时，以第一个失败项构造错误。
- FSR credit 返回 `Some(<1)` 时会无限等待，不增加“无数据”重试计数；只有错误或 `None` 才在四次检查序列后报错。FSR 状态轮询同样无总超时。
- `CreateVolumes` 查不到源快照会失败；源标签已有 `snapshot/` 前缀时不再复制，以避免递归加前缀。`iops`/`throughput <= 0` 转为 `None`。
- `HandleDescribeVolumesResponse` 仅在 `FastRestored` 明确为 `Some(false)` 时拒绝；`None` 不会失败。未返回 volume ID 的非 available 项不会进入下一轮。
- `DeleteSnapshots` 与 `DeleteVolumes` 是 best-effort：它们吞掉单项 API 错误且无返回值。调用者不能从接口判断是否清理完整，需依靠外部审计或后续清理机制。
- `Mutex` poisoned 会触发 `expect` panic；worker 闭包内的 panic 会被 `WorkerPool` 捕获并转成普通错误，但调度线程或结果读取处的 poison panic 不在该保护范围内。

## 并发与资源生命周期

- `EC2Session` 用 `Arc<dyn Ec2Client>` 和 `Arc<dyn CloudWatchClient>` 共享客户端；trait 要求 `Send + Sync`，允许跨 worker 线程调用。
- `WorkerPool::apply_on_error_group` 在提交前通过 `(Mutex<usize>, Condvar)` 获取并发令牌；达到上限时调用线程阻塞。worker 完成或 panic 后都会释放令牌并唤醒一个等待者。
- `ErrorGroup::go` 为每项工作启动 OS 线程，`wait` join 全部线程后返回共享槽中的第一个错误。它不会在首错出现时取消其他线程，这与“汇聚首错”一致，但不具备 Go `errgroup.WithContext` 的主动取消能力。
- 快照/卷创建和删除使用有界 `WorkerPool`；FSR enable/disable 则直接向 `ErrorGroup` 提交每个批次，因此其并发数不受 `EC2Session.concurrency` 限制，而由 AZ 数和批次数决定。
- `NewEC2Session` 创建一个 Tokio runtime，并让 EC2/CloudWatch adapter 共同持有；session 及两个 adapter 释放最后一个 `Arc` 后 runtime 才销毁。
- 各种 wait 方法使用 `std::thread::sleep` 同步阻塞，没有取消 token、deadline 或 async yield。调用方必须把可能的长时间等待计入任务生命周期设计。
- `DeleteSnapshots`/`DeleteVolumes` 会等待所有清理线程结束才返回，即使单项失败；`EnableDataFSR`/`DisableDataFSR` 也在返回前 join 全部批任务。

## 与 Go 版本的对应关系

本文件直接移植 `br/pkg/aws/ebs.go`，公开流程和关键常量基本逐项对应：

- Go `EC2Session`、`VolumeAZs`、`NewEC2Session` 对应同名 Rust 符号；Rust 额外引入 `Ec2Client`/`CloudWatchClient` trait 和 `NewEC2SessionWithClients` 以支持无真实凭据测试。
- Go `CreateSnapshots`、`createSnapshotsWithRetry`、`WaitSnapshotsCreated`、`DeleteSnapshots` 对应 Rust 同名或 snake_case helper，仍保留按 store 创建、pending-limit 重试、进度增量和 best-effort 删除语义。
- Go `EnableDataFSR`、`waitDataFSREnabled`、`getFSRCreditBalance`、`DisableDataFSR`、`fetchTargetSnapshots` 对应 Rust 的同名公开方法及 snake_case 私有 helper，批大小、credit 时间窗/统计量、轮询间隔和冲突状态一致。
- Go `CreateVolumes`、`WaitVolumesCreated`、`DeleteVolumes`、`HandleDescribeVolumesResponse` 对应 Rust 同名方法；AZ 选择、可选性能参数、标签前缀、FSR 检查和部分结果返回语义一致。
- Go 直接使用 `config.EBSBasedBRMeta` 与 `glue.Progress`；Rust 为避免 crate 环依赖，在本文件定义 serde 兼容元数据和最小 `Progress` trait，由 task 层完成适配。
- Go 使用 AWS SDK 的指针/枚举类型；Rust 用本地精简模型转换 SDK 返回值，未知状态保留在 `Other(String)`。
- Go 使用 `util.WorkerPool`、`errgroup` 和 `atomic.Int32`；Rust 用 `WorkerPool`、`ErrorGroup`、`Mutex<i32>` 实现。两者都会等待已启动任务，但 Rust 实现没有 context 取消。
- Go 的生产日志没有在 Rust 中逐项移植；错误字符串和返回形状承担主要诊断信息。Rust 对 AWS 空数组响应增加了显式检查，避免 Go 版索引访问可能产生的 panic。

测试对应关系：`br/pkg/aws/ebs_test.go` 的进度解析、volume 状态拆分和 snapshot 完成/失败/pending 场景由 `br/pkg/aws/ebs_test.rs` 对齐；Rust 还验证 SDK service error metadata 和空 AWS ID 保留。`br/pkg/aws/parity_test.rs::go_rust_public_contract_matches` 串联重试、FSR 空输入/校验、快照与卷创建清理、标签相关恢复以及进度行为。

## 扩展指南

- 新增 AWS API 时，先把最小调用面加入 `Ec2Client` 或 `CloudWatchClient`，再在 `AwsEc2Client`/`AwsCloudWatchClient` 做 SDK 类型转换；同步扩展 `br/pkg/aws/ebs_test.rs` 的 stub 和 `br/pkg/aws/parity_test.rs` 的内存实现，避免测试依赖真实 AWS。
- 新增或调整 EBS 元数据字段时，修改对应 serde 模型并核对 `br/pkg/config` 的 Go JSON 形状及 `restore_ebs_meta.rs` 的反序列化/回写逻辑；不要只改本地结构名。
- 修改快照创建筛选时，应集中在 `CreateSnapshots` 的 block-device 遍历与 `InstanceSpecification` 构造处，并至少覆盖 root volume、目标数据盘、额外数据盘、无 attachment、空 AWS 返回等独立测试。
- 修改重试/轮询策略时，优先扩展 `EC2SessionTimers`，保留生产默认值与测试可注入性；若增加超时或取消，要同时说明与 Go 行为的差异，并检查 task 层清理路径。
- 修改 FSR 行为时，保持 `FsrApiSnapshotsThreshold` 的 AWS 限制、按 AZ 分组以及 unsuccessful item 的处理；应覆盖恰好 10/超过 10 个快照、多个 AZ、credit 为 0/None/错误、disabled/disabling/enabling/optimizing 状态。
- 修改建卷标签时，维护三类固定标签和“仅复制非 `snapshot/` 源标签”的不变量，避免标签递归增长；测试应记录传给 `CreateVolume` 的完整 tags，而不只是确认调用成功。
- 修改并发实现时，验证令牌在成功、错误和 panic 三条路径都释放，并明确是否仍需“等待全部、返回首错”以及是否要为 FSR 批次施加 `concurrency` 上限。
- 新增测试必须继续放在独立文件 `br/pkg/aws/ebs_test.rs` 或 `br/pkg/aws/parity_test.rs`，不要把 Rust 测试嵌入 `ebs.rs`。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 7,032 个 Rust 文件；`br/pkg/aws/ebs.rs` 被识别为 91 个符号。使用了 `status`、`files --filter br/pkg/aws`、`explore`、`query` 与按行 `node` 检查。
- RustCodeGraph 关键边：`NewEC2Session` 的生产 Rust 调用者为 `br/pkg/task/restore_ebs_meta.rs::doRestore`；图中还确认了 `CreateSnapshots -> create_snapshots_with_retry`、`EnableDataFSR -> fetch_target_snapshots/wait_data_fsr_enabled`、`CreateVolumes -> DescribeSnapshots/CreateVolume`、等待方法到 Describe API/`Progress::IncBy` 的关系。
- 完整阅读的目标与模块文件：`br/pkg/aws/ebs.rs`、`br/pkg/aws/lib.rs`、`br/pkg/aws/Cargo.toml`。
- 直接入口证据：`br/pkg/task/restore_ebs_meta.rs` 与 `br/pkg/task/Cargo.toml`；源码搜索确认恢复 API 的生产调用点，并确认快照生命周期 API 当前没有排除测试后的 Rust 外部调用点。
- Go 对照：`br/pkg/aws/ebs.go`，核对了全部同名公开流程、常量、轮询时间、错误条件和标签规则。
- 测试证据：`br/pkg/aws/ebs_test.rs`、`br/pkg/aws/parity_test.rs`、`br/pkg/aws/ebs_test.go`。这些文件覆盖进度解析、快照状态、AWS 错误 metadata、volume 状态/FSR 校验、pending-limit 重试、清理与公开契约串联。
- 本任务只新增文档，未运行 Cargo 或 AWS 集成调用。文档结构通过任务指定命令检查，章节数量必须恰为 11；事实复核使用本节所列静态源码、调用图、Cargo 和测试证据。
