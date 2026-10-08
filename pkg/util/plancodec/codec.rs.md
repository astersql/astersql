# `pkg/util/plancodec/codec.rs`

## 文件定位

`codec.rs` 是 `astersql-util-plancodec` crate 的文本执行计划线格式与压缩格式实现。crate 入口 `pkg/util/plancodec/lib.rs` 以 `include!("codec.rs")` 将它放入私有 `codec` 模块，再通过 `pub use codec::*` 暴露公开 API；同一 crate 的 `binary_plan_decode.rs` 也直接复用本文件的 `Decompress`、`Error` 和丢弃提示常量。

该文件位于计划的生产、持久化与展示之间：编码侧把计划节点写成以换行分行、tab 分列的紧凑字节格式；存储侧以 Snappy 压缩并用标准 base64 表示；解码侧把物理算子 ID 和 task 编码还原成可读文本树。实际接线可见 `pkg/util/topsql/topsql.rs::default_pipeline`、`pkg/util/stmtsummary/reader.rs` 的 `PlanStr` 工厂以及 `pkg/util/stmtsummary/v2/column.rs` 的 `PlanStr` 工厂。

## 核心职责

- `EncodePlanNode` 与 `NormalizePlanNode` 生成 Go 兼容的计划行；前者保留估算/运行时字段，后者只保留稳定的归一化子集。
- `DecodePlan` 解压普通计划、增加列标题并渲染树；`DecodeNormalizedPlan` 直接渲染未压缩归一化计划且不加标题。
- `EncodeTaskType`、`EncodeTaskTypeForNormalize` 和 `decodeTaskType` 在 root/cop 及 TiKV、TiFlash、TiDB task 表示之间转换。
- `Compress`/`Decompress` 实现标准 base64 外层和 Snappy block 内层；`BinaryPlanDiscardedEncoded` 构造二进制计划的 protobuf 丢弃哨兵。
- `PlanDecoder` 解析节点深度、恢复树形连接线、补齐列并复用缓冲区，避免每次展示计划都重新分配全部中间容器。
- `Error` 保持 base64、Snappy、protobuf、UTF-8、非法计划和 panic 的错误边界；`GoMessage`/`as_bytes` 额外保存 Go 字符串允许携带的任意原始字节。

## 主要符号

- `PlanDiscardedEncoded: &str`：文本计划过长时的线格式哨兵 `"[discard]"`；`PLAN_DISCARDED_DECODED` 是展示侧固定文案。
- `Error`：本 crate 的编解码错误枚举。`GoMessage(Vec<u8>)` 的 `Display` 仅作有损 UTF-8 展示，精确错误字节需经 `Error::as_bytes` 获取；`Snappy` 和 `Protobuf` 支持来源错误转换。
- `BinaryPlanDiscardedEncoded() -> String`：构造 `tipb::ExplainData`，设置 `discarded_due_to_too_long`，序列化后交给 `Compress`。序列化失败时返回空字符串，和 Go 包初始化表达式的回退行为一致。
- `DecodePlan(...) -> Result<Vec<u8>, Error>`：公开的压缩文本计划入口，空输入返回空字节；启用表头，并把渲染 panic 收敛为 `Error::DecodePlanPanicked`。
- `DecodeNormalizedPlan(...) -> Result<Vec<u8>, Error>`：公开的未压缩归一化计划入口，禁用表头。它保留 Go string 的任意字节语义，且没有把内部 panic 转成 `Error`。
- `PlanDecoder`：私有、有状态的渲染器，持有输出 `buf`、节点 `depths`、树形 `indents`、解析后的 `planInfos`、`addHeader` 和父节点缓存 `cacheParentIdent`。
- `PlanInfo`：单个有效计划行的深度和字段字节数组。
- `decodePlanInfo`：解析单行的 depth、计划 ID、task 和其余字段；少于两列的行被忽略。
- `EncodePlanNode`：输出完整节点；`rowCount` 对整数不保留小数，对有限非整数保留两位，并显式兼容 `+Inf`、`-Inf`、`NaN`；只有任一运行时字段非空时才追加完整四列。
- `NormalizePlanNode`：输出 depth、物理类型 ID、task、explain info；调用者负责传入已归一化的 explain info。
- `EncodeTaskType`/`EncodeTaskTypeForNormalize`：root 编为 `0`；cop 通常为 `1_<store>`，归一化 TiKV cop 特例为 `1`。
- `Compress`/`Decompress`：公开的压缩往返接口。`Decompress` 在交给 `snap` 前规范化 Go 可接受的最长十字节、非最短 uvarint 长度头。
- `go_atoi`、`quote_go_bytes`、`decode_base64`：私有兼容层，分别复现 Go `strconv.Atoi` 的平台有符号范围、Go 字节字符串引用格式和 `encoding/base64.StdEncoding` 的错误位置/非严格 padding-bit 行为。

