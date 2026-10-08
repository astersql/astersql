# `pkg/store/mockstore/mockcopr/analyze.rs`

## 文件定位

本文件属于 `astersql-store-mockstore-mockcopr` crate，crate 入口 `pkg/store/mockstore/mockcopr/lib.rs` 通过 `mod analyze;` 装配本模块，并在 `#[cfg(test)]` 下把独立测试文件 `analyze_test.rs` 接入同一 crate。`Cargo.toml` 的 `[package.metadata.porting]` 将 crate 对应到 Go 包 `pkg/store/mockstore/mockcopr`；本实现服务于 mockstore 的 Coprocessor `ANALYZE` 路径，不是 TiKV 或 SQL 层生产统计系统的完整实现。

已核实的上游调用链是 `rpc_copr.rs::coprRPCHandler::HandleCmdCop` → `copr_handler.rs::coprHandler::handle_request` → 本文件 `coprHandler::handleCopAnalyzeRequest`。`handle_request` 只在载荷为 `RequestPayload::Analyze` 时进入本模块；本文件再按 `AnalyzeType::{Index, Columns}` 选择索引或列统计分支。

## 核心职责

本文件承担三组职责：

- 为 mock 协议定义 `HistogramBucket`、`ColumnAnalyzeResult` 和 `AnalyzeResult`，并把分析结果编码为本 crate 自有的确定性字节格式。
- 扫描 `KvReader` 返回的可见行，对非 NULL 值排序，计算精确 distinct 数、等宽行数分桶的累计直方图和固定种子蓄水池样本；索引路径把整行编码成一个 `Datum::Bytes`，列路径按列偏移分别统计。
- 提供简化的 `analyzeColumnsExec` 拉取接口：首次取数时扫描并缓存全部行，之后每次 `Next` 最多输出一行。

这些能力是轻量 mock，而不是 Go 版本完整统计协议。Rust 没有在这里处理 protobuf/tipb 解码、请求上下文、时区、flags、CMSketch、FMSketch、collation、主键专用直方图或 `statistics::SampleBuilder`；输出也不是 Go `AnalyzeIndexResp`/`AnalyzeColumnsResp` 的 protobuf 编码。

## 主要符号

- `pub struct HistogramBucket`：保存桶的 `lower`、`upper`、累计 `count` 和与上界相等的尾部值数 `repeats`。
- `pub struct ColumnAnalyzeResult`：保存单列或单个复合索引的直方图、`null_count`、样本和 `distinct_count`。
- `pub struct AnalyzeResult`：列分析使用 `columns`，索引分析使用 `index: Some(...)`；当前成功结果只会走其中一种形状。
- `AnalyzeResult::encode` 与私有 `encode_column`：依次以大端整数、`Datum::encode` 和显式长度前缀编码结果；先写列数，再写列结果，随后写一字节索引存在标记及可选索引结果。
- 私有 `analyze_values`：NULL 分离、排序、精确 NDV、采样和直方图构造的共同实现。
- 私有 `reservoir_sample`：使用固定初始状态和 xorshift 更新规则执行确定性蓄水池采样。
- `coprHandler::handleCopAnalyzeRequest`：ANALYZE 的本地分派与响应/错误映射入口。
- `coprHandler::{handleAnalyzeIndexReq, handleAnalyzeColumnsReq}`：分别扫描并构造索引统计或逐列统计。
- `pub struct analyzeColumnsExec` 及其 `new`、`Fields`、`getNext`、`Next`、`NewChunk`、`Close`：模拟 Go `RecordSet` 生命周期的惰性扫描游标。

文件没有模块级常量、trait 或条件编译项。命名保留 Go 风格；crate 根的 `#![allow(non_camel_case_types, non_snake_case, ...)]` 允许这些符号形状。顶部 `BTreeMap` 导入在当前文件中未使用，也由 crate 级允许项放行。

## 执行流程

请求分派与公共统计流程如下：

1. `handleCopAnalyzeRequest` 先检查 `request.ranges`。空范围直接返回默认 `Response`，不会扫描 reader。
2. 方法再次以模式匹配确认载荷是 `RequestPayload::Analyze`；若被直接误调用于 DAG/Checksum 请求，同样返回默认响应。
3. `AnalyzeType::Index` 调用 `handleAnalyzeIndexReq`，`AnalyzeType::Columns` 调用 `handleAnalyzeColumnsReq`。
4. 分支成功时调用 `AnalyzeResult::encode` 填充 `Response.data`；`Response` 其余字段维持默认。扫描失败时把 `CopError::to_string()` 放入 `Response.other_error`。

