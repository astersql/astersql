# `br/pkg/restore/log_client/flow_control.rs`

## 文件定位

本文件属于 `astersql-br-pkg-restore-log-client` library crate；crate 根 `br/pkg/restore/log_client/lib.rs` 通过 `pub mod flow_control` 挂载它。它位于压缩 SST 恢复的入口与真正导入之间：`LogClient::RestoreSSTFileSets` 在切换 TiKV import mode、调用 `GoRestore` 之前，先调用 `LogClient::adjustTiKVFlowControlForCompactedSSTRestore`。因此这里不是通用限速器，而是为 compacted-SST restore 预先放宽 TiKV pending-compaction 软/硬阈值的保护步骤。

`br/pkg/restore/log_client/Cargo.toml` 将该目录声明为独立 crate，`[lib] path = "lib.rs"`，并通过 `astersql-br-pkg-restore` 获得 `BatchBackupFileSet`。该目录没有 `doc.go`；Rust crate 入口 `lib.rs` 和 Go 同路径实现 `flow_control.go` 是最近的职责定义。

## 核心职责

文件围绕三个阶段工作：

1. `estimateCompactedSSTFlowControl` 根据 snapshot 恢复量、checkpoint 已计入量、新增 SST 物理大小、TiKV store 数和副本数，估计单 store 的 L6、L5 数据量以及待 compact 字节数。
2. `compactedSSTFlowControlTarget` 结合估计值和各 TiKV 实例的现有配置，计算只增不减的 soft/hard 目标值；soft 至少 1 TiB，hard 至少 2 TiB 且至少为 soft 的两倍。
3. `adjustTiKVFlowControlForCompactedSSTRestore` 通过受限 SQL 读取全体 TiKV 配置，仅在估计 pending bytes 超过 100 GiB 且至少一个实例配置不足时执行全局 `SET CONFIG`，并固定先写 hard、后写 soft。

该逻辑只负责恢复前的配置放宽，不负责恢复完成后回滚原配置，也不拥有 SST 导入、模式切换或 worker 生命周期。

## 主要符号

- `tikvSoftPendingCompactionBytesLimit`、`tikvHardPendingCompactionBytesLimit`：两项 TiKV 配置的真实名称；查询和更新都复用这些常量。
- `compactedSSTFlowControlPendingThreshold`：100 GiB 的启用阈值；`pendingBytes <= threshold` 时不调整。
- `TiKVConfigValue { instance, value }`：`SHOW CONFIG` 每个 TiKV 实例的名称和值。`instance` 当前只保存证据，不参与目标计算。
- `CompactedSSTFlowControlConfig { soft, hard }`：所有实例的两组原配置。
- `CompactedSSTFlowControlEstimate`：保留输入总量、每 store 的 L6/L5 估算、pending bytes、store 数和副本数，便于测试和后续观测扩展。
- `ceilDiv`、`estimateLevelBytesPerStore`：向上均摊后乘有效副本数；有效副本数取 `min(replicas, stores)`，乘法饱和。
- `estimatePendingCompactionBytes`：按 `ratio = L6/L5` 和 `(L5 - L6/10) * (ratio + 1)` 估算 pending bytes，并把越界结果钳制到 `u64::MAX`。
- `estimateCompactedSSTFlowControl`：遍历 `BatchBackupFileSet[*].SSTFiles`；单文件优先用 `Size_`，为零时回退到 `TotalBytes`，并把 checkpoint 字节以饱和加法纳入 compacted 总量。
- `parseByteSizeConfig`：解析 TiKV 配置的人类可读容量；支持 B/K/KB/KiB 到 P/PB/PiB、大小写和指数数值，按 1024 进位，并拒绝负数、非有限数、尾随/多余空格和达到 `i64::MAX` 的结果。
- `maxTiKVConfigBytes`、`allTiKVConfigsAtLeast`：前者忽略不可解析项并取可解析最大值；后者要求列表非空、每项可解析且均不小于目标。
- `compactedSSTFlowControlTarget`：生成 soft/hard 目标；使用饱和加乘防止极值溢出。
- `formatBytes`：只在能整除时选 TiB/GiB/MiB/KiB，否则输出字节数，保证写回值无小数。
- `getTiKVConfigValues`、`setTiKVConfig`：分别封装 `SHOW CONFIG ... type = 'tikv'` 与 `SET CONFIG tikv`。
- `LogClient::adjustTiKVFlowControlForCompactedSSTRestore`：唯一面向恢复主链的编排入口。

## 执行流程

`LogClient::RestoreSSTFileSets` 已先排除空文件集和取消状态，然后进入本文件的方法：

