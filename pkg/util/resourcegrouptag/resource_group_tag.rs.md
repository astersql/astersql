# `pkg/util/resourcegrouptag/resource_group_tag.rs`

## 文件定位

[对应源文件](./resource_group_tag.rs)是 `astersql-util-resourcegrouptag` crate 的业务实现文件，由同目录 `lib.rs` 声明为 `resource_group_tag` 模块并把公开项重新导出。crate 边界由 `pkg/util/resourcegrouptag/Cargo.toml` 定义；它依赖 `astersql-tablecodec-rowindexcodec` 做 TiDB 键分类、依赖固定为 2.8.0 的 `protobuf` 生态生成 KV/TiPB 消息，并用 `thiserror` 定义可比较的解码错误。

它位于 SQL 请求到 TiKV RPC 的资源归因边界：一方面解析随 RPC 携带的 `tipb.ResourceGroupTag`，取回 SQL digest；另一方面根据请求首键确定行键/索引键标签。直接的生产接线见 `pkg/kv/kv.rs`：`ResourceGroupTagBuilder::Build` 调用首键提取，`EncodeTagWithKey` 调用键分类并构造标签。反向解码的直接例子见 `pkg/util/deadlockhistory/deadlock_history.rs`，死锁等待链使用 `DecodeResourceGroupTag` 恢复 SQL digest。

## 核心职责

1. `DecodeResourceGroupTag` 以兼容 Go gogo protobuf 解码器的规则扫描线格式，只返回字段 1（`sql_digest`），同时校验已知字段的 wire type、长度、varint 和未知字段边界。
2. `GetResourceGroupLabelByKey` 把 `rowindexcodec::GetKeyKind` 的行键、索引键和其他结果映射到 `ResourceGroupTagLabelRow`、`ResourceGroupTagLabelIndex`、`ResourceGroupTagLabelUnknown`。
3. `GetFirstKeyFromRequest` / `GetFirstKeyFromRequestMut` 从九类 KV/Coprocessor 请求的固定位置取“第一个键”，分别提供只读借用和可写借用；`RequestPayload::Other` 明确表示不支持的请求。
4. `InMemoryRequest` 及其辅助结构保留 Go 内存态中“nil 与非 nil 空切片”“nil 重复消息元素”等 protobuf 序列化后会丢失的区别，用于精确对齐 Go 类型分支语义。

本文件不负责选择资源组、实施限流、编码完整标签或发送 RPC；这些职责在调用方完成。它也不是通用 protobuf 解码器，只解析本任务需要的 `ResourceGroupTag` 字段布局。

## 主要符号

- `DecodeResourceGroupTagError { encoded: String }`：公开错误类型，内部保存输入的连续小写十六进制文本；显示格式固定为 `invalid resource group tag data <hex>`。
- `DecodeResourceGroupTag(&[u8]) -> Result<Option<Vec<u8>>, DecodeResourceGroupTagError>`：公开解码入口。空输入和合法但没有 SQL digest 的标签均成功返回 `None`；显式的零长度字段 1 返回 `Some(Vec::new())`。
- `decode_tag`、`take_varint`、`skip_unknown`：私有线格式实现。`decode_tag` 识别 proto 字段 1 至 5；`take_varint` 最多消费 10 个字节覆盖 `u64`；`skip_unknown` 支持 wire type 0、1、2、3、4、5，并以迭代深度处理 group，避免递归栈溢出。
- `GetResourceGroupLabelByKey(&[u8]) -> ResourceGroupTagLabel`：公开键分类入口，完全委托 `rowindexcodec::GetKeyKind` 判断键形状。
- `RequestPayload` / `Request`：公开 RPC 载荷模型。前者覆盖 `Get`、`BatchGet`、`Scan`、`Prewrite`、`Commit`、`BatchRollback`、`Coprocessor`、`BatchCoprocessor`、`PessimisticLock`、`InMemory` 与 `Other`。
- `RequestKey`、`RequestKeys`、`RequestMutations`、`RequestRange`、`RequestRanges`、`RequestRegions`：公开的最小内存投影；嵌套 `Option` 保留 Go 的 nil 消息、nil 字节切片和 nil 列表元素。
- `InMemoryRequest`：上述投影的请求枚举；私有 `first_key` 和 `first_key_mut` 保证读写视图遵循相同的首元素路径。
- `GetFirstKeyFromRequest` / `GetFirstKeyFromRequestMut`：公开只读/可写首键入口。返回值借用原请求存储，不复制键；可写版本的修改直接反映到请求。