## 执行流程

完整计划编码从 `EncodePlanNode` 开始：写入 depth，调用 `encodeID` 把 `TypeStringToPhysicalID(planType)` 与实例 ID 拼成 `typeId_instanceId`，写入 task 和格式化后的 row count，再由 `escapeString` 把 explain info 中的 tab/换行替换成字面 `\\t`/`\\n`。若存在运行时信息，四个运行时字段必须一起占位。多行字节最终可交给 `Compress`：先 `snap::raw::Encoder::compress_vec`，再用标准 base64 编码。

普通展示从 `DecodePlan` 开始。它取出池中 decoder、清空输出并设置 `addHeader=true`，然后在受保护的闭包内调用 `PlanDecoder::decode`。`decode` 先 `Decompress`；仅当解压失败且原输入正好等于 `PlanDiscardedEncoded` 时返回丢弃文案，否则传播错误。成功解压后进入 `buildPlanTree`。

`buildPlanTree` 按换行切分节点，通过 `decodePlanInfo` 忽略无效短行并解析有效行。随后可插入与第一行列数相符的标题，按 `2 * depth` 初始化缩进；对每个非首节点，`findParentIndex` 从深度缓存或反向扫描寻找父节点，`fillIndent` 把同层前序连接符改为中间节点并补竖线。`alignFields` 先把所有行扩展到相同列数，再按列最大宽度补空格（最后一列除外，第 0 列宽度包括缩进）。最后逐行写入一个前导 tab、树形字符和字段，返回输出缓冲的克隆。

归一化链路由 `NormalizePlanNode` 写入稳定字段，经 TopSQL 管线的 `Compress` 持久化；读取时 `DecodeNormalizedPlan` 跳过解压和表头，直接执行上述树构建。`pkg/util/topsql/topsql.rs::default_pipeline` 明确把这两个函数作为 reporter 的解码与压缩回调。

## 数据与状态

文本线格式的不变量是“一行一个节点、tab 分列”：第 0 列为有符号深度，第 1 列为数值物理类型 ID及可选实例后缀，第 2 列为 task 编码，其余列原样展示。`decodePlanInfo` 允许只有 depth 与 ID 的行；task 仅在第 2 列实际存在时解析。计划 ID 只允许 `id` 或 `id_instance` 两段，更多下划线段直接报错。

`PlanDecoder` 是一次调用期间的可变状态，不是共享解码状态。容量不足时，`buildPlanTree` 按节点数量重新分配 `depths`、`planInfos`、`indents`；否则清空后复用容量。`buf` 每次入口清空，`cacheParentIdent` 每次建树前清空，因此从池中复用不会把上次计划的逻辑状态带到本次。

模块级 `DECODER_POOL: LazyLock<Mutex<Vec<PlanDecoder>>>` 是唯一进程共享可变状态。池没有大小上限；每次正常入口取走一个独占 decoder，结束后归还。`BinaryPlanDiscardedEncoded` 与文本哨兵不共享表示：前者是压缩后的 `ExplainData` protobuf，后者是字面字符串。

## 依赖与调用关系

上游方面，`pkg/util/topsql/topsql.rs::default_pipeline` 调用 `DecodeNormalizedPlan` 和 `Compress`，负责 TopSQL reporter 的归一化计划往返；`pkg/util/stmtsummary/reader.rs` 与 `pkg/util/stmtsummary/v2/column.rs` 调用 `DecodePlan` 生成 statement summary 的 `PlanStr`，并在失败时记录错误后返回空字节。crate 内部的 `pkg/util/plancodec/binary_plan_decode.rs::{DecodeBinaryPlan, DecodeBinaryPlan4Connection}` 调用 `Decompress` 后解析 `tipb::ExplainData`。

