# `pkg/ddl/sequence.rs`

## 文件定位

本文件属于 `astersql-ddl` crate（见 `pkg/ddl/Cargo.toml`），并由 `pkg/ddl/lib.rs` 以 `pub mod sequence` 公开。它提供一套自包含的 Rust SEQUENCE 元数据模型、CREATE/ALTER 选项归并与校验逻辑，以及一个进程内目录模型。

当前接线程度有限：仓库调用检索只发现 `pkg/ddl/sequence_test.rs` 使用这些公开符号，没有发现 SQL executor、DDL job worker、`meta::Mutator` 或持久化系统表调用它们。因此它不是当前完整应用中实际执行 SEQUENCE DDL 的作业处理器。实际 Go 主链位于 `pkg/ddl/sequence.go`：`onCreateSequence`、`onAlterSequence` 通过 DDL job、元数据事务和 schema version 更新完成持久化操作。本文件应理解为移植中的领域逻辑模型，而非已接线的生产 DDL 通路。

## 核心职责

- `SequenceInfo` 保存序列定义和一个简化的内部进度 `current`。
- `SequenceOption` 统一表达 CREATE/ALTER 可修改的参数。
- `sequence_defaults`、`build_sequence_info`、`apply_sequence_options` 和 `validate_sequence_options` 负责默认值、覆盖规则、重启基数及输入合法性。
- `SequenceCatalog` 用 `(schema_id, ASCII 小写名称)` 作为键，模拟创建、修改和删除序列。
- `SequenceError` 将校验失败和目录操作失败归类为六种无载荷错误。

本文件不生成序列的下一个值，不实现 `nextval`/`lastval`/`setval`，不管理缓存区间，也不提交 DDL job、更新 schema version 或写入 KV。

## 主要符号

- `SequenceInfo { start, min_value, max_value, increment, cache, cycle, comment, current }`：完整定义与本地进度。`cache` 用数值 `1` 表示 `NoCache`，没有与 Go `SequenceInfo.Cache` 等价的独立布尔位。
- `SequenceOption`：包含 `Start`、`MinValue`/`NoMinValue`、`MaxValue`/`NoMaxValue`、`Increment`、`Cache`/`NoCache`、`Cycle`、`Comment` 和 `Restart(Option<i64>)`。`Restart(None)` 表示回到当前 `start`。
- `SequenceError`：`InvalidIncrement`、`InvalidBounds`、`StartOutOfBounds`、`InvalidCache`、`AlreadyExists`、`NotFound`；`Display` 直接输出枚举的 `Debug` 名称。
- `sequence_defaults(increment)`：按步长符号构造正向或负向默认值；零步长暂按正向构造，随后由校验拒绝。
- `build_sequence_info(options)`：创建入口。它先从选项末尾寻找最终 `Increment`，再构造默认值、应用全部选项、设置 `current` 并校验。
- `apply_sequence_options(info, options, altering)`：按切片顺序覆盖已有定义；仅当 `altering` 为真时处理 `Restart`，返回重启目标。
- `validate_sequence_options(info)`：检查步长、边界、起点、缓存及缓存乘步长的溢出风险。
- `restart_sequence_base(value, increment)`：正向返回 `value - 1`，负向返回 `value + 1`，用 wrapping 算术覆盖整数端点。
- `SequenceCatalog::{create, alter, drop_sequence}`：进程内 `BTreeMap` 目录操作；名称仅做 ASCII 大小写归一化。

所有上述类型和自由函数均为公开 API；`SequenceCatalog.sequences` 字段保持私有。

## 执行流程

CREATE 模型从 `build_sequence_info` 开始：

1. 逆序扫描选项，采用最后一个 `Increment`；缺省为 `1`。
2. `sequence_defaults` 根据步长正负选择起点和上下界，缓存默认为 `1000`，关闭 cycle，清空注释。
3. `apply_sequence_options(..., false)` 顺序应用选项；CREATE 中的 `Restart` 被忽略。
4. 将 `current` 设为 `restart_sequence_base(start, increment)`，即让后续一次步进可得到起始值的基数。
5. 再次调用 `validate_sequence_options` 后返回。这里存在重复校验：`apply_sequence_options` 已校验一次，随后 `build_sequence_info` 又校验一次。

ALTER 模型由 `SequenceCatalog::alter` 驱动：先定位条目并克隆旧值，在副本上执行 `apply_sequence_options(..., true)`；全部选项合法才用副本替换原值。若含 `Restart(Some(v))`，目标为 `v`；`Restart(None)` 使用修改后的 `start`；随后 `current` 写为目标前一个基数。失败不会部分修改目录中的原对象。