## 执行流程

解码流程如下：

1. `DecodeResourceGroupTag` 对空切片直接返回 `Ok(None)`；否则调用 `decode_tag`。
2. `decode_tag` 循环读取 protobuf tag varint，拆成字段号和 wire type。字段号非正数或在顶层遇到 end-group（wire 4）即判畸形。
3. 字段 1、2、5 必须是 length-delimited；只有字段 1 的内容被复制为 SQL digest，后出现的字段 1 覆盖前值。字段 3、4 必须是 varint，值只被跳过。
4. 未知字段交给 `skip_unknown`。固定 64 位/32 位分别跳过 8/4 字节，长度字段先解长度，group 用 `depth` 迭代消费到深度归零；截断、非法 wire type、varint 溢出或深度下溢均失败。
5. 私有函数返回 `None` 时，公开入口把原输入编码为十六进制并构造 `DecodeResourceGroupTagError`。

请求首键流程由 `RequestPayload` 分支决定：Get 取 `key`；BatchGet、Commit、BatchRollback 取 `keys[0]`；Scan 取 `start_key`；Prewrite 和 PessimisticLock 取 `mutations[0].key`；Coprocessor 取 `ranges[0].start`；BatchCoprocessor 取 `regions[0].ranges[0].start`。任一必需容器为空或可空节点为 `None` 时通常返回 `None`，绝不退而选择后续非空元素。`InMemoryRequest` 使用同一路径，但保留空切片存在性。可写入口逐分支返回相同底层字节的 `&mut [u8]`。

典型写入链为 `pkg/kv/kv.rs::ResourceGroupTagBuilder::Build` → `GetFirstKeyFromRequest` → `EncodeTagWithKey` → `GetResourceGroupLabelByKey` → protobuf `write_to_bytes` → 写入 `tikvrpc::Request.ResourceGroupTag`。典型读取链为 `ErrDeadlockToDeadlockRecord` → `DecodeResourceGroupTag` → digest 十六进制化；解码失败会由调用方告警并退为空 digest，而不是丢弃等待链项。

## 数据与状态

本文件没有模块级可变状态。解码器只维护当前输入切片、当前 digest 和未知 group 深度；所有权清晰：成功的 digest 是从输入复制出的 `Vec<u8>`，错误持有输入的十六进制 `String`。

生成 protobuf 载荷中的标量 `bytes` 没有 Go 内存态的 presence 信息，因此 `non_empty` 把空字节正规化为 `None`。相反，`InMemoryRequest` 用 `Option<Vec<u8>>` 区分 nil（`None`）与非 nil 空切片（`Some(vec![])`），并用 `Vec<Option<...>>` 保留重复消息中的 nil 元素。这一区别是该数据模型存在的主要原因。

首键 API 的返回生命周期绑定到传入 `Request`。只读接口返回 `Option<&[u8]>`，可写接口返回 `Option<&mut [u8]>`；它们既不分配也不克隆请求键。列表顺序是不变量：只看索引 0，即便第一个键为空或 nil，也不搜索后续元素。

## 依赖与调用关系