下游方面，计划类型转换依赖同 crate `id.rs` 的 `PhysicalIDToTypeString` 和 `TypeStringToPhysicalID`；树形绘制依赖 `astersql-util-texttree` 的 `TreeLastNode`、`TreeMiddleNode`、`TreeBody`、`TreeNodeIdentifier`；task store 数值来自 `lib.rs::kv::StoreType`。外部 crate 依赖由 `pkg/util/plancodec/Cargo.toml` 声明：`base64`、`snap`、`protobuf`、`thiserror`、`log` 与路径依赖 `astersql-util-texttree`；构建脚本生成本 crate 使用的 `tipb` Explain protobuf 代码。

RustCodeGraph 的文件节点显示 `codec.rs` 定义 58 个符号，精确查询能定位 Rust `DecodePlan`、`decodePlanInfo`、`EncodePlanNode` 及同路径 Go 对照；当前索引的精确 `callers/callees` 命令未返回这些函数的边，因此上述生产调用关系由模块入口和符号引用交叉核验，不把全仓库同名 `Compress`/`DecodePlan` 结果误算为本模块调用。

## 错误处理与边界

- 空 `DecodePlan`/`DecodeNormalizedPlan` 输入成功返回空结果；空 base64 输入进入 Snappy 后是损坏输入，而不是空明文。
- `DecodePlan` 捕获解码和缩进计算中的 panic、记录日志并返回 `DecodePlanPanicked`。为确保 decoder 在内部 panic 后仍归还池中，它先在内层捕获 panic、归还对象，再恢复展开到外层。`DecodeNormalizedPlan` 也会先归还 decoder，但随后恢复 panic，保持 Go 归一化入口未安装 recover 的边界。
- 负深度可被 `go_atoi` 解析；`-0` 等价于零，真正负值在缩进长度转换时 panic。测试明确区分普通入口的错误返回和归一化入口的 panic。
- `go_atoi` 接受可选正负号，并按当前平台 `isize` 范围拒绝溢出；错误消息引用原始输入，即使包含非 UTF-8 字节。计划 ID 数值合法但未知时，由 `PhysicalIDToTypeString` 产生未知类型名称。
- `decodeTaskType` 把 `0` 开头视为 root；单段非 root 编码兼容归一化 cop；带 store 后缀时解析第二段，0/1/2 对应 tikv/tiflash/tidb，其余数值展示 `unspecified`。当前实现不拒绝第二段之后的额外段，这是与 Go `strings.Split` 后只读取 `segs[1]` 一致的现状。
- `decode_base64` 忽略 CR/LF，接受 Go 标准解码器允许的非零未使用 padding bits，同时严格报告缺失、多余或尾随 padding 的原输入字节偏移。
- `Compress` 认为 Snappy 压缩只会因超大输入失败并使用 `expect`；极端超限输入会 panic。`BinaryPlanDiscardedEncoded` 则吞掉 protobuf 序列化错误并返回空字符串。
- `NormalizePlanNode` 对无法解释为 UTF-8 的 plan type 使用空字符串查 ID，而 explain info、task 和输出整体仍按原始字节保存；这一区分由独立测试覆盖。

## 并发与资源生命周期

`DECODER_POOL` 由 `Mutex` 串行保护“取出/归还”操作，但耗时的解析和渲染发生在锁外；一个 `PlanDecoder` 同一时刻只属于一个调用者。锁中毒时 `take_decoder`/`put_decoder` 使用 `into_inner` 恢复池内容，不因先前持锁线程 panic 永久失去服务能力。

普通与归一化入口都保证在内部建树 panic 前先重新捕获、把 decoder 放回池，再分别转成错误或恢复 panic。正常 `Result::Err` 同样在返回调用者前归还。`codec_test.rs::tree_alignment_and_concurrent_decoder_reuse` 以 8 个 scoped 线程各重复 20 次成功、错误和哨兵解码，验证池复用不会造成跨调用污染。