1. 若 `unsafeSession`、`sstRestoreManager` 不存在，或 manager 的 `storeCount`/`replicaCount` 为零，直接成功返回；这些状态不足以形成可靠估算或执行配置 SQL。
2. 先查询 soft，再查询 hard。任何 SQL/行转换错误立即返回；任一结果为空则跳过调整。
3. `estimateCompactedSSTFlowControl` 从 checkpoint 开始累加新 compacted SST 大小；分别估算 snapshot 对应 L6 和 compacted SST 对应 L5 的单 store 数据量，再计算 pending bytes。
4. pending bytes 不超过 100 GiB 时结束，不触碰配置。
5. 计算目标值。若 soft 的每个实例都已达到 soft 目标，且 hard 的每个实例都已达到 hard 目标，则结束。
6. 先设置 hard，再设置 soft。hard 写失败时不会尝试 soft；hard 成功而 soft 失败时错误上抛，但 hard 已经生效，没有事务性回滚。
7. 返回后，`RestoreSSTFileSets` 才会按 `online` 决定是否切换 import mode，并调用 restorer 导入 SST。`flow_control_test.rs::restore_sst_pipeline_adjusts_configs_before_mode_switch_and_import` 验证配置写入先于 import。

## 数据与状态

估算数据全部是函数内值，不在 `LogClient` 中缓存。`snapshotRestoreBytes` 原样作为 L6 总量；`compactedSSTBytes` 是 checkpoint 与当前批次物理大小的饱和和。每 store 估算使用 `ceil(total/storeCount) * min(replicaCount, storeCount)`，所以副本数超过 store 数不会进一步放大。

配置读取结果按 TiKV 实例保存。计算目标时用各组中“可解析值的最大值”作为下界，以避免降低已有最高配置；是否跳过写入则要求“每一个实例”都达到目标。由此，混合配置如 `4TiB` 与 `192GiB` 会触发全局设置，使所有实例对齐到不低于现有最大值的目标。

本文件产生的唯一外部持久状态是 TiKV 集群配置。它没有保存原值，也没有恢复钩子；调用成功后配置继续保留。`instance` 字段没有用于逐实例写入，因为 `SET CONFIG tikv` 面向所有 TiKV。

## 依赖与调用关系

上游直接调用者是 `br/pkg/restore/log_client/client.rs` 中的 `LogClient::RestoreSSTFileSets`。RustCodeGraph 同时显示目标文件被 `client.rs`、独立测试 `flow_control_test.rs` 以及 snap-client 的 import 文件/测试引用；对本文件入口的源码核验确认生产调用位于 `client.rs:1047`。

下游依赖包括：

- `crate::client::LogClient` 及其 `unsafeSession`、`sstRestoreManager.storeCount`、`replicaCount` 状态。
- `crate::stubs::glue::{Session, SqlArg}`：`Session::ExecRestrictedSQL` 承担查询和设置配置；返回行的第 1、3 列分别解释为 instance 和 value。
- `crate::stubs::{Context, Error, Result}`：统一取消上下文与错误类型。
- `astersql_br_pkg_restore::BatchBackupFileSet`：恢复文件集合；来自 `Cargo.toml` 的路径依赖 `astersql-br-pkg-restore = { path = ".." }`。

RustCodeGraph 的 callee 结果确认主入口调用 `getTiKVConfigValues`、`estimateCompactedSSTFlowControl`、`compactedSSTFlowControlTarget`、`allTiKVConfigsAtLeast`、`formatBytes` 和 `setTiKVConfig`；估算函数继续调用 `estimateLevelBytesPerStore` 与 `estimatePendingCompactionBytes`。

## 错误处理与边界

- 缺 session、manager、store/replica 计数、任一配置项集合，或估计量不超过阈值，均是可接受的跳过条件并返回 `Ok(())`。
- `getTiKVConfigValues` 对 SQL 错误、缺列、非 UTF-8 bytes 都返回错误；它按固定列下标读取，因此依赖 `SHOW CONFIG` 行形状。
- `setTiKVConfig` 给底层错误附加配置名和值；测试验证写阶段错误含 `failed to set config`，且后续写入不会继续。
- `maxTiKVConfigBytes` 会静默忽略非法字符串，但 `allTiKVConfigsAtLeast` 会把非法字符串视为“不满足”，从而触发统一写入。若一整组值均非法，目标下界退回 floor/估算值。
- 所有容量汇总和目标放大使用饱和运算；浮点 pending 结果小于等于零归零，超过范围钳制为 `u64::MAX`。浮点转整数会截断小数部分，这是与 Go 转 `uint64` 对齐的行为。
- `parseByteSizeConfig` 的上界按 Docker `RAMInBytes` 所返回的有符号范围模拟，而非接受整个 `u64` 范围。
- 两次 `SET CONFIG` 不是原子操作：soft 写失败可能留下只提高 hard 的部分成功状态。写 hard 在先可避免反向顺序短暂形成 soft 高于 hard 的危险窗口。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁或事务；计算函数是纯函数。`adjustTiKVFlowControlForCompactedSSTRestore` 借用 `&self`、`&Context`、文件集和 `Session`，不取得这些对象的所有权，也不负责关闭 session 或 manager。

