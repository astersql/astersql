# `br/pkg/stream/logging_helper.rs`

## 文件定位

遍历上游库 ID 到 `DBReplace` 的映射。

## 核心职责

本文件只提供 `LogDBReplaceMap`，其职责是：

1. 遍历上游库 ID 到 `DBReplace` 的映射。
2. 忽略 `DBReplace::FilteredOut == true` 的整个库。
3. 为每个未过滤库生成一行，写入标题、库名、上下游库 ID，再追加未过滤表的名称、上下游表 ID 和分区 ID 对。
4. 仅在调用者传入 `Some(&mut Vec<String>)` 时保存生成的行；`None` 不产生可观测输出。

它是诊断/可观测辅助，不参与恢复决策，也不修改传入的替换映射。

## 主要符号

- `pub fn LogDBReplaceMap(title: &str, dbReplaces: &HashMap<UpstreamID, DBReplace>, mut out: Option<&mut Vec<String>>)`：唯一公开函数。`title` 是每行前缀；`dbReplaces` 是只读的库/表/分区替换树；`out` 是可选文本收集器。函数返回 `()`，不报告错误。
- `std::collections::HashMap`：同时是输入容器及 `DBReplace::TableMap` / `TableReplace::PartitionMap` 的实际容器，它的迭代顺序不构成 API 保证。
- `crate::stubs::UpstreamID`：`i64` 类型别名，用于库、表和分区的上游 ID（`br/pkg/stream/stubs.rs`）。
- `crate::stubs::DBReplace`：包含 `Name`、`DbID`、`TableMap`、`FilteredOut` 和 `Reused`。本函数读取前四者中除 `Reused` 外的相关字段，不输出 `Reused`。
- `crate::stubs::TableReplace`：通过 `DBReplace::TableMap` 间接访问，读取 `Name`、`TableID`、`PartitionMap` 和 `FilteredOut`；`IndexMap` 不在输出中。

本文件没有模块常量、自定义类型、trait、`impl` 或条件编译项。

## 执行流程

1. `LogDBReplaceMap` 从 `dbReplaces` 取出 `(upstream_db_id, db_replace)`。
2. 若库的 `FilteredOut` 为真，立即 `continue`：该库、其下所有表和分区均不会出现在摘要中。
3. 用 `format!` 创建库级文本：`title`、`dbName`、`upstreamId` 和 `downstreamId`。
4. 遍历 `db_replace.TableMap`。若表的 `FilteredOut` 为真则跳过该表；否则用 `push_str(format!(...))` 将表名及上下游表 ID 追加到同一行。
5. 对每个未过滤表遍历 `PartitionMap`，将每组 `up partition` / `down partition` ID 追加到该库的同一行。
6. 若 `out` 为 `Some`，把完整行 `push` 进向量；若为 `None`，丢弃已构建的字符串。
7. 对所有未过滤库重复上述过程，然后返回。空映射或全部过滤的映射不会追加任何行。

## 数据与状态

函数的输入形成三层只读结构：`HashMap<UpstreamID, DBReplace>` → `DBReplace::TableMap` → `TableReplace::PartitionMap`。上游 ID 是各层 `HashMap` 的键，下游 ID 分别保存在 `DbID`、`TableID` 和分区映射的值中。

唯一可变状态是函数内部的 `String` 和可选借用的 `Vec<String>`。输入映射仅被共享借用，不会改写 ID、过滤标记或名称。输出粒度是“每个未过滤库一行”，而不是每表或每分区一行。

HashMap 的库、表和分区遍历顺序均不稳定，因此调用者不应依赖行顺序或行内字段组的顺序。`parity_test.rs` 只断言行数和子串，没有锁定整行顺序，与这一容器特性一致。

## 依赖与调用关系

