# `br/pkg/restore/utils/stubs.rs`

## 文件定位

`stubs.rs` 是 `astersql-br-pkg-restore-utils` crate 的本地依赖适配层。crate 入口 `br/pkg/restore/utils/lib.rs` 通过 `#[path = "stubs.rs"] pub mod stubs` 暴露整个模块，并额外把 `AppliedFile` 扁平再导出。`br/pkg/restore/utils/Cargo.toml` 表明该 crate 对应 Go 包 `br/pkg/restore/utils`，自身只直接依赖错误和区间相关的三个工作区 crate；本文件因此用精简 Rust 类型替代 Go 实现依赖的 kvproto、TiDB model/tablecodec/codec、日志与 protobuf clone 能力，避免为 restore utils 的算法和测试引入完整 RPC/存储依赖图。

它不是独立的恢复实现，也不进行 RPC、磁盘 I/O 或真实日志输出。主要生产消费者是同 crate 的 `rewrite_rule.rs`、`merge.rs`、`common.rs`，跨 crate 消费者包括 `br/pkg/restore/restorer.rs`、`br/pkg/restore/misc.rs`、`br/pkg/restore/split/splitter.rs` 以及 log client 对 `AppliedFile` 的实现。RustCodeGraph 将该文件识别为 405 行、58 个符号的已索引生产文件。

## 核心职责

本文件有四组职责：

1. 用 `backuppb::{File, DataFileInfo}`、`import_sstpb::RewriteRule` 和 `model::*` 表达恢复键重写所需的最小数据面；字段只覆盖当前调用链会读取或写入的键范围、表/分区/索引 ID、时间戳过滤和少量文件元数据。
2. 用 `tablecodec` 和 `codec` 提供可排序整数、表键前缀及 memcomparable 字节串的最小编码/解码行为，使 `rewrite_rule.rs` 能按 Go 的字节级语义生成和匹配规则。
3. 用 `AppliedFile` 统一未编码 SST 键范围和已编码日志文件键范围，让 `GetRewriteRawKeys`、`GetRewriteEncodedKeys` 与 `FindMatchedRewriteRule` 可接受不同文件载体。
4. 用 `util`、`redact`、`logutil`、`log` 提供深拷贝、脱敏展示和日志签名兼容边界。其中只有 `log::Panic` 保留终止行为，其他日志级别都是空操作。

文件头注释明确限定这些类型只是本地桩：不能据此宣称已支持真实 protobuf wire 往返、RPC、存储或完整 TiDB 编解码。

## 主要符号

- `backuppb::File`：SST 文件描述，含 `Name`、`StartKey`、`EndKey`、字节/KV 数、CF 和校验字段；`GetStartKey`/`GetEndKey` 返回克隆，`GetName` 返回借用。它实现 `AppliedFile`。
- `backuppb::DataFileInfo`：PiTR 日志文件的路径与已编码键范围，也实现 `AppliedFile`。空 `EndKey` 的开放上界语义留给调用方处理。
- `import_sstpb::RewriteRule`：旧/新键前缀及 `NewTimestamp`、`IgnoreAfterTimestamp`、`IgnoreBeforeTimestamp`。getter 返回克隆；`Display` 通过 `redact::Key` 以十六进制展示前缀。
- `model::{CIStr, PartitionDefinition, PartitionInfo, IndexInfo, TableInfo}`：规则生成需要的元数据子集。`CIStr::new` 同时保存原始名称 `O` 和小写名称 `L`；表包含可选分区及索引列表。
- `metautil::Table`：零字段占位类型，只满足 `common.rs::CreatedTable::OldTable` 的类型引用。
- `tablecodec::{EncodeTablePrefix, GenTablePrefix, GenTableRecordPrefix, GenTableIndexPrefix, EncodeTableIndexPrefix}`：生成 `t + 有序 table_id`、`_r`、`_i + index_id` 等键前缀；`DecodeTableID` 逆向解析表 ID；`PrefixNext` 计算字典序后继前缀。
- `codec::{EncodeInt, EncodeBytes, DecodeBytes}`：实现符号位翻转的大端整数编码，以及每 8 字节一组、附 marker 的 memcomparable 字节编码。
- `util::ProtoV1Clone`：对 `RewriteRule` 做结构体 clone，替代 protobuf 深拷贝路径。
- `redact::Key`：把键格式化成不带 `0x` 的小写十六进制字符串。
- `logutil::{KeyField, FileField, RewriteRuleField}` 与同名构造函数：仅保留调用签名和借用关系，不生成真实 zap field。
- `log::{Panic, Error, Warn, Debug}`：`Panic` 调用 Rust `panic!`；其余函数无副作用。
- `AppliedFile`：公开 trait，要求 `GetStartKey` 和 `GetEndKey` 返回拥有所有权的 `Vec<u8>`；`lib.rs` 将其再导出为 crate 级 API。