外部配置是集群共享状态，因此多个恢复任务并发调用时可能交错执行“读旧值—算目标—全局写入”。实现通过目标不低于当前可见最大值降低主动降配风险，但没有 compare-and-set、锁或事务保证，也不会重读确认最终值；扩展并发策略时必须考虑读写竞态和部分写成功。取消与超时语义完全由传入 `Context` 和 `Session::ExecRestrictedSQL` 实现承担。

## 与 Go 版本的对应关系

Rust 文件直接对应 `br/pkg/restore/log_client/flow_control.go`，主要公式、阈值、配置键、Size_ 优先规则、饱和运算、跳过条件以及“先 hard 后 soft”的顺序一致。Go 的 `compactedSSTSizeForFlowControl`、`saturatingAddUint64`、`saturatingMulUint64`、`ceilDivUint64` 在 Rust 中分别内联为 fold 分支、标准 `saturating_add`/`saturating_mul` 和 `ceilDiv`。

当前可观察差异如下：

- Go 会为跳过、解析失败、估算结果和成功调整记录结构化日志；Rust 当前没有对应日志函数，`TiKVConfigValue.instance` 因而也未用于诊断。
- Go 通过 `kv.WithInternalSourceType(ctx, kv.InternalTxnBR)` 标记内部 SQL 来源；Rust 的本地 `Session` 接口直接接收 `Context`，未显式附加该标记。
- Go 使用 `docker/go-units` 的 `RAMInBytes`/`FromHumanSize`；Rust 是本地兼容解析器。`flow_control_test.rs::byte_size_config_preserves_docker_units_numeric_and_spacing_rules` 专门锁定指数与空格规则。
- Go 类型和辅助函数多为包内私有；Rust 因分文件测试和 crate API 组织暴露了若干 `pub` 符号。
- Go 独立测试入口位于 `export_test.go`，实际断言位于 `client_test.go::TestEstimateCompactedSSTFlowControl`、`TestEstimatePendingCompactionBytes` 和 `TestCompactedSSTFlowControlTarget`；Rust 对应测试集中在独立文件 `flow_control_test.rs`。

## 扩展指南

- 修改估算模型时，从 `estimateCompactedSSTFlowControl`、`estimateLevelBytesPerStore` 和 `estimatePendingCompactionBytes` 入手，并同步 `flow_control_test.rs` 中物理大小回退、checkpoint、零 store、副本上限和极值饱和用例；同时逐项对照 Go 公式，不能为测试方便简化。
- 修改配置目标或阈值时，更新 `compactedSSTFlowControlTarget` 与常量，并覆盖 floor、现有较高配置、混合实例值、`u64::MAX` 和 100 GiB 边界。
- 扩展容量语法时，以 Go `docker/go-units` 行为为兼容契约，先为 `parseByteSizeConfig` 在独立测试文件加入有效/无效输入，尤其关注空格、指数、负数、溢出和大小写。
- 改动 SQL 列或 Session 抽象时，同步 `getTiKVConfigValues`、`sqlString`、`setTiKVConfig` 及 `ConfigSession` 测试桩；保留参数绑定，不把配置值拼接进 SQL。
- 若增加恢复后回滚、幂等确认或并发协调，应在 `LogClient` 的恢复生命周期层设计明确所有权，并增加部分写失败、取消和并发恢复测试；不能仅在本文件末尾隐式恢复，因为当前调用方在返回后还有模式切换和异步导入阶段。
- 任何行为改动都应同时核对 `client.rs::RestoreSSTFileSets` 的调用顺序，以及 Go `flow_control.go`/`client.go`；Rust 测试必须继续放在 `flow_control_test.rs`，不要内嵌到生产文件。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter br/pkg/restore/log_client` 定位目标及相邻文件；`node --file br/pkg/restore/log_client/flow_control.rs` 读取完整 257 行和四个引用文件；`query` 核对主入口、估算、解析、目标计算及 SQL helper；`callees` 核对主入口和关键纯函数的下游边。
- Rust 生产代码：`br/pkg/restore/log_client/flow_control.rs`、`client.rs:1029-1069`、`lib.rs`。
- crate 边界：`br/pkg/restore/log_client/Cargo.toml`；未发现该包的 `doc.go`。
- Rust 独立测试：`br/pkg/restore/log_client/flow_control_test.rs`，覆盖估算与饱和、目标值、所有跳过条件、配置读取/写入顺序、读写错误短路、恢复流水线顺序、store/replica 初始化及 Docker units 兼容规则。
- Go 对照：`br/pkg/restore/log_client/flow_control.go`、`client.go:370-393`、`export_test.go:139-177`、`client_test.go:1984-2049`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令确认目标存在且恰有 11 个固定二级章节，并人工复核唯一生产物与 Git 暂存范围。
