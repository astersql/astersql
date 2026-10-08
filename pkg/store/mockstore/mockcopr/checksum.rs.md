# `pkg/store/mockstore/mockcopr/checksum.rs`

## 文件定位

本文件属于 `astersql-store-mockstore-mockcopr` crate；crate 的入口是同目录 `lib.rs`，其中以 `mod checksum;` 装配本模块。它实现 mock Coprocessor 的 Checksum 单请求处理分支，供 mockstore 的请求/响应链路测试使用，而不是生产存储层的真实数据校验器。crate 归属和 Go 来源分别由 `Cargo.toml` 的包名及 `[package.metadata.porting].go-package = "pkg/store/mockstore/mockcopr"` 佐证。

上游入口位于 `copr_handler.rs`：`coprHandler::handle_request` 将 `RequestPayload::Checksum` 分派给本文件的 `handleCopChecksumRequest`，再把单个 `Response` 包装进 `BatchResponse`。更外层的命令式 RPC 入口 `rpc_copr.rs::coprRPCHandler::HandleCmdCop` 会先检查请求上下文，然后构造 `coprHandler` 并调用 `handle_request`。

## 核心职责

`coprHandler::handleCopChecksumRequest` 的唯一职责是返回一个确定性的 mock 响应：`checksum`、`total_kvs`、`total_bytes` 三个 protobuf `uint64` 字段都取 `1`。该方法刻意忽略传入的 `Request`，不扫描 `ranges`，不使用 `start_ts`，也不访问 `coprHandler.reader`；因此它验证的是 Checksum 响应的路由和承载格式，不证明 KV 数据校验算法正确。

文件直接写出等价 protobuf wire bytes `[0x08, 0x01, 0x10, 0x01, 0x18, 0x01]`。三个键字节依次代表字段号 1、2、3 且 wire type 为 varint，后随值 `1`。

## 主要符号

- `impl coprHandler`：把 Checksum 行为附加到 `copr_handler.rs` 定义的处理器类型；本文件不定义新结构、trait、常量或条件编译项。
- `pub fn handleCopChecksumRequest(&self, _request: &Request) -> Response`：模块的公开方法。`_request` 的下划线命名明确表示当前实现不读取请求；返回类型 `Response` 也定义于 `copr_handler.rs`。
- 局部变量 `data: Vec<u8>`：保存固定的六字节 protobuf 编码。返回时仅设置 `Response.data`，其余字段通过 `Response::default()` 保持为空或默认值。

该方法因 crate 根部允许 `non_snake_case` 而保留 Go 风格名称，以便与迁移来源 `checksum.go::handleCopChecksumRequest` 对齐。

## 执行流程

1. 调用方把载荷标识为 `RequestPayload::Checksum`；`coprHandler::handle_request` 选中 Checksum 分支。
2. `handleCopChecksumRequest` 接收处理器与请求引用，但不读取两者的状态。
3. 方法构造六字节向量：字段 1 `checksum=1`、字段 2 `total_kvs=1`、字段 3 `total_bytes=1`。
4. 方法以结构体更新语法创建 `Response`，将该向量放入 `data`，并令 `chunks`、`region_error`、`other_error`、`locked`、`counts`、`execution_summaries` 保持默认值。
5. `handle_request` 将响应放进只有一个元素的 `BatchResponse.responses`；命令式 RPC 路径随后取出首个响应。

本流程没有根据请求内容分支，也没有循环、扫描、编解码失败分支或异步步骤。

## 数据与状态

输入 `Request` 在 `copr_handler.rs` 中包含 `ranges`、`start_ts` 和 `payload`，但本方法全部忽略。处理器持有的 `Arc<dyn KvReader>` 和可选 `region_error` 也不会在本方法内读取；Region 上下文错误由更外层 `HandleCmdCop` 在分派前处理。

输出 `Response.data` 是拥有所有权的 `Vec<u8>`，每次调用都会新建。除 `data` 外，默认响应不存在 Region 错误、其他错误或锁冲突，也不返回 chunk、行数统计或执行摘要。虽然 `copr_handler.rs::KvReader` 另有 `checksum(ranges, start_ts)` 默认实现，能够扫描可见版本并计算 FNV 组合值、KV 数和字节数，但当前 Checksum handler 没有调用它；两者不能混同。

## 依赖与调用关系

直接依赖只有 `crate::copr_handler::{Request, Response, coprHandler}`，均来自同一 crate；本文件本身不引用 `Cargo.toml` 中的外部可选依赖。`Vec` 和 `Default` 来自 Rust 标准预导入。

已核实的调用链为：

`rpc_copr.rs::coprRPCHandler::HandleCmdCop` → `copr_handler.rs::coprHandler::handle_request` → `checksum.rs::coprHandler::handleCopChecksumRequest` → `Response::default`。

独立测试 `checksum_test.rs::checksum_response_matches_go_protobuf_wire_format` 也直接调用该方法。RustCodeGraph 的文件节点显示 `checksum.rs` 被 `copr_handler.rs` 与 `checksum_test.rs` 使用；精确 `callers`/`callees` 查询在本次验证的 30 秒窗口内未返回，因此上述边又由对应源码调用点复核。