本文件没有 feature gate、`cfg` 条件、异步函数或全局可变状态。私有符号包括表键编码常量、codec 分组常量和 `tablecodec::encode_int`。

## 执行流程

典型流程从 `rewrite_rule.rs` 开始，而不是从本文件主动执行：

1. `GetRewriteRules`、`GetRewriteRulesMap` 或 `GetRewriteRuleOfTable` 读取 `model::TableInfo`，用 `tablecodec` 为旧/新表、分区和索引构造前缀，并组装 `import_sstpb::RewriteRule`。
2. 文件校验或匹配阶段通过 `AppliedFile::GetStartKey`/`GetEndKey` 取得范围。`backuppb::File` 代表 raw SST 范围，`DataFileInfo` 或其他上游实现可代表已编码范围。
3. raw 路径直接以 `tablecodec::DecodeTableID` 和旧前缀匹配；encoded 路径先用 `codec::DecodeBytes` 还原原始键。匹配后，`RewriteAndEncodeRawKey` 替换第一个旧前缀并再次 `EncodeBytes`。
4. `PrefixNext` 常用于把 record 前缀变成半开区间上界；`rewrite_rule_test.rs::test_rewrite_file_keys` 分别验证 raw `File` 与 encoded `DataFileInfo` 的正确 API，且证明把 encoded 文件误送入 raw API 不会得到正确结果。
5. 诊断展示时，`RewriteRule` 的 `Display` 调用 `redact::Key`；调用链需要日志参数时可以构造 `logutil` 包装器，但当前桩不会输出日志。

编码细节上，表/索引 ID 先把 `i64` 视为 `u64` 并翻转最高符号位，再按大端写入，因此字典序与有符号整数顺序一致。`EncodeBytes` 总会写出最后一组：完整 8 字节输入后还会追加全 padding 终止组；`DecodeBytes` 逐组验证 marker 和零 padding，最后返回未消费输入与解码值。

## 数据与状态

所有数据都由调用者拥有；getter 通过 clone 返回键，避免借用跨越上层调用生命周期。`File`、`DataFileInfo`、`RewriteRule` 和 model 类型均实现 `Clone`、`Debug`、`Default`、`PartialEq`、`Eq`，适合规则构造与确定性断言。

关键不变量如下：

- 表键前缀固定为字节 `t`，record/index 分隔符分别为 `_r` 和 `_i`；完整索引前缀还包含有序编码的 index ID。
- `DecodeTableID` 只接受普通表键，或先带一个 mode 字节（`x`/`r`）和三字节 keyspace ID 的 API V2 键；非法模式、过短输入或剥离后非 `t` 前缀都返回 `0`。
- `PrefixNext` 从尾部执行带进位加一；当所有字节均为 `0xff`（空输入也落入同一分支）时，在原输入末尾追加 `0`。
- memcomparable 字节编码每组为 8 字节数据加 1 字节 marker，padding 必须全为零；`DecodeBytes` 会清空并复用可选输出缓冲区。
- `CIStr::new` 的 `L` 由 Rust `to_lowercase` 生成，用于 `misc.rs` 按名称匹配分区和索引。

本文件没有缓存、锁、通道、事务或后台任务；所有函数只操作参数和局部值。

## 依赖与调用关系

crate 内直接关系为：