- 下游 `crate::rowindexcodec::GetKeyKind`：提供 TiDB 行键/索引键分类事实；本文件只做枚举映射。
- 下游 `crate::kvproto::{kvrpcpb, coprocessor}`：类型化 RPC 请求来自 `build.rs` 按官方 proto 子集生成的绑定。
- 下游 `crate::tipb::ResourceGroupTagLabel`：标签枚举来自 `proto/tipb/resourcetag.proto`；同一 schema 规定字段 1 至 5 分别是 sql digest、plan digest、label、table id 和 keyspace name。
- 下游 `thiserror`：只用于 `DecodeResourceGroupTagError` 的 `Error`/`Display` 实现。
- 上游 `pkg/kv/kv.rs`：生产标签时调用 `GetFirstKeyFromRequest` 和 `GetResourceGroupLabelByKey`；`pkg/kv/lib.rs` 提供适配层并把借用结果复制为其公开的 `Vec<u8>`。
- 上游 `pkg/util/deadlockhistory/deadlock_history.rs`：调用 `DecodeResourceGroupTag` 解析 TiKV 死锁等待链的资源组标签。
- 其他直接依赖在 Cargo 中包括 `pkg/executor`、`pkg/util/execdetails/internal/ruv2` 和若干测试 crate；仓库搜索显示 executor/server 测试直接使用解码入口验证跨模块标签契约。

RustCodeGraph 的 `files`/`node --file` 查询确认该文件有 45 个索引符号，并报告被 29 个文件使用；精确名称的 `callers`/`callees` 查询未输出可用边，因此上述具体调用边又由对应生产源码和 Cargo 依赖声明核验，不能把“29 个使用文件”理解为 29 个运行时调用者。

## 错误处理与边界

`DecodeResourceGroupTag` 的失败面包括：零字段号、已知字段 wire type 不匹配、截断的 tag/长度/负载、超过 `u64` 能力的 varint、非法 wire type 6/7、顶层 end-group，以及未知 group 未闭合。错误不会暴露局部 digest，统一携带完整原始输入的十六进制文本。未知合法字段会跳过，以维持 protobuf 向前兼容；10,000 层 group 的测试证明实现采用循环而非递归。

空值语义需要特别注意：空输入表示没有标签，返回 `None`；编码为字段 1 且长度为零表示字段存在，返回 `Some(empty)`；生成的 proto3 请求键无法表达 presence，所以 Get/Scan/Mutation/Range 的空字节返回 `None`，而 `InMemoryRequest` 能返回 `Some(&[])`。BatchGet/Commit/BatchRollback 的生成绑定直接取第一个 `Vec<u8>`，因此列表中的空首元素仍返回 `Some(&[])`。

PessimisticLock 特意保留 Go 的异常边界：类型化 nil 请求体会 `expect` panic；内存投影中首个 mutation 为 nil 也会 panic。其他载荷通常以 `?` 安全返回 `None`。`Other` 始终返回 `None`。扩展请求类型时不能把这些差异统一成“更安全”的行为，否则会偏离 Go 兼容契约。

## 并发与资源生命周期

所有函数都是同步、局部计算，没有锁、通道、任务、线程、I/O 或全局缓存；同一请求只要遵守 Rust 借用规则即可跨线程由外层安全管理。本文件本身不承担请求发送、重试或资源组配额的生命周期。

内存生命周期由借用类型约束：读借用存续时不能取得冲突的可写借用，可写首键视图存续时请求不能被同时访问。`GetFirstKeyFromRequestMut` 不改变向量长度，只允许就地改写现有字节，因此不会自行触发重新分配。`DecodeResourceGroupTag` 仅为成功 digest 或错误文本分配所有权数据；未知 group 深度使用 `usize::checked_add/checked_sub`，防止深度计数溢出或下溢。

## 与 Go 版本的对应关系

Go 对照实现是 `pkg/util/resourcegrouptag/resource_group_tag.go`。三个原始公开能力一一对应：Go 使用生成的 `tipb.ResourceGroupTag.Unmarshal`，Rust 为兼容 protobuf 2.8 与 gogo 的未知 group 行为实现轻量扫描器；两者都只返回 SQL digest，并在畸形输入时报告包含原字节十六进制的错误。键分类分支完全相同。