## 错误处理与边界

方法返回 `Response` 而非 `Result`，内部没有可失败操作：固定字节向量的构造不暴露错误，且没有动态 protobuf marshal。由此，本方法不会填写 `other_error`。这与 Go 版本调用 `resp.Marshal()` 并在失败时写入 `OtherError` 的源码形态不同，但对三个固定 `uint64=1` 字段而言，Rust 直接编码消除了该动态失败点，并保持成功路径字节语义一致。

空范围、非法范围、任意 `start_ts`、不同 reader 内容以及非空处理器 Region 错误都不会改变本方法的直接返回值。正常应用调用应经过 RPC 上下文检查与 `handle_request` 的载荷分派；若绕过上游直接调用，本方法不会自行验证载荷确为 Checksum，也不会传播 reader 错误。

## 并发与资源生命周期

本方法只借用 `&self` 和 `&Request`，不修改共享状态，不加锁、不启动线程、不收发通道，也不持有跨调用资源。局部 `Vec<u8>` 随 `Response` 移交给调用方，响应释放时一并回收。

处理器内部 reader 使用 `Arc<dyn KvReader>`，但本方法不克隆或访问该 `Arc`，所以 Checksum 分支自身没有 reader 生命周期或并发访问要求。`rpc_copr.rs` 的 BatchCop 租约线程与通道也不参与此单请求处理流程。

## 与 Go 版本的对应关系

同路径 `checksum.go::handleCopChecksumRequest` 同样忽略请求内容，构造 `tipb.ChecksumResponse{Checksum: 1, TotalKvs: 1, TotalBytes: 1}`，再将 marshal 结果置入 `coprocessor.Response.Data`。Rust 的六字节常量正是这三个小 varint 字段的规范 wire representation，`checksum_test.rs` 对此逐字节断言，并确认 `other_error == None`。

差异在于 Go 通过生成的 protobuf 类型 marshal，并保留理论上的 marshal 错误响应；Rust 当前 crate 没有在本文件使用对应 protobuf 类型，而是直接构造 wire bytes，因此没有 marshal 错误路径。两端都不是实际存储校验：固定非零值的目的仅是模拟成功响应。当前目录未找到专门的 Go `checksum_test.go`；Go 语义证据来自 `checksum.go` 与 `rpc_copr.go`，Rust 回归证据来自独立的 `checksum_test.rs`。

## 扩展指南

若仍要保持 Go mock 兼容，仅修改响应字段时，应同步调整 `handleCopChecksumRequest` 的 wire bytes 和 `checksum_test.rs::checksum_response_matches_go_protobuf_wire_format` 的期望值，并用 protobuf 字段号与 wire type 复核编码；不要把 Rust 测试嵌回生产源文件。

若目标改为真实数据相关校验，最可能的接入点仍是本方法，但必须显式决定是否复用 `KvReader::checksum(&request.ranges, request.start_ts)`、如何把三个 `u64` 编码为 tipb wire format，以及 reader/范围错误映射到 `Response` 哪个错误字段。此变化会偏离现有 Go mock 的固定值语义，需同步独立测试覆盖空范围、MVCC 可见性、字节统计、错误传播与确定性，并评估扫描的时间/内存成本；不能仅删除固定值便宣称与生产 Checksum 等价。

若引入 protobuf 库或新的 crate 依赖，还需同步 `Cargo.toml` 并检查工作区依赖策略；当前实现无需任何外部依赖。

## 验证依据

- `pkg/store/mockstore/mockcopr/checksum.rs`：唯一生产符号、固定 wire bytes、默认响应字段和无失败路径。
- `pkg/store/mockstore/mockcopr/copr_handler.rs`：`RequestPayload::Checksum` 分派、`Request`/`Response`/`coprHandler` 定义，以及未被当前 handler 使用的 `KvReader::checksum`。
- `pkg/store/mockstore/mockcopr/rpc_copr.rs`：`HandleCmdCop` 的上下文检查和上游调用链。
- `pkg/store/mockstore/mockcopr/lib.rs`：模块装配和独立测试文件声明。
- `pkg/store/mockstore/mockcopr/Cargo.toml`：crate 名称、lib 入口、Go 包迁移元数据和依赖边界。
- `pkg/store/mockstore/mockcopr/checksum.go`、`rpc_copr.go`：固定三字段 Go 实现及 `kv.ReqTypeChecksum` 路由。
- `pkg/store/mockstore/mockcopr/checksum_test.rs`：六字节 protobuf 成功响应的回归断言；没有运行 Cargo，符合本任务约束。
- RustCodeGraph：`status` 显示索引包含目标 Rust 文件；`files --filter` 报告该文件有 3 个符号；`query handleCopChecksumRequest` 定位 Rust、同路径 Go 和 unistore Go 的同名实现；文件节点报告两处使用者。精确 callers/callees 查询超时，调用边由上述源码位置补证。

人工复核结论：本文区分了固定 mock 响应与真实 `KvReader::checksum`，能够说明文件存在原因、运行路径、状态与错误边界，以及保持 Go 兼容或扩展为真实校验时应修改和测试的位置。