- `lib.rs` 声明 `stubs`，并导出 `AppliedFile`。
- `common.rs` 使用 `metautil::Table` 与 `model::TableInfo` 组成建表中间结构。
- `merge.rs` 使用 `backuppb::File` 表达待合并文件。
- `rewrite_rule.rs` 同时使用 `AppliedFile`、`backuppb`、`codec`、`import_sstpb`、`log`、`logutil`、`model`、`redact`、`tablecodec`、`util`；这是本文件最完整的生产消费者。

跨 crate 的直接证据包括：`br/pkg/restore/restorer.rs` 和 `br/pkg/restore/misc.rs` 引用 `backuppb`；`br/pkg/restore/split/splitter.rs` 引用 `tablecodec`；`br/pkg/restore/log_client/log_file_manager.rs::LogDataFileInfo` 与 `br/pkg/restore/split/sum_sorted.rs` 使用公开的 `AppliedFile`。部分上层 `stubs.rs` 还再导出这里的 `tablecodec`/`codec`，说明本文件是迁移期共享兼容边界，而非 canonical kvproto 实现。

下游仅有标准库 `std::fmt`；其余功能均在文件内实现。Cargo manifest 没有声明 kvproto、grpcio 或日志依赖，这与文件“隔离重量依赖”的定位一致。

## 错误处理与边界

`codec::DecodeBytes` 是本文件主要的可恢复错误入口，错误类型为 `String`：不足 9 字节时报输入不足，marker 导出的 padding 数大于 8 时报告非法 marker，终止组中出现非零 padding 时报告非法 padding。它不会 panic，也不会静默接受损坏编码。

`tablecodec::DecodeTableID` 对非法或过短键返回哨兵值 `0`，不返回 `Result`；调用方若允许真实表 ID 为 0，必须结合前缀合法性自行消歧。`PrefixNext` 对全 `0xff` 和空输入采用“追加零”策略，这更接近本迁移代码使用的 `kv.NextKey` 习惯，不应擅自替换为“无后继”错误。

`log::Panic` 无条件 panic，且忽略传入消息，固定输出 `log.Panic invoked`；`Error`、`Warn`、`Debug` 则完全丢弃消息。因而这些桩只能维持控制流/签名，不能用来验证日志字段或可观测性。`redact::Key` 是十六进制展示而非加密；它避免原始二进制明文直接写出，但不提供机密性保证。

`RewriteRule` getter、`AppliedFile` getter 和 `ProtoV1Clone` 都通过 clone 提供值语义，成本随键或规则大小线性增长。新增大对象字段时需重新评估复制成本。

## 并发与资源生命周期

本文件没有共享可变资源，构造出的值遵循 Rust 所有权自然释放。`KeyField`、`FileField`、`RewriteRuleField` 用生命周期参数把包装器限制在被引用键/文件/规则的有效期内，不持有或延长外部资源生命周期。

并发安全主要来自“无全局状态 + 值克隆”：`rewrite_rule_test.rs::test_set_time_range_filter_race` 启动 100 个线程，每个线程 clone 共享规则并修改自己的 `RewriteRule`，验证时间范围写入不需要桩层锁。该测试覆盖调用方式，但本文件本身没有显式 `Send`/`Sync` 实现或任务管理。

`DecodeBytes` 可接收一个已有 `Vec<u8>` 作为输出缓冲，函数先 `clear` 再复用其容量；返回时缓冲所有权转交给调用者。其余编码函数分配并返回新 `Vec`，没有外部句柄需要关闭。

## 与 Go 版本的对应关系

Go `br/pkg/restore/utils/rewrite_rule.go` 的 `AppliedFile` 同样只包含 `GetStartKey`/`GetEndKey`，注释明确当前两类实现是全量备份/恢复 SST 与 PiTR KV 文件；Rust 用 `File`、`DataFileInfo` 及开放 trait 保留该语义。Go 侧真实类型来自 `kvproto/pkg/brpb`、`kvproto/pkg/import_sstpb`，编码来自 TiDB `tablecodec`/`codec`，clone、脱敏和日志来自 `util`、`redact`、`logutil`；Rust 则把被当前算法触达的子集集中到此文件。