CREATE/DROP 目录操作分别由 `create` 和 `drop_sequence` 完成。重复 CREATE 配合 `if_not_exists` 返回 `Ok(false)`；缺失 DROP 配合 `if_exists` 返回 `Ok(false)`，否则返回相应错误。

## 数据与状态

`SequenceInfo` 是可克隆的值对象。其范围不变量由 `validate_sequence_options` 给出：`increment != 0`；`min_value < max_value`；上下界不得分别等于 `i64::MIN`、`i64::MAX`；`start` 必须落在闭区间内；`cache != 0`；步长绝对值不得大于范围跨度；缓存大小还必须低于 `(i64::MAX - abs(increment)) / abs(increment)`。

溢出相关运算先提升到 `i128`，避免对 `i64::MIN` 求绝对值时溢出。`restart_sequence_base` 刻意使用 `wrapping_sub`/`wrapping_add`：例如正向在 `i64::MIN` 重启得到 `i64::MAX`，负向在 `i64::MAX` 重启得到 `i64::MIN`；`pkg/ddl/sequence_test.rs::restart_value_and_base_arithmetic_match_go` 固定了该行为。

`SequenceCatalog` 的状态只存在于当前 Rust 值拥有的 `BTreeMap` 中，没有磁盘格式、事务、版本号或恢复点。键中的 schema ID 区分命名空间，名称用 `to_ascii_lowercase` 归一化；Unicode 大小写、标识符排序规则和 SQL 标识符规范不在此模型中实现。

## 依赖与调用关系

文件只直接使用 Rust 标准库：`std::fmt`、`std::error::Error`、`String` 和 `std::collections::BTreeMap`；因此 `pkg/ddl/Cargo.toml` 中没有为本文件单独引入外部 crate。crate 边界由 `[lib] path = "lib.rs"` 和 `lib.rs::pub mod sequence` 建立。

RustCodeGraph 给出的 `build_sequence_info` 下游边为 `sequence_defaults`、`apply_sequence_options`、`restart_sequence_base`、`validate_sequence_options`；`apply_sequence_options` 下游边为后两者。调用方查询没有返回生产调用者，文本检索也只定位到 `pkg/ddl/sequence_test.rs` 的导入和测试调用。因此不能从当前代码证明它已经连接到 `pkg/ddl/executor.rs`、job scheduler 或 KV 元数据层。

Go 对照主链则是：DDL worker 调用 `onCreateSequence`/`onAlterSequence`；前者经 `createSequenceWithCheck` 调用 `meta.Mutator.CreateSequenceAndSetSeqValue`，后者经 `updateVersionAndTableInfo` 保存定义，必要时由 `restartSequenceValue` 调用 `meta.Mutator.RestartSequenceValue`。这组持久化、job 状态和 schema version 依赖均不在本 Rust 文件内。

## 错误处理与边界

所有可预期失败以 `Result` 返回，不发生 I/O。校验按固定顺序短路，因此同时存在多个问题时只返回第一个类别：先步长，再边界和起点，再缓存，最后检查步长是否超过跨度。

`apply_sequence_options` 会直接修改传入值，然后才校验；调用者若直接使用它，出错后参数可能已经被部分覆盖。只有 `SequenceCatalog::alter` 通过“克隆—校验—替换”提供原子式内存更新。扩展调用者必须沿用该模式，不能假设函数失败时输入未变。

选项重复时通常是“最后一个生效”；`build_sequence_info` 也显式用最后一个 `Increment` 决定初始默认范围。`Restart` 在 CREATE 模式被静默忽略。重启目标本身不接受 `[min_value, max_value]` 校验，这与 Go 的 ALTER 行为及现有 Rust 测试一致。

目录方法区分幂等请求和错误：`if_not_exists`/`if_exists` 仅把相应冲突转换为 `Ok(false)`，不会返回已有对象或缺失对象的详情。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务或外部资源。`SequenceCatalog` 的修改方法需要 `&mut self`，并发协调完全交给拥有者；它本身既不是共享目录，也没有跨进程一致性。

对象生命周期与普通 Rust 所有权一致：创建时把 `SequenceInfo` 移入 map；ALTER 克隆后提交；DROP 移除并丢弃值。`comment` 在应用选项时克隆。该模型无法提供 Go DDL 的 owner failover、job 恢复、schema 同步或 nextval 并发分配保证。

## 与 Go 版本的对应关系

`sequence_defaults`、`build_sequence_info` 和 `validate_sequence_options` 对应 `pkg/ddl/sequence.go` 的默认填充、`buildSequenceInfo` 和 `validateSequenceOptions`；`apply_sequence_options(..., true)` 对应 `alterSequenceOptions`；`restart_sequence_base` 对应 `restartSequenceValue` 中的基数计算。Rust 的 ALTER 复制后提交与 Go `onAlterSequence` 的 `copySequenceInfo := *tblInfo.Sequence` 意图一致。

