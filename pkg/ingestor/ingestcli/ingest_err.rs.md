# `pkg/ingestor/ingestcli/ingest_err.rs` 逻辑说明

## 文件定位

`ingest_err.rs` 属于 `astersql-ingestor-ingestcli` crate。crate 根 `pkg/ingestor/ingestcli/lib.rs` 将它声明为私有模块 `ingest_err`，再用 `pub use ingest_err::*` 重导出公开类型和函数。`pkg/ingestor/ingestcli/Cargo.toml` 的 `package.metadata.porting.go-package` 指向同路径 Go 包，说明这里是 `pkg/ingestor/ingestcli/ingest_err.go` 的 Rust 移植面。

该 crate 位于 SST 导入链的 TiKV HTTP 客户端边界。包契约 `pkg/ingestor/doc.go` 把 `pkg/ingestor` 定义为直接向底层存储导入 SST 并协助排序、Region 切分/散射及 import mode 准备的组件；具体到本文件，它不发送请求，而负责把“HTTP 传输已经完成、TiKV 响应体却携带 Region 逻辑错误”的结果解码并归类。当前 Rust 生产入口是 `pkg/ingestor/ingestcli/client.rs::ClientImpl::ingest`：非 200 响应先调用 `decode_error_pb`，再调用 `NewIngestAPIError`，最后包装成 `client::Error::IngestApi`。

## 核心职责

本文件有三层职责：

1. 用 `ErrorPb` 保存 ingest 重试决策需要的 `errorpb.Error` 子集，并用 `decode_error_pb` 从 protobuf wire bytes 解析该子集。
2. 用 `NewIngestAPIError` 按固定优先级把 TiKV 错误映射到 `astersql_ingestor_errdef::NormalizedError`，保留稳定 RFC code 和响应细节。
3. 用 `getIngestFailedMsg` 为未单独分类的 errorpb 变体补上类型名，避免 TiKV 原始 `message` 为空时丢失诊断信息。

它不负责判断下一阶段究竟重试 ingest、重写还是重新扫描 Region；它只输出分类与可选的新 Region。Go 上层会消费这些类别决定阶段，当前 Rust 生产代码则只在 ingestcli 内生成并传播该错误，尚未发现 Rust 生产调用者传入 Region 提取回调。

## 主要符号

- `ErrorPb`：公开的解码结果。除 `message: String` 和 `current_regions: Vec<crate::Region>` 外，其余字段均为“对应嵌套消息是否出现”的布尔标志。它只保存分类所需信息，不是完整的 kvproto `errorpb.Error` 镜像。
- `CategorizedError`：由 `category: errdef::NormalizedError` 与 `detail: String` 组成。`is` 通过 `RFCCode()` 比较类别；`Display` 在 detail 非空时输出“类别: 细节”；它也实现 `std::error::Error`。
- `IngestAPIError`：公开错误包装，包含 `err: CategorizedError` 和 `new_region: Option<crate::RegionInfo>`。`Display` 委托给内层错误，`source`、`Cause`、`Unwrap` 都暴露内层分类错误，分别服务 Rust 错误链与 Go 风格兼容接口。
- `RegionExtractFn`：`dyn Fn(&[crate::Region]) -> Option<crate::RegionInfo>` 的公开类型别名，仅在 EpochNotMatch 分支使用。
- `NewIngestAPIError`：公开分类入口，保持 Go `switch` 的分支优先级，并只在 EpochNotMatch 时调用可选的 Region 提取函数。
- `getIngestFailedMsg`：公开的兜底消息构造器，为 15 类未独立映射的 errorpb 变体生成类型名前缀。
- `decode_error_pb`：crate 内可见的 wire 解码入口；直接生产调用点位于 `client.rs::ClientImpl::ingest`，测试也通过同 crate 私有路径调用它。
- `decode_epoch_not_match`、`decode_region`、`decode_epoch`、`decode_peer`：私有的嵌套消息解码链。
- `fields`、`decode_varint_value`、`decode_varint`、`invalid_wire`：私有 protobuf 基础解析与统一畸形响应错误构造函数。

本文件没有模块级常量、trait、条件编译项或内部测试模块；测试独立放在 `pkg/ingestor/ingestcli/ingest_err_test.rs`，并由 `lib.rs` 的 `#[cfg(test)]` 模块声明接入。

