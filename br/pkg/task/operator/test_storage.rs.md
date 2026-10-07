# `br/pkg/task/operator/test_storage.rs`

## 文件定位

本文件实现 BR `test-storage` 运维子命令的核心探测逻辑，是 Go 文件 [`test_storage.go`](./test_storage.go) 的 Rust 移植。它属于 Cargo 包 `astersql-br-pkg-task-operator`（见 [`Cargo.toml`](./Cargo.toml)），由 [`lib.rs`](./lib.rs) 以 `pub mod test_storage` 挂载并通过 `pub use test_storage::*` 平铺公开。直接命令入口是 [`br/cmd/br/operator.rs`](../../../cmd/br/operator.rs) 的 `newTestStorageCommand`：注册参数、调用 `TestStorageConfig::ParseFromFlags`，再调用 `RunTestStorage`。

虽然文件名含 `test_storage`，它不是 Rust 单元测试文件，而是可由 BR CLI 调用的生产候选模块，用于在备份/恢复前主动检查外部存储配置、权限及接口语义。真正的独立 Rust 测试位于 [`test_storage_test.rs`](./test_storage_test.rs) 与 [`parity_test.rs`](./parity_test.rs)。

## 核心职责

- `DefineFlagsForTestStorageConfig` 与 `TestStorageConfig::ParseFromFlags` 定义并解析后端参数、存储 URI、成功后清理开关、失败暂停开关及测试数据大小。
- `RunTestStorage` 通过 `ParseBackend`、`NewStorage` 创建 `ExternalStorage`，生成随机载荷，然后按固定顺序探测对象写入、存在性、完整读取、流式读取、范围读取、流式创建、重命名、目录遍历、分页遍历、单删与批删。
- `TestResult`、`TestReport`、`TestContext::AddResult` 把每一步转为可打印的通过/失败记录，并在交互调试模式下于失败后等待用户输入。
- `formatBytes`、`printStep`、`printSuccess`、`printError` 只负责终端呈现，不改变存储判定。

该模块验证的是一次探针运行中的接口行为，不是长期健康检查：它不重试、不并行执行、不保存结构化报告，也不会回滚所有可能的部分写入。

## 主要符号

- 常量 `testFileName1`、`testFileName2`、`testFileNameRenamed`、`testDirName` 固定本次探针使用的对象键；`defaultTestDataSize` 为 1 MiB。
- `TestResult { Name, Passed, Duration, Details, Error }` 表示单一步骤。`Error` 仅在失败时携带底层错误或语义不匹配说明。
- `TestReport` 保存规范化存储 URI、开始/结束时间、总数/成功数/失败数、逐步结果与近似流量。`TestReport::AddResult` 是计数不变量的唯一维护点；每加入一个结果，`TotalTests` 增一且成功/失败计数之一增一。
- `TestStorageConfig` 是公开配置；`ParseFromFlags` 拒绝空 URI 和非正 `TestDataSize`，但没有保证数据至少达到范围测试所需的 200 字节。
- `ArcStorage = Arc<dyn ExternalStorage>` 使运行上下文与清理闭包共享同一存储对象。
- `TestContext` 聚合报告、存储句柄及 `PauseWhenFail`；其 `AddResult` 先记账，再按失败状态选择是否阻塞读取 stdin。
- `RunTestStorage(TestStorageConfig) -> Result<()>` 是公开业务入口。
- `fill_random` 从 `/dev/urandom` 填满测试数据；`fill_random_from_reader` 抽出可注入 `Read` 的核心，供独立错误传播测试调用。
- `testWriteFile`、`testFileExists`、`testReadFile`、`testOpen`、`testOpenWithRange`、`testCreate`、`testRename`、`testWalkDir`、`testWalkDirWithSubDir`、`testWalkDirWithPagination`、`testDeleteFile`、`testDeleteFiles` 是私有顺序步骤。

## 执行流程

1. `newTestStorageCommand` 创建无位置参数的 `test-storage` 命令，`DefineFlagsForTestStorageConfig` 注册参数；执行时先由 `ParseFromFlags` 校验配置。
2. `RunTestStorage` 建立带开始时间的 `TestReport`，用 `ParseBackend` 解析 URI，再以 `StorageOptions { SendCredentials: true, .. }` 调用 `NewStorage`。后端创建成功后，以 `store.URI()` 覆盖报告中的 URI。
3. 按 `TestDataSize` 分配缓冲区，`fill_random` 用系统随机源执行 `read_exact`；随机源打开或读取失败会在任何存储步骤前直接返回。
4. 固定步骤链为：`WriteFile(file1)` → 验证存在 → `ReadFile` 校验长度与逐字节内容 → `Open` 校验文件大小并流式读完 → `[100, 200)` 范围读并逐字节校验 → `Create(file2)` 以 64 KiB 分块写入并关闭 writer → 重命名为 `renamed` 并核对旧/新键 → 根目录 Walk → 写入两个子目录对象并 Walk → 创建十个对象、以 `ListCount=3` Walk 并检查数量与唯一性 → 删除 `file1` → 批删 `renamed` → 再确认 `file1` 不存在。
5. 每一步自行计时、构造一个 `TestResult`，最终调用 `TestContext::AddResult`；某一步失败通常不会阻止后续步骤，因此后续失败可能是前序失败的连锁结果。
6. 主流程设置结束时间和近似 `TotalBytes = testData.len() * 4`，打印报告，执行尽力而为的清理并关闭存储。若报告含任一失败项，则最终返回 `storage test failed`。