两者并非完整等价：

- Go 的 CREATE 会在所有选项扫描后根据最终步长及“是否显式设置”补默认 min/max/start；例如只提高 `MINVALUE` 时，默认 START 会取 `max(min, 1)`。Rust 先构造默认值再逐项覆盖，可能使同类输入因旧默认 START 越界而失败。
- Go 用 `Cache` 布尔值加 `CacheValue` 表达 NOCACHE；Rust 折叠为 `cache = 1`，因此无法保留这两个元数据字段的区别。
- Go CREATE 接受 AST 和 table options，并拒绝不支持的表选项；Rust 只接受 `SequenceOption`，没有 AST、标识符、权限或 table option 错误上下文。
- Go 错误映射到带 schema/name 的 TiDB `dbterror`；Rust 错误无上下文，仅打印枚举名。
- Go 主链持久化 `TableInfo.Sequence`、推进 job 状态并更新 schema version；Rust `SequenceCatalog` 仅为内存模型，DROP 也没有对应 Go DDL job 行为。

`pkg/ddl/sequence_test.go` 的 `TestCreateSequence`、`TestSequenceFunction` 和 `BenchmarkInsertCacheDefaultExpr` 是完整 SQL 行为基准。Rust 测试中的前两个大型 `*_draft_structure` 仅记录 SQL 步骤数量和期望文字，不驱动真实 SQL 引擎；只有末尾四个测试直接执行本文件逻辑。

## 扩展指南

若补齐选项语义，优先修改 `build_sequence_info`/`apply_sequence_options`，并在独立的 `pkg/ddl/sequence_test.rs` 增加针对选项顺序、正负步长默认范围、重复选项和 NOCACHE 表达的可执行断言。不要把测试内嵌回生产源文件。

若要接入真实 DDL，不能只扩展 `SequenceCatalog`：应复用 crate 现有 job/元数据/schema-version 框架，并逐项对齐 `pkg/ddl/sequence.go::{onCreateSequence,onAlterSequence,createSequenceWithCheck,restartSequenceValue}`。需要明确持久化格式、取消/重试、owner failover、版本同步及并发 nextval 与 RESTART 的非连续风险；Go 注释明确指出 ALTER RESTART 与旧定义下继续分配可能导致不连续、非单调。

新增错误时应决定其属于输入校验还是目录操作，并保留 `Result` 边界；若要兼容 SQL 层，还需在更高层映射为带对象名的 TiDB 错误码。任何直接调用 `apply_sequence_options` 的新路径都应先复制旧值，避免失败后残留部分修改。

性能方面，`BTreeMap` 提供有序的对数复杂度查找，但当前无遍历顺序契约；若替换容器或引入共享锁，需补充名称规范化、锁粒度和跨 schema 隔离测试。真实缓存区间分配应检查乘法溢出和持久化原子性，不能把当前 `cache` 字段误当成已实现的缓存分配器。

## 验证依据

- 生产源：`pkg/ddl/sequence.rs`，核对全部两个数据类型、两个枚举、五个自由函数/trait 实现相关逻辑，以及 `SequenceCatalog` 的三个方法。
- 模块与 crate：`pkg/ddl/lib.rs` 的 `pub mod sequence`、测试期 `mod sequence_test`；`pkg/ddl/Cargo.toml` 的 crate 名、`[lib]` 路径和依赖边界。
- Rust 测试：`pkg/ddl/sequence_test.rs`；直接行为证据为 `sequence_rejects_i64_endpoint_bounds_like_go`、`sequence_rejects_cache_increment_overflow_like_go`、`failed_alter_does_not_mutate_sequence_like_go`、`restart_value_and_base_arithmetic_match_go`。其余 SQL 步骤测试被标明为迁移草稿而非执行引擎验证。
- Go 对照：`pkg/ddl/sequence.go` 的 `onCreateSequence`、`createSequenceWithCheck`、`handleSequenceOptions`、`validateSequenceOptions`、`buildSequenceInfo`、`alterSequenceOptions`、`onAlterSequence`、`restartSequenceValue`；`pkg/ddl/sequence_test.go` 的两个测试和一个 benchmark。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点；`query` 定位 `SequenceCatalog`（第 228 行）和 `build_sequence_info`（第 124 行）；`node build_sequence_info` 与 `callees` 确认其四条内部调用边，`callees apply_sequence_options` 确认校验与重启基数两条边。调用方图查询未给出生产调用者，随后以仓库文本检索确认只有测试引用。
- 本任务是文档分析，依计划不运行 Cargo。结构验证要求目标文档存在并且恰有固定的十一个二级标题。