索引分支调用 `reader.scan(ranges, start_ts, false)`，保持升序扫描请求。每个 `KvPair.value` 的所有 `Datum` 按原列顺序连续调用 `Datum::encode`，再整体包装为 `Datum::Bytes`；因此复合索引的一行只作为一个统计值。随后 `analyze_values` 生成统计，返回 `columns = []`、`index = Some(...)`。

列分支同样只扫描一次。它以所有行的最大列数作为宽度；对每个偏移收集一组值，短行缺失位置补 `Datum::Null`，再独立调用 `analyze_values`。空扫描得到宽度零和空 `columns`；结果的 `index` 为 `None`。

`analyze_values` 先计数并移除 NULL，再排序非 NULL 值。NDV 等于排序后相邻不等的次数加上“非空”标记；NULL 不计入 NDV。采样发生在排序之后。直方图把 `bucket_size` 当作目标桶数：先钳制为至少 1，再计算 `rows_per_bucket = ceil(value_count / bucket_size)`，按该块大小切分；每桶 `count` 是累计行数，`repeats` 只统计该块末尾连续等于 `upper` 的值。重复值若跨块，可能分散到多个桶。

## 数据与状态

统计值使用 `copr_handler.rs::Datum`，其顺序规则决定排序、NDV 和桶边界：NULL 最小；有符号、无符号和浮点数可跨数值变体比较；字节串按字节序；不同类型族按标签排序。`Datum::encode` 使用本 crate 自定义的类型标签和大端载荷，这同时决定索引复合值及最终响应中边界/样本的字节表示。

`reservoir_sample` 对前 `sample_size` 个排序值直接克隆；之后每个位置更新本地 `u64` 状态，并以 `state % (index + 1)` 决定是否替换已有样本。`sample_size == 0` 返回空向量；样本容量不会超过值数。固定种子让相同输入产生相同样本，利于测试复现，但它不是密码学随机源。

`analyzeColumnsExec` 持有共享只读 reader 的 `Arc<dyn KvReader>`、范围、MVCC `start_ts`，以及私有的 `rows`、`cursor`、`loaded`。构造后尚未扫描；首次 `getNext` 扫描全部范围并只保留各 `KvPair.value`。`loaded` 保证包括空结果在内也只扫描一次。`Fields` 只查看已缓存首行宽度，所以加载前固定返回 0；它不是请求 schema。`cursor` 单调递增，`Close` 不重置它。

## 依赖与调用关系

本文件直接使用标准库 `Arc`、向量/切片方法和整数编码；`BTreeMap` 是当前未使用的遗留导入。crate 内依赖全部来自 `copr_handler.rs`：`AnalyzeRequest`、`AnalyzeType`、`CopError`、`Datum`、`KeyRange`、`KvReader`、`Request`、`RequestPayload`、`Response`、`Row` 和 `coprHandler`。尽管 `Cargo.toml` 声明了多项 optional 业务 crate，本文件没有直接引用它们。

RustCodeGraph 与源码共同确认以下边：

- `rpc_copr.rs::coprRPCHandler::HandleCmdCop` 调用 `coprHandler::handle_request`，后者对 `RequestPayload::Analyze` 调用 `handleCopAnalyzeRequest`。
- `handleCopAnalyzeRequest` 调用 `handleAnalyzeIndexReq` 或 `handleAnalyzeColumnsReq`，成功后调用 `AnalyzeResult::encode`。
- 两个分析分支都下调 `KvReader::scan` 和 `analyze_values`；`analyze_values` 下调 `reservoir_sample`。
- `analyzeColumnsExec::Next` 调用 `getNext`。图索引还显示该 `Next` 被 `cop_handler_dag.rs::handleCopDAGRequest`/`drainRowsFromExecutor` 以及独立测试引用；不过当前 ANALYZE handler 本身没有构造这个执行器，而是直接扫描 reader。