字节语义重点对齐：`encode_int`/`EncodeInt` 使用符号位翻转大端布局；表、record、index 前缀形态与 Go 调用一致；`EncodeBytes`/`DecodeBytes` 保留 8 字节分组和 marker/padding 校验；`DecodeTableID` 支持 TiKV client-go API V2 的四字节 mode/keyspace 前缀。`parity_test.rs::decode_table_id_accepts_api_v2_keyspace_prefixes` 验证 `x`、`r` 两种 mode 和短输入失败分支。

差异和限制必须保留：Rust `metautil::Table` 是空占位；protobuf getter/clone 由普通字段 clone 模拟；日志字段不是 zap field，非 panic 日志无输出；`DecodeTableID` 和 codec 只覆盖当前 restore 测试所需格式；没有 protobuf 未知字段、wire 编码、RPC 或完整 model 行为。因此若上层开始依赖新的 Go 字段或边界，必须先扩充桩并加入对应独立 Rust 测试，不能假设自动兼容。

## 扩展指南

新增能力时按真实消费者选择接入点：

- 规则新增 kvproto 字段：扩展 `import_sstpb::RewriteRule`、getter/clone/equality使用处，并同步 `rewrite_rule_test.rs` 的构造与断言；若字段影响格式化，还要更新 `Display`。
- 文件元数据新增字段：扩展 `backuppb::File` 或 `DataFileInfo`；若影响范围抽象，优先保持 `AppliedFile` 最小化，避免把 SST 专属字段强加给 PiTR 文件。
- 新键格式或 keyspace mode：修改 `tablecodec::DecodeTableID` 或前缀生成函数，并在 `parity_test.rs` 增加 Go 对照边界；编码修改必须同时覆盖正数、负数、极值、短输入、损坏 marker/padding 和半开区间。
- 新 model 元数据参与规则生成：扩展 `model` 类型，并同步 `misc.rs`、`rewrite_rule.rs` 以及各自独立测试文件；不要把测试嵌入 `stubs.rs`。
- 需要真实日志、protobuf 或 RPC 时，应替换调用边界或引入上游带 tag 的依赖，而不是继续把完整子系统堆进桩。需特别评估 API 兼容性、clone 的内存/性能成本以及错误类型从 `String` 迁移后的传播方式。

按仓库约束，Rust 单元测试必须留在独立文件。首选测试位置是 `br/pkg/restore/utils/rewrite_rule_test.rs`（规则、文件键、codec/tablecodec 联动）和 `br/pkg/restore/utils/parity_test.rs`（Go-Rust 公共契约与独立编码边界）；若修改只服务于上层直接消费者，还应同步该消费者同目录测试。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/pkg/restore/utils/stubs.rs` 定位目标；`node --file ... --offset 1 --limit 400` 与末尾补读覆盖全部 405 行；结果标明目标被 restore 的 log client、restorer、snap client 和测试等文件使用。
- RustCodeGraph：读取 `br/pkg/restore/utils/lib.rs`，确认模块声明、crate 级 `AppliedFile` 再导出和四个独立测试模块；读取 `rewrite_rule.rs`，确认规则生成、文件校验、raw/encoded 重写、脱敏与日志调用链。
- Cargo：`br/pkg/restore/utils/Cargo.toml` 确认 package 名、`lib.rs` 入口、Go 包映射，以及仅有 errors/rtree 三个直接依赖。
- Go 对照：`br/pkg/restore/utils/rewrite_rule.go` 确认 `AppliedFile` 两类用途、真实依赖来源、前缀生成、Decode/Encode、clone、脱敏和错误处理语义；`br/pkg/restore/utils/rewrite_rule_test.go` 提供 raw/encoded 文件范围与表/索引映射用例。
- Rust 测试：`br/pkg/restore/utils/rewrite_rule_test.rs` 覆盖 `ProtoV1Clone` 字段选择、memcomparable round-trip、raw/encoded API 分流、`AppliedFile` 自定义实现、规则匹配/校验和 100 线程时间过滤；`br/pkg/restore/utils/parity_test.rs` 覆盖公开契约与 API V2 table ID 解码。
- 本任务只新增说明文档，按计划不运行 Cargo；交付验证仅执行任务指定的 11 章节结构检查，并人工核对上述路径、符号和限制均已写入。