## 数据与状态

运行期间的可变状态集中在 `TestContext.Report`。测试数据只生成一次并由各步骤借用；完整读取和范围读取会创建新的返回缓冲区。`TotalBytes` 不是实际 I/O 计量，而是以“写、读、流式创建、范围读”四份完整载荷粗略估算，因此既忽略子目录/分页对象，也把固定 100 字节范围读按整份载荷计算。

对象状态依赖步骤顺序：`file1` 是整读、流读和范围读的基准；`file2` 写入后立刻重命名；分页对象仅在分页步骤内部记录名称并尝试删除。子目录步骤实际创建 `br-test-dir/br-test-file-1.tmp` 与 `br-test-dir/file2.tmp`。主清理闭包却尝试删除 `br-test-dir/br-test-file-1.tmp` 与 `br-test-dir/br-test-file-2.tmp`，因此 `br-test-dir/file2.tmp` 不在其删除集合内，这是当前代码事实。

`CleanupOnSuccess` 的实际语义是“主流程走到报告阶段时是否执行清理”，不是“仅在全部测试通过时清理”：清理发生在失败计数转成返回错误之前。解析后端、创建存储或生成随机数提前失败时，清理闭包尚未执行或主流程会提前返回。

## 依赖与调用关系

上游调用链为 `br/cmd/br/operator.rs::newTestStorageCommand` → `TestStorageConfig::ParseFromFlags` → `RunTestStorage`。模块入口 [`lib.rs`](./lib.rs) 将这些公开符号再导出到 operator crate 根。RustCodeGraph 的文件关系显示本文件还被 [`parity_test.rs`](./parity_test.rs) 和 [`test_storage_test.rs`](./test_storage_test.rs) 使用。

下游主要来自 [`stubs.rs`](./stubs.rs)：`BackendOptions`/`FlagSet` 承担配置，`ParseBackend`/`NewStorage` 创建存储，`ExternalStorage` 及其 reader/writer 抽象提供 I/O，`ReaderOption` 与 `WalkOption` 描述范围读取和遍历参数，`Error`/`Result` 统一错误传递。Cargo 清单声明本 crate 直接依赖 `astersql-objstore`，但该文件通过本 crate 的 `stubs` 适配层使用存储能力；同时依赖标准库的 `Arc`、`Read`/`Write`、`Path` 与时间类型。

RustCodeGraph 对 `RunTestStorage` 的被调边确认了全部私有测试步骤、`fill_random`、`TestReport::Print` 和打印辅助函数；对 `testWriteFile` 等步骤的被调边确认它们统一实例化 `TestResult` 并汇入 `TestContext::AddResult`。由于 Go/Rust 同名符号并存，按名称查询 callers 会产生歧义，CLI 上游关系以模块导入和 `operator.rs` 源码交叉核验。

## 错误处理与边界

- 后端解析和存储创建失败分别增加 `failed to parse storage backend`、`failed to create external storage` 上下文并立即返回；此时没有报告汇总。
- 随机源错误由 `fill_random_from_reader` 标注为 `failed to generate test data` 并向上传播；[`test_storage_test.rs`](./test_storage_test.rs) 使用始终失败的 reader 验证该错误链。
- 存储步骤把底层错误写入 `TestResult.Error`，继续执行其余步骤，最终统一返回 `storage test failed`。语义错误（长度、内容、存在性、数量、重复键）由本文件主动构造。
- `testOpen` 的成功判定只比较返回长度，不逐字节核对内容；完整内容正确性由之前的 `testReadFile` 承担。范围读取则同时核对长度和内容。
- 配置只要求 `TestDataSize > 0`，但 `testOpenWithRange` 会索引 `expectedData[100..200)` 的逻辑位置；当后端确实返回 100 字节而测试数据短于 200 字节时，Rust 可能越界 panic。Go 对照同样固定使用 `[100, 200)`，且子目录/分页步骤还会固定切 `[:512]`、`[:256]`、`[:1024]`；Rust 仅对子目录和分页切片使用 `min` 做了保护。
- `testCreate` 假设 writer 每次成功写入至少一个字节；若某实现返回 `Ok(0)`，循环不会前进。
- 清理与 `Close` 的错误被刻意忽略，因而不会出现在报告中；这意味着命令成功不证明临时对象全部删除或关闭过程成功。