独立测试 `analyze_test.rs` 直接调用 `handleCopAnalyzeRequest`、`analyzeColumnsExec::{Next,getNext,Close}`。`lib.rs` 没有 `pub use analyze::*`，所以这些公开符号主要对 crate 内模块和同 crate 测试可见，并非从 crate 根直接再导出的稳定外部 API。

## 错误处理与边界

- 空 `ranges` 或载荷类型不符被视为无响应数据的守卫条件，返回完全默认的 `Response`，不写 `other_error`。
- 两个分析分支唯一显式可失败操作是 `KvReader::scan`；`?` 保留 `CopError`，顶层再把其展示字符串降格为 `other_error`。该转换会丢失结构化错误变体，且本方法不设置 `region_error` 或 `locked`。
- `handleCopAnalyzeRequest` 内部编码不返回 `Result`。`analyze_values` 中的 `expect("histogram chunk is non-empty")` 依赖 `values.chunks(rows_per_bucket)` 永不产生空块；前置的非空检查和 `rows_per_bucket.max(1)` 维持该不变量。
- `bucket_size == 0` 被当作 1 个目标桶，不报错；超大桶数会得到每块一行。`sample_size == 0` 合法且无样本。
- 缺列被计为 NULL；真正的 NULL 被排除于排序、NDV、样本和直方图，但计入 `null_count`。
- 索引路径只是连接带自描述标签的 `Datum` 编码，并不读取 Go 请求中的 `NumColumns`、时区或 collation。结果只能作为该 Rust mock 协议的行为，不能据此声称兼容真实 Analyze protobuf。
- `analyzeColumnsExec::Next` 总会先 `chunk.clear()`；扫描错误时目标 chunk 已被清空。到达 EOF 后重复调用持续返回空 chunk，且不重复扫描。

## 并发与资源生命周期

分析 handler 只借用 `&self`，通过 `Arc<dyn KvReader>` 的共享引用调用要求 `Send + Sync` 的 reader；本文件不创建线程、任务、锁或通道，也不修改 handler 共享状态。单次 handler 调用会把扫描结果、每列值、排序副本、样本、桶和最终编码同时或先后保存在内存中，没有流式处理、内存限额或 spill。列路径还会为每一列重新遍历全部行并克隆值，时间与内存成本随行数和最大列宽增长。

`analyzeColumnsExec` 自身通过 `&mut self` 串行推进游标；`Arc` 只延长 reader 生命周期，不让同一个执行器并发消费。首次读取会全量缓存所有行，之后逐行克隆输出。`Close` 是无操作：不会释放缓存、回滚游标或阻止继续调用；对象 drop 时 `rows`、范围和 `Arc` 自动释放。测试确认 `Close` 后继续读取只看到当前游标之后的状态，并且 reader 总扫描次数仍为一次。

## 与 Go 版本的对应关系

同路径 `analyze.go` 提供同名 handler 和 `analyzeColumnsExec`，Rust 保留了空范围守卫、索引/列分支、升序 MVCC 扫描、列执行器 `Next` 清空目标以及 `Close` 无操作等外形，但实现范围明显更窄。

Go 入口还校验 `kv.ReqTypeAnalyze`、请求 Region context，并从 `req.Data` protobuf 反序列化 `tipb.AnalyzeReq`；Rust 请求已经是结构化 `RequestPayload::Analyze`，Region context 由更外层 `HandleCmdCop` 的简化会话检查处理。Go 在 `req.StartTs == 0` 时使用 `StartTsFallback`，Rust 始终直接把 `request.start_ts` 传给 reader。

Go 索引路径使用 `indexScanExec`、`statistics.NewSortedBuilder` 和可选 `CMSketch`，并输出 protobuf `AnalyzeIndexResp`；Rust 把扫描行编码为一个字节值，计算简化直方图、精确 NDV和样本，再输出自有格式。Go 列路径构造字段类型、默认值 decoder、collator、`statistics.SampleBuilder`、FM/CMS sketch 和可选 PK builder，输出 `AnalyzeColumnsResp`；Rust 只按向量偏移统计 `Datum`，缺列补 NULL。因此两者共享“mock ANALYZE 扫描并汇总统计”的目的，但 Rust 当前不是逐字段等价移植。