## 执行流程

当前生产流程从 `pkg/ingestor/ingestcli/client.rs::ClientImpl::ingest` 开始：

1. HTTP POST `/ingest_s3` 返回 200 时直接成功，不进入本文件。
2. 非 200 时，`decode_error_pb(&response.body)` 调用 `fields` 遍历 protobuf 字段。字段 1 以 UTF-8 解码为 `message`；字段 2—22 中已知的 length-delimited 子消息被转换为布尔标志，字段 5 还进入 `decode_epoch_not_match` 解出 `current_regions`。
3. 解码失败时，`client.rs` 不生成 `IngestAPIError`，而是把错误改写为含 HTTP status 的 `client::Error::HttpStatus`，消息为 `failed to unmarshal error response: ...`。
4. 解码成功后，调用者在 `ErrorPb.message` 末尾追加 `(ingest SST ID <id>)`，再以 `extract_region_fn = None` 调用 `NewIngestAPIError`。
5. `NewIngestAPIError` 按以下优先级只选择首个匹配类别：`not_leader` → `epoch_not_match` → message 包含 `raft: proposal dropped` → `server_is_busy` → `region_not_found` → `read_index_not_ready` → `disk_full` → `ErrKVIngestFailed`。
6. 前七个专门类别直接采用 `ErrorPb.message` 作为 detail；兜底 `ErrKVIngestFailed` 调用 `getIngestFailedMsg`。最终结果经 `From<IngestAPIError>` 转成 `client::Error::IngestApi` 返回。

`getIngestFailedMsg` 也按固定顺序只选首个布尔标志，依次识别 `KeyNotInRegion`、`StaleCommand`、`StoreNotMatch`、`RaftEntryTooLarge`、`MaxTimestampNotSynced`、`ProposalInMergingMode`、`DataIsNotReady`、`RegionNotInitialized`、`RecoveryInProgress`、`FlashbackInProgress`、`FlashbackNotPrepared`、`IsWitness`、`MismatchPeerId`、`BucketVersionNotMatch`、`UndeterminedResult`。有类型无 message 时返回类型名；两者都有时以空格拼接；没有已知类型时原样返回 message。

## 数据与状态

所有状态均为单次调用内的拥有值，没有全局缓存或跨请求可变状态。`ErrorPb` 通过 `Default` 初始化，再由解码器逐字段填充；同一已知 protobuf 字段重复出现时，字符串和布尔标志采用后写/置真语义，`epoch_not_match` 的 `current_regions` 会被后出现的字段结果替换。

`current_regions` 使用 `crate::Region`，其实际字段定义在 `pkg/ingestor/ingestcli/interface.rs`：Region ID、起止 key、可选 `RegionEpoch` 和 `Vec<Peer>`；嵌套解码分别保留 epoch 的 `conf_ver/version` 与 peer 的 `id/store_id`。`RegionExtractFn` 可以把这些候选 Region 转成带可选 leader 的 `RegionInfo`，但提取策略与覆盖范围判断不在本文件中。

`CategorizedError.category` 是从 errdef 静态类别克隆出的 `NormalizedError`，类别身份以 RFC code 为准；`detail` 保存本次响应文本。`IngestAPIError.new_region` 默认是 `None`，只有 EpochNotMatch 且调用者提供回调并返回 `Some` 时才有值。当前 `client.rs` 生产调用明确传 `None`，所以当前 Rust HTTP ingest 路径不会填充 `new_region`。

## 依赖与调用关系

直接依赖关系如下：

- `astersql_ingestor_errdef`：由 `Cargo.toml` 以路径依赖 `../errdef` 引入，提供 `NormalizedError` 和 `ErrKVNotLeader`、`ErrKVEpochNotMatch`、`ErrKVRaftProposalDropped`、`ErrKVServerIsBusy`、`ErrKVRegionNotFound`、`ErrKVReadIndexNotReady`、`ErrKVDiskFull`、`ErrKVIngestFailed`。
- `crate::Region`、`RegionEpoch`、`Peer`、`RegionInfo`：定义在 `interface.rs`，供 EpochNotMatch 解码和回调输出使用。
- `crate::Error`：实际为 `client.rs::Error` 的 crate 重导出；本文件用其 `Protobuf(String)` 变体传播 UTF-8 或 wire 格式错误。
- Rust 标准库 `fmt`、`error::Error`：提供展示和错误链接口；本文件无第三方 protobuf runtime 依赖，而是实现所需子集的 wire 解码。