该文件不启动线程、不持有异步任务、通道、文件、网络连接或事务。主要资源是计划输入/输出字节和可复用 Vec 容量；`buildPlanTree` 返回 `self.buf.clone()`，所以归还池后调用者拥有独立结果。池无淘汰策略，历史峰值容量可能随 decoder 长期保留，扩展时应关注异常大计划造成的常驻内存。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/plancodec/codec.go`。Rust 保留了 Go 的常量、`planDecoder`/`planInfo` 结构、解码步骤、表头裁剪、父节点查找、树线填充、列对齐、计划行编码、task 编码和 Snappy+base64 顺序；`codec_test.go` 的 root/cop 与丢弃哨兵用例也在 `codec_test.rs` 中对应存在。

实现层差异主要用于精确复刻 Go 运行时语义。Go 的 `string` 可装任意字节，Rust 因而让主要解码结果和 `PlanInfo.fields` 使用 `Vec<u8>`，只在错误 `Display` 边界有损转换；Go `strconv.Atoi`、字符串引用、base64 错误偏移和 Snappy uvarint 宽容度由私有兼容函数复现。Go `sync.Pool` 对应 Rust 的 `LazyLock<Mutex<Vec<_>>>`，语义上均复用 decoder，但 Rust 池不会自动受 GC 清理。

Rust `DecodePlan` 通过双层 `catch_unwind` 复现 Go 的 `defer recover`，而 `DecodeNormalizedPlan` 保持 Go 中 panic 向上传播的行为。Rust 还显式处理浮点 `+Inf`、`-Inf`、`NaN` 与负零，以匹配 `strconv.FormatFloat` 的可见结果；对超出 UTF-8 的字段和平台整数边界的补充用例比 Go 的同路径测试更细，但没有改变线格式。

## 扩展指南

新增计划列时，优先修改编码入口 `EncodePlanNode`、`PlanDecoder::addPlanHeader` 及相应消费方，保持“运行时字段成组占位”和最后一列不补空格的不变量；同步扩展独立的 `pkg/util/plancodec/codec_test.rs`，不要把测试嵌入生产文件。若变更归一化内容，还必须同时审查 `NormalizePlanNode`、TopSQL digest/上报兼容性和 Go `codec.go`，因为字段变化可能改变跨版本摘要。

新增 store 类型时，应同步 `pkg/util/plancodec/lib.rs::kv::StoreType`、`EncodeTaskType`/`EncodeTaskTypeForNormalize` 和 `decodeTaskType` 的展示映射，并补齐全部编码值、未知值和无后缀兼容测试。新增物理算子类型则应在 `id.rs` 的双向映射处理，本文件只负责调用该映射。

调整解压或错误处理时必须保留 Go 的边界：CR/LF、padding 错误偏移、非零 padding bits、十字节非最短 Snappy uvarint、任意非 UTF-8 字节、`isize` 溢出和普通/归一化入口不同的 panic 行为。优化池或缓冲策略时，应继续验证并发隔离、panic 后归还和大计划内存上限；池无界是潜在性能/常驻内存风险，而给池加上限会改变复用特征，应单独基准评估。

二进制哨兵或 protobuf 格式扩展应同步检查 `BinaryPlanDiscardedEncoded`、`binary_plan_decode.rs` 以及构建生成的 `tipb` 类型，不能把文本 `"[discard]"` 与 protobuf 标志混为一种协议。所有 Rust 行为变更都应与 `pkg/util/plancodec/codec.go` 及 `codec_test.go` 对照，避免只为 Rust 测试缩减 Go 语义。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件、4,415 个 Go 文件；`files --filter pkg/util/plancodec` 确认目标、模块入口、Go 对照和独立测试均已索引；`node --file pkg/util/plancodec/codec.rs` 阅读全部 774 行；`query DecodePlan`、`query EncodePlanNode` 精确区分 Rust/Go 符号。精确 `callers/callees` 对目标函数未返回边，调用关系因此由引用搜索和调用点源码核验。
- 生产源码：`pkg/util/plancodec/codec.rs`；模块边界：`pkg/util/plancodec/lib.rs`；crate 声明：`pkg/util/plancodec/Cargo.toml`；crate 内下游：`pkg/util/plancodec/binary_plan_decode.rs`。
- 直接调用点：`pkg/util/topsql/topsql.rs::default_pipeline`、`pkg/util/stmtsummary/reader.rs` 的 `PlanStr` 工厂、`pkg/util/stmtsummary/v2/column.rs` 的 `PlanStr` 工厂。
- Go 对照：`pkg/util/plancodec/codec.go`；Go 测试：`pkg/util/plancodec/codec_test.go`。
- Rust 独立测试：`pkg/util/plancodec/codec_test.rs`，覆盖 task/哨兵、base64 padding、负深度与 panic、树对齐与并发池、非 UTF-8、Go 错误文本与整数范围、Snappy uvarint、全部 u8 store 值和 Go Unicode 引用版本。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前仅执行任务规定的 11 章节结构校验，并人工复查所有行为陈述均能回指上述符号或文件。