Go `analyzeColumnsExec::Fields` 返回预建的 `ResultField` schema，`Next` 从 `tableScanExec` 流式拉取一行并把 codec NULL flag 转成 SQL NULL；Rust `Fields` 返回已加载首行的宽度，且执行器首次调用便缓存整个扫描。当前 Rust 独立测试只覆盖请求守卫与执行器生命周期，没有直接断言直方图、NDV、采样、索引编码或错误响应。目录内没有同路径 `analyze_test.go`；Go 行为依据来自 `analyze.go` 本身及相邻 Go 请求链。

## 扩展指南

- 若扩展简化统计语义，主要接入点是 `analyze_values`。修改分桶、NDV、NULL 或采样规则时，应在独立 `analyze_test.rs` 增加重复值跨桶、全 NULL、空输入、`bucket_size`/`sample_size` 为零和确定性样本测试；不要把测试嵌入生产文件。
- 若修改响应格式，必须同步检查 `AnalyzeResult::encode`、`encode_column`、`Datum::encode` 以及所有消费端。当前格式没有版本号；字段顺序或整数宽度变化存在静默兼容风险。
- 若要提高与 Go 的兼容度，不能只补一个字段：需要明确引入 tipb 请求/响应、`start_ts` fallback、context/region 错误映射、类型与 collation、CMS/FM sketch、PK histogram 和默认值解码的范围，并逐项对照 `analyze.go`。这属于较大子系统迁移，不应把当前自有编码误标为 protobuf。
- 若优化大数据量资源使用，应优先评估让列/索引统计增量消费扫描结果，以及消除列路径对每个单元格的重复克隆；但流式直方图和确定性采样必须保持排序/NDV契约，且要评估结果与 Go builder 的差异。
- 修改 `analyzeColumnsExec` 时需保持“首次访问最多扫描一次”“`Next` 先清空目标且最多追加一行”“`Close` 不回绕”这些已测试行为。若要让 `Fields` 表示 schema，应新增显式字段元数据，不能继续从尚未加载的行推导。
- 正确性风险主要是简化 `Datum` 排序与真实 SQL 类型/collation 不一致；兼容风险主要是自有响应格式；性能风险主要是全量扫描、排序、逐列克隆和最终编码的峰值内存。

## 验证依据

- `pkg/store/mockstore/mockcopr/analyze.rs`：RustCodeGraph `node --file` 完整读取 1–297 行，核对 22 个已索引符号、分支、编码、统计算法和执行器状态。
- `pkg/store/mockstore/mockcopr/copr_handler.rs`：`CopError`、`Datum::{Ord,encode}`、`KvReader`、`AnalyzeType`、`AnalyzeRequest`、请求/响应类型、`coprHandler` 及 `handle_request` 分派。
- `pkg/store/mockstore/mockcopr/rpc_copr.rs`：`coprRPCHandler::HandleCmdCop` 的请求上下文检查和外层调用入口。
- `pkg/store/mockstore/mockcopr/Cargo.toml`、`lib.rs`：crate 名、lib 入口、Go 包映射、optional 依赖边界、模块及独立测试装配；目标包目录未发现 `doc.go`。
- `pkg/store/mockstore/mockcopr/analyze.go`：Go handler、索引/列统计构建和 `analyzeColumnsExec` 的完整对照证据。
- `pkg/store/mockstore/mockcopr/analyze_test.rs`：`analyze_request_guards_match_go` 证明空范围/错误载荷不扫描；`analyze_columns_exec_matches_record_set_lifecycle` 证明 `Next` 清空目标和空扫描只执行一次；`analyze_columns_close_does_not_rewind_the_record_set` 证明 `Close` 不回绕且不重扫。
- RustCodeGraph：`status` 报告索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/store/mockstore/mockcopr` 列出目标 Rust/Go/测试文件；`query` 定位 Rust、同路径 Go 和 unistore 的同名符号；窄化 `explore` 明确给出 `handle_request → handleCopAnalyzeRequest`、handler 内两分支以及 `analyzeColumnsExec::Next` 的调用者。单独 `callers handleCopAnalyzeRequest` 在 90 秒内无输出后中止，调用边已由图探索和源码调用点交叉复核。

本任务是纯文档分析，按计划未运行 Cargo。人工复核确认本文区分了当前 Rust mock 与 Go 完整统计实现，能够回答文件为何存在、请求如何运行、状态和错误如何流动，以及安全扩展时需要修改和补测的位置。