已验证的直接调用边为：`client.rs::ClientImpl::ingest` → `decode_error_pb` → `fields` →（按内容）`decode_epoch_not_match` → `decode_region` → `decode_epoch`/`decode_peer`，随后 `ClientImpl::ingest` → `NewIngestAPIError` →（兜底时）`getIngestFailedMsg`。`NewIngestAPIError` 的 Rust 非测试生产调用仅见 `client.rs`；其余直接 Rust 调用位于 `ingest_err_test.rs`。RustCodeGraph 对目标文件报告 22 个符号，精确文本复核未发现 ingestctrl 的 Rust 生产接线。

## 错误处理与边界

分类边界首先取决于顺序：如果一个 `ErrorPb` 同时置多个标志，`NewIngestAPIError` 与 `getIngestFailedMsg` 都只使用最靠前的匹配项。修改顺序会改变兼容行为，必须视作 API 语义变化。

protobuf 边界由 `fields` 统一约束：field number 不能为 0 或大于 `i32::MAX`；支持 wire type 0、1、2、5；其他 wire type、越界的定长字段、越界的 length-delimited 内容、未终止或超过 u64 容量的 varint 都返回 `crate::Error::Protobuf("malformed errorpb.Error response")`。第十个 varint byte 只能为 0 或 1，以对齐 gogo/protobuf 的整数溢出拒绝行为。字段 1 若不是合法 UTF-8，则保留具体 UTF-8 错误文本。未知字段只要 wire 格式合法就由上层 match 忽略，保留 protobuf 的向前兼容性。

解码器只读取业务所需子集：除 EpochNotMatch 的 Region 列表外，其他嵌套错误的内部字段均不保留；Region 也只对应当前 `interface.rs::Region` 能表达的字段。因而不得把 `ErrorPb` 当作可无损往返编码的通用 protobuf 类型。

`message.contains("raft: proposal dropped")` 是专门类别判定，属于字符串兼容契约；若 TiKV 改变文案，错误会落入其他显式标志或通用 `ErrKVIngestFailed`。`CategorizedError::is` 只比较 RFC code，不比较 detail 或完整结构。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、文件、网络连接或 RAII 资源守卫。所有函数都是同步纯计算：借用输入 byte slice 或 `ErrorPb`，构造拥有型结果后返回；闭包只在 `NewIngestAPIError` 调用期间同步借用。

`RegionExtractFn` 不带 `Send`/`Sync` 约束，因为分类函数不会保存或跨线程调度该闭包。返回的 `RegionInfo` 被移动进 `IngestAPIError`，与输入 slice 不共享借用。`client.rs` 的 HTTP 连接和请求生命周期属于调用者，解码/分类失败仅通过返回值退出，不承担连接清理。

性能上，`fields` 会先构造包含全部字段 slice 的 `Vec`，嵌套消息再次构造各自字段 `Vec`；Region、key、message 和 detail 会发生拥有型分配/克隆。错误响应通常很小，但若扩展到大响应或高频路径，应在保持边界行为的前提下评估流式解析，而不能简单删除校验。

## 与 Go 版本的对应关系

Rust `IngestAPIError` 对应 Go 同名结构，`err/new_region` 对应 `Err/NewRegion`；Rust `Display`、`Cause`、`Unwrap` 分别覆盖 Go `Error`、`Cause`、`Unwrap` 的可观察语义。Rust 用 `CategorizedError` 显式保存类别和 detail，以 RFC code 实现 Go `errors.Is` 所依赖的类别身份。

Rust `NewIngestAPIError` 的分类顺序与 `ingest_err.go` 的 `switch` 一致，`getIngestFailedMsg` 的 15 个变体顺序和三种拼接结果也与 Go 一致。独立的 Rust 测试 `test_convert_pb_error_to_error`、`test_get_ingest_failed_msg` 与 Go 的 `TestConvertPBError2Error`、`TestGetIngestFailedMsg` 对齐；Rust 另有 EpochNotMatch 回调和畸形 protobuf 边界测试。