## 并发与资源生命周期

本文件没有创建线程、异步任务、锁或通道，所有探测串行运行。`ArcStorage` 的用途是共享所有权而非并发调度：`TestContext` 和清理闭包各持有一个 `Arc`。

reader 在 `testOpen`/`testOpenWithRange` 的正常和大部分错误分支显式 `Close`；writer 在写失败时尝试关闭，在成功时必须关闭成功才将步骤标记为通过。主存储在报告与清理之后调用 `store.Close()`，但不检查关闭错误。

分页对象在完成 Walk 后逐个尽力删除；然而创建第 N 个对象失败会在进入删除循环前返回，已创建对象可能残留。子目录第二次写入失败时，第一个对象也会保留到主清理阶段；结合键名不一致，`file2.tmp` 仍可能残留。`PauseWhenFail` 会同步阻塞当前线程读取 stdin，适合人工排障，不适合无人值守执行。

## 与 Go 版本的对应关系

Rust 的常量、报告字段、参数名、主步骤顺序、64 KiB 流式写块、范围 `[100, 200)`、分页数量 10/页大小 3、错误文案和最终失败策略都直接对应 [`test_storage.go`](./test_storage.go)。[`br/cmd/br/operator.go`](../../../cmd/br/operator.go) 与 Rust `operator.rs` 也都把该逻辑挂到 `test-storage` 子命令。

主要实现差异如下：

- Go 通过 `context.Context` 调用每个存储接口；Rust `RunTestStorage` 无 context 参数，使用同步 `ExternalStorage` trait。
- Go 使用 `crypto/rand.Read`；Rust直接打开 Unix `/dev/urandom`，因此当前实现具有平台假设。
- Go 通过 `defer store.Close()` 与 `defer cleanup()` 管理退出；Rust在正常主流程尾部显式执行。当前 Rust 的各探测步骤不会用 `?` 提前返回，但未来若在清理前增加早退，需要额外保护资源生命周期。
- Rust 对子目录和分页载荷切片使用 `min`，比 Go 固定切片更能容忍小数据；范围校验仍保留与 Go 相同的最小长度隐含前提。
- Go 报告打印本地格式化时间；Rust `format_time` 只输出 Unix 秒数。两者展示不同，但不影响存储判定。

## 扩展指南

新增存储能力探测时，应在 `RunTestStorage` 中把新步骤放到满足对象状态前提的位置，新增私有步骤函数，并确保无论成功还是失败都恰好调用一次 `TestContext::AddResult`。若创建新临时键，必须同时扩充正常、部分失败和最终清理路径；优先把键集合集中管理，避免当前子目录键名不一致的问题。

新增范围或切片场景时，应同步强化 `ParseFromFlags` 的最小数据长度约束，或让步骤按实际数据长度选择合法区间，防止 panic。新增 writer 循环应处理零进展写入。若要把报告用于机器消费，应新增独立的结构化输出层，不能解析带颜色的人类文本。

测试逻辑必须继续放在独立文件：低层错误注入放入 [`test_storage_test.rs`](./test_storage_test.rs)，公开契约、配置边界和 `MemStorage` 全流程放入 [`parity_test.rs`](./parity_test.rs)。涉及 Go 对齐时同步检查 [`test_storage.go`](./test_storage.go) 及 Go CLI 入口。真实云后端的凭证、权限、分页一致性和 rename 支持仍需集成环境验证，不能由 `MemStorage` 用例替代。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter br/pkg/task/operator/test_storage.rs` 确认目标文件已索引。
- RustCodeGraph `node --file ... --offset 1/500`：读取并核对目标文件 1–1207 行及 42 个符号；文件关系标明由 `parity_test.rs`、`test_storage_test.rs` 使用。
- RustCodeGraph `query TestStorage`、`callees RunTestStorage`、`callees testWriteFile`、`callees testWalkDirWithPagination`：核对公开类型、主流程步骤及结果汇总调用边。因 Go/Rust 同名定义导致按名称的 callers 查询无明确输出，调用上游另以源码验证。
- crate 与入口证据：[`Cargo.toml`](./Cargo.toml)、[`lib.rs`](./lib.rs)、[`br/cmd/br/operator.rs`](../../../cmd/br/operator.rs)。
- Go 对照证据：[`test_storage.go`](./test_storage.go)、[`br/cmd/br/operator.go`](../../../cmd/br/operator.go)。
- 测试证据：[`test_storage_test.rs`](./test_storage_test.rs) 验证随机源错误传播；[`parity_test.rs`](./parity_test.rs) 验证报告计数、flag 解析、空 URI 错误和 `MemStorage` 全流程。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令验证文档恰含十一个固定二级标题，并人工检查所有关键结论均可回溯到上述符号或文件。