- 模块接入：`br/pkg/stream/lib.rs` 声明并扁平再导出 `logging_helper`，所以外部 crate 理论上可通过 `astersql_br_pkg_stream::LogDBReplaceMap` 调用。
- 下游依赖：仅直接依赖标准库 `HashMap` 以及同 crate `stubs.rs` 的 `UpstreamID` / `DBReplace`；`Cargo.toml` 没有为此文件引入日志 crate。
- Rust 调用者：`br/pkg/stream/parity_test.rs::go_rust_public_contract_matches` 构造一个库映射，传入收集向量，断言产生一行且包含 `downstreamId=101`。未搜索到 Rust 生产调用者。
- Go 生产上游：`br/pkg/restore/log_client/client.go` 在基础库替换信息形成后记录；`br/pkg/task/stream.go` 分别在 snapshot restore 前扫描日志 meta KV 之后、以及构建 rewrite rules 之前记录映射。这些调用反映文件在完整 BR 恢复链中的预期位置，但不是 Rust 端已接线的证据。
- 数据产生方：`br/pkg/stream/table_mapping.rs` 维护 `TableMappingManager::DBReplaceMap`，`br/pkg/stream/rewrite_meta_rawkv.rs` 中 `SchemasReplace::DbReplaceMap` 用于实际 ID 重写；本函数只消费同形数据。

## 错误处理与边界

`LogDBReplaceMap` 无 `Result` 返回值，也不调用可失败的 I/O；因此没有显式错误传播路径。`format!` 和 `String` / `Vec` 扩容的内存分配失败仍遵循 Rust 运行时的全局分配失败行为，不由本 API 恢复。

重要边界如下：

- `dbReplaces` 为空、或所有库都被过滤：输出不变。
- 库未过滤但无表，或其表全被过滤：仍生成库级行。
- 表被过滤：其表信息和全部分区映射一并省略。
- `out == None`：不保存输出，但当前实现仍执行遍历与字符串构建，所以它是“静默”而不是零成本 no-op。
- 名称和 `title` 不做转义或截断；若包含换行或类似 `key=value` 的文本，会原样进入摘要，这不是结构化日志保证。
- 本函数不输出 `IndexMap` 和 `Reused`，不能将其文本视为 `DBReplace` 的无损序列化。

## 并发与资源生命周期

文件不创建线程、异步任务、通道、锁、事务或外部资源。`dbReplaces` 是调用期间的不可变借用；`out` 是调用期间的独占可变借用，编译器保证函数执行时没有其他安全 Rust 代码并发修改该向量。本函数不保存任何借用，返回后所有临时 `String` 按所有权规则释放，已 `push` 的字符串所有权属于调用者的向量。

计算和临时内存成本与未过滤的库、表、分区数量及名称/数字文本长度线性相关。在 `None` 路径中也会付出这些构建成本；在 `Some` 路径中，最终字符串会持有到调用者删除它们为止。

## 与 Go 版本的对应关系

已验证的差异是：

- Go 版接收 `map[UpstreamID]*DBReplace`，Rust 版接收 `&HashMap<UpstreamID, DBReplace>`；Rust 数据不存在 nil `*DBReplace` 分支。
- Go 版用 `pingcap/log.Info` 和 `zapcore.Field` 直接生成结构化 info 日志，每库一次日志调用；Rust 版拼接单个非结构化 `String`，且只在 `out=Some` 时可观测。
- Go 版已有三个生产调用点；Rust 搜索只发现 `parity_test.rs` 的测试调用。因此当前 Rust 实现是可测的迁移辅助，不应宣称已达到 Go 的生产日志效果。

## 扩展指南

- 若接入 Rust 生产恢复流程，应在与 Go 三个调用点等价的阶段显式调用，并先决定是引入真实 logger，还是由上层消费收集字符串。不要把当前 `None` 当作已记日志。
- 若要稳定输出以便 golden test 或机器消费，需在 `LogDBReplaceMap` 中显式排序库、表和分区 ID，或改用有序容器；这会引入 `O(n log n)` 排序成本，应评估大规模映射下的性能。
- 若要恢复 Go/zap 的结构化语义，应调整本函数的输出抽象，而非继续扩充不可逆的平面字符串；需同时核对 `Cargo.toml` 依赖和 crate 公开 API 兼容性。
- 若增加 `Reused`、`IndexMap` 或其他字段，应同步检查 Go `logging_helper.go` 是否也应输出，避免两端诊断信息漂移。
- 测试不应内嵌在生产源文件。小幅契约更改可同步 `br/pkg/stream/parity_test.rs::go_rust_public_contract_matches`；若增加空映射、库/表过滤、分区输出、`None` 或顺序策略的细粒度回归，应新建独立 `br/pkg/stream/logging_helper_test.rs` 并仅在 `lib.rs` 的 `cfg(test)` 下挂载。