差异主要在输入与接线：Go 直接接收生成的 `*errorpb.Error`，Rust 为避免依赖完整 protobuf 类型而在本文件手写 wire 子集解码。Go `pkg/ingestor/ingestctrl/region_job.go` 的生产调用会传 `extractRegionFromErr` 风格闭包；当前 Rust `client.rs` 调用传 `None`，而 Rust ingestctrl 文件中只检索到测试注释，没有等价生产调用边。因此“能够通过回调产生 `new_region`”已由 Rust 单测证明，但“当前 Rust 生产 ingest 主链会产生 `new_region`”并不成立。

Go `TestIngestAPIErrorRetryable` 还通过 `pkg/lightning/common.IsRetryableError` 验证 `ErrKVIngestFailed` 可重试、`ErrKVDiskFull` 不可重试。Rust `test_ingest_api_error_categories_are_distinct` 只验证两者 RFC code 不同，并明确说明 ingestcli crate 不依赖该上层分类器；不能把这个局部测试描述为完整重试策略集成测试。

## 扩展指南

新增或更新 errorpb 变体时，至少同步检查以下位置：

1. 若变体影响重试类别，在 `ErrorPb` 增加状态，在 `decode_error_pb` 按真实 protobuf field number 解码，并在 `NewIngestAPIError` 的 Go 对齐优先级中放置分支。
2. 若变体仍归入通用 ingest 失败但 message 可能为空，在 `getIngestFailedMsg` 增加类型名；不要随意改变已有分支顺序。
3. 若新决策需要嵌套消息内容，扩展私有解码链及 `interface.rs` 的数据类型前先核对 Go/kvproto schema；仅把“消息存在”置 true 不足以支持内容驱动决策。
4. 同步更新 `pkg/ingestor/ingestcli/ingest_err_test.rs` 的分类、消息、优先级和畸形输入用例，并对照 `ingest_err_test.go`。测试必须继续独立于生产源文件。
5. 若把 Region 提取接入 Rust 生产链，需要修改实际调用方而非只改本文件，并验证 EpochNotMatch 后续阶段消费 `new_region` 的行为；当前 `client.rs` 固定传 `None` 是明确的迁移限制。

兼容风险包括 RFC code 或分类顺序变化、protobuf field number/wire type 不匹配、raft dropped 文案变化及错误 detail 格式变化。性能风险集中在大错误体的多层 `Vec` 和内容克隆；并发风险当前不存在，但若未来保存回调或异步执行，必须重新定义闭包的所有权与 `Send`/`Sync` 边界。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、307,296 个节点；`files --filter pkg/ingestor/ingestcli` 确认目标、调用方和独立测试均已索引；`node --file` 完整读取 `ingest_err.rs`（1—373）、`client.rs` 的错误类型与 ingest 路径（1—130、525—604）、`interface.rs` 的 Region 类型（20—174）和 `ingest_err_test.rs`（1—337）。
- RustCodeGraph 符号查询：确认 `NewIngestAPIError` 位于 `ingest_err.rs:120`、`getIngestFailedMsg` 位于 `:165`、`decode_error_pb` 位于 `:207`；目标文件共 22 个索引符号。精确 `callers/callees` 命令在本地索引上连续超时，因此调用边进一步用目标目录及 `pkg/ingestor` 的精确 `rg` 检索复核，没有以同名 `Error` 的宽泛图结果代替证据。
- Rust/Cargo 证据：`pkg/ingestor/ingestcli/Cargo.toml`、`lib.rs`、`client.rs`、`interface.rs`、`ingest_err.rs`、`ingest_err_test.rs`。
- Go 对照证据：`pkg/ingestor/doc.go`、`pkg/ingestor/ingestcli/ingest_err.go`、`ingest_err_test.go`、`client.go`，以及用于确认 Go 上层 Region 回调接线的 `pkg/ingestor/ingestctrl/region_job.go`。
- 人工复核结论：本文区分了解码、分类与上层重试决策，标出了当前 Rust 生产调用只传 `None` 的限制，没有把测试能力或 Go 接线写成 Rust 已接线事实，也没有建议把测试内嵌回生产文件。