Go 的 `GetFirstKeyFromRequest` 对 `tikvrpc.Request.Req` 做类型开关；Rust 用 `RequestPayload` 做等价枚举分派。九类已知请求的取键位置一致，且都只取第一项。Go 的 PessimisticLock 分支不检查类型化 nil，也不检查首 mutation 是否 nil，Rust 以 `expect` 明确保留相同 panic 边界。

生成 protobuf Rust 类型无法完整表达 Go 的 nil/空切片别名语义，所以 Rust 额外提供 `InMemoryRequest`、六个投影结构和 `GetFirstKeyFromRequestMut`。这些不是 Go 新功能的简化，而是用于保存 Go 原始内存状态和“返回切片别名原存储”的契约。`resource_group_tag_test.rs` 的 presence、指针相等和写回测试验证了这一点；`migration_aster_unit_test.rs` 则逐分支验证基础移植行为。

## 扩展指南

新增 RPC 类型时，应同时修改 `RequestPayload`、`GetFirstKeyFromRequest` 和 `GetFirstKeyFromRequestMut`；若该类型需要保留 Go nil/presence 语义，还应新增或复用投影结构、扩展 `InMemoryRequest::first_key`/`first_key_mut`。两种视图必须选择完全相同的路径，并在 `resource_group_tag_test.rs` 中覆盖 nil 请求、空列表、nil 首元素、空首键、多个键只取第一项、读写指针一致和写回行为。

调整标签 schema 时，先以 `proto/tipb/resourcetag.proto` 的字段号和 wire type 为依据，再同步 `decode_tag` 的已知字段规则及 `build.rs` 生成输入。不得改变既有字段号；新增字段应验证合法未知字段仍可跳过、各类截断仍报错。若要返回 plan digest 等更多内容，应设计新的结果类型，避免静默改变 `DecodeResourceGroupTag` 现有返回契约。

改变键分类规则应在 `rowindexcodec` 的真实键规则基础上修改 `GetResourceGroupLabelByKey`，并同步本目录 Rust 独立测试与 Go 测试。性能上应保持首键提取零拷贝、未知 group 非递归；兼容性上重点防止把 nil 和空值混同、跳过第一个空元素、或改变 PessimisticLock 的 panic 行为。Rust 单元测试继续放在同目录独立 `*_test.rs` 文件，不内嵌到生产源文件。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引含 11,467 个文件；`files --filter pkg/util/resourcegrouptag` 列出目标实现、模块入口和两份独立 Rust 测试；`node --file ... --offset ...` 读取了目标文件全部 380 行并给出“45 个符号、29 个使用文件”。对四个公开入口执行 `callers`/`callees` 没有返回边，故未据此推断具体调用数量。
- 实现与边界：完整读取 `pkg/util/resourcegrouptag/resource_group_tag.rs`，并核对 `proto/tipb/resourcetag.proto` 的字段号、类型和标签枚举。
- crate 与生成边界：读取 `pkg/util/resourcegrouptag/Cargo.toml`、`lib.rs`、`build.rs`；确认 crate 再导出、三项运行依赖，以及 kvproto/tipb 绑定的生成来源。
- Go 对照：读取 `pkg/util/resourcegrouptag/resource_group_tag.go` 和 `resource_group_tag_test.go`，核对解码、键分类、请求类型开关和 nil/空列表预期。
- Rust 测试：读取 `pkg/util/resourcegrouptag/resource_group_tag_test.rs` 与 `migration_aster_unit_test.rs`；证据覆盖 protobuf 编码、畸形/未知 wire、深层 group、所有请求分支、PessimisticLock panic、借用别名、内存态 presence 和可写视图。
- 生产调用：读取 `pkg/kv/kv.rs`、`pkg/kv/lib.rs` 和 `pkg/util/deadlockhistory/deadlock_history.rs` 的直接调用段；另以 Cargo/源码引用搜索核对依赖声明。此次为纯文档分析，依任务约束未运行 Cargo 或代码测试。
