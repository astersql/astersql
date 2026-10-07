# `pkg/executor/join/joiner.rs`

## 文件定位

本文件属于 `astersql-executor-join` crate，是连接算法与“连接类型语义”之间的结果整形层。`pkg/executor/join/lib.rs` 以 `pub mod joiner` 导出它；Hash Join v1/v2、各类 probe、Merge Join、Index Lookup Join/Hash Join/Merge Join 在找到候选行后调用这里的 `Joiner`，由它计算 other condition、决定 SQL 三值逻辑下的匹配状态，并生成匹配或未匹配结果。

文件由两部分组成：第 1–1286 行是注释化的早期 Go 映射草稿，不能视作当前 API；第 1288 行起才是实际编译的 Rust 实现。当前实现用 `Vec<Value>` 表示行，是 Go `chunk.Row`/`chunk.Chunk` 语义的轻量 Rust 版本，而不是 Go 实现的逐类型对象层次复刻。

crate 边界由 `pkg/executor/join/Cargo.toml` 定义：库入口是同目录 `lib.rs`，包名为 `astersql-executor-join`。本文件实际代码只直接依赖同 crate 的 `row_table_builder::Value` 和标准库 `Arc`；Cargo 中大量 Windows 条件依赖服务于整个 join crate，并非本文件逐项直接使用。

## 核心职责

1. 通过 `JoinType` 统一表达 Semi、AntiSemi、LeftOuterSemi、AntiLeftOuterSemi、Left/Right/FullOuter 和 Inner 八种结果语义。
2. 通过 `Predicate` 顺序求值 other conditions，保留 SQL 的 `true`、`false`、`NULL` 三态；CNF 中 `false` 支配此前出现的 `NULL`。
3. 支持两种探测方向：`try_to_match_inners` 用一行 outer 扫描多行 inner；`try_to_match_outers` 用一行 inner 扫描多行 outer。
4. 对匹配行执行左右顺序修正和 inline projection；对未匹配行执行 anti/semi 标记或 outer join 默认 inner 补行。
5. 为 null-aware anti join（NAAJ）保存键两侧 NULL 组合，并按 Go 的短路语义生成 `false` 或 `NULL` 标记。

本文件不负责构建哈希表、比较 join key、spill、worker 调度或扫描索引；这些职责分别位于 `hash_join_v1.rs`、`hash_join_v2.rs`、各 probe 文件及 index/merge join 文件中。

## 主要符号

- `Row = Vec<Value>`：一行连接数据。`Value` 定义在 `row_table_builder.rs`，可表示 `Null`、布尔、整数、浮点、字节和文本。
- `Predicate`：`Arc<dyn Fn(&[Value]) -> Result<Option<bool>, String> + Send + Sync>`。`Some(true)` 表示通过，`Some(false)` 表示拒绝，`None` 表示 SQL NULL；闭包错误以 `String` 返回。
- `JoinType`：连接类型枚举。`FullOuter` 是当前 Rust API 的扩展；Go `joiner.go` 的工厂和类型判别没有对应分支。
- `OuterRowStatus::{Unmatched, Matched, HasNull}`：outer-build 批量探测时，每个 outer 相对当前 inner 的状态。
- `NaajType`：NAAJ 的左右键 NULL 组合；`Unknown` 是默认值。
- `Joiner`：不可变配置对象，保存 `join_type`、`null_aware`、`outer_is_right`、默认 inner 行、条件闭包和左右投影列下标。
- `MatchResult`：`try_to_match_inners` 的返回摘要；`matched` 表示至少一行通过，`has_null` 供 anti/semi miss 处理，`consumed` 记录逻辑消耗的 inner 数量。
- `Joiner::new`：唯一构造入口。拒绝零 `max_chunk_size`，并拒绝在非 AntiSemi/AntiLeftOuterSemi 上启用 null-aware；参数本身不会保存 chunk 大小。
- `try_to_match_inners`、`try_to_match_outers`、`on_miss_match`：三条主要运行时入口。
- `evaluate`、`join_rows`、`select`、`project_outer`、`project_joined`、`with_marker`、`naaj_match_row`：条件求值、左右拼接、投影和标记生成的内部辅助函数。

## 执行流程

`try_to_match_inners` 的流程是：空 inner 集合直接返回默认 `MatchResult`；随后逐行按 `outer_is_right` 组装完整 joined row，调用 `evaluate` 求值；普通 Anti/Semi 家族按类型累积 `has_null`；匹配后根据 `JoinType` 输出 outer、追加布尔标记、输出 NAAJ 标记，或输出左右投影后的完整连接行。Semi 家族命中后短路，并把 `consumed` 设为 `inners.len()`，表达“调用方无需继续处理这一批”的语义；普通 Inner/Outer 会继续输出本批所有匹配行。

`try_to_match_outers` 的流程与方向相反：对每个 outer 和固定 inner 求值，先生成 `Matched/HasNull/Unmatched`，再只为匹配项输出结果。Semi 的条件 NULL 被降为 `Unmatched`；其他非匹配类型可报告 `HasNull`。null-aware AntiSemi/AntiLeftOuterSemi 的 outer-build 路径当前直接返回空状态，保持 Go 对应方法仍为 TODO 的能力边界。

`on_miss_match` 由上游在“没有同 key inner”“outer 侧过滤失败”或“候选连接行全被 other condition 过滤”后调用：Semi/Inner 不输出；普通 AntiSemi 仅在没有 NULL 时输出 outer；null-aware AntiSemi 直接输出 outer；LeftOuterSemi/AntiLeftOuterSemi 分别追加 false/true，若 `has_null` 则追加 NULL；三种 outer join 用 `default_inner` 补齐，空默认行会退化为单个 `Value::Null`。

`evaluate` 按条件顺序执行：任意 `false` 立即返回 `(false, false)`；否则记录是否见过 NULL；全部条件结束后，仅在没有 NULL 时匹配。该顺序解释了 `joiner_test.rs::false_condition_overrides_earlier_null` 的断言。

## 数据与状态

`Joiner` 的配置在构造后不再修改。`default_inner` 是 outer join miss 时拼接的 owned row；`conditions` 通过 `Arc` 共享闭包；`left_used/right_used` 的 `None` 表示保留全部列，`Some(empty)` 表示输出该侧零列。`outer_is_right` 同时决定条件求值行和最终输出行的左右顺序，投影索引始终按原始 left/right schema 解释。

`MatchResult.has_null` 只应影响普通 AntiSemi、LeftOuterSemi、AntiLeftOuterSemi。Semi 与普通 Inner/Outer 不向调用方暴露条件 NULL；null-aware anti 路径把 other condition 的 false/NULL 都视为无效 inner，也不传播 `has_null`。`consumed` 是逻辑控制信号，不是底层迭代器游标：当前 API 接收切片且不会改变调用方集合。

`max_chunk_size` 只在 `new` 中检查非零，之后不保存在结构体中，也不限制 `Vec<Row>` 输出规模。因而当前 Rust 的内存/批大小行为与 Go `chunk.RequiredRows`、`InitChunkSize`、`MaxChunkSize` 不等价。

## 依赖与调用关系

模块出口是 `pkg/executor/join/lib.rs -> joiner`。RustCodeGraph 的 `callees try_to_match_inners` 显示它调用本文件的 `evaluate`、`join_rows`、`project_outer`、`project_joined`、`with_marker`、`naaj_match_row`；`Value` 来自 `row_table_builder.rs`。

主要上游调用链由源码搜索核实：

- `hash_join_v1.rs`、`hash_join_v2.rs` 在 build/probe 匹配后调用三条主要入口，并据 `OuterRowStatus` 或 `MatchResult` 决定 miss 输出。
- `inner_join_probe.rs`、`outer_join_probe.rs`、`semi_join_probe.rs`、`anti_semi_join_probe.rs`、`left_outer_semi_join_probe.rs` 通过其 context 中的 `Joiner` 生成 probe 结果。
- `base_semi_join.rs` 集中调用 `try_to_match_inners` 和 `on_miss_match`，承接 semi/anti probe 的公共流程。
- `merge_join.rs`、`index_lookup_join.rs`、`index_lookup_hash_join.rs`、`index_lookup_merge_join.rs` 在各自找到候选 inner 集合后复用同一语义层。

RustCodeGraph 当前能解析目标符号与下游边，但对这些方法形式的 callers 查询返回空；上述上游边因此以相邻 Rust 源码的直接调用位置作为补充证据，而不是声称图索引已覆盖。

## 错误处理与边界

构造错误包括 `max_chunk_size == 0` 和在非 anti 类型上启用 `null_aware`。运行时错误仅来自 `Predicate`，由 `evaluate` 使用 `?` 原样向 `try_to_match_*` 返回；发生错误前已经写入 `output` 的行不会回滚，调用方若需原子批次必须自行隔离输出。

`select` 使用 `filter_map(row.get(index))`：越界投影下标会被静默忽略，而不是报错。这与强 schema 校验不是同一保证，扩展构造路径时应在更上游验证列索引。

NAAJ outer-build 路径返回空 `Vec`，不是“所有行 unmatched”；调用方必须把它理解为尚未实现/不适用。`NaajType::Unknown` 在 `naaj_match_row` 中输出 NULL 标记，避免把未知状态误判为 false。空 `default_inner` 会补一个 NULL，无法从本文件推断真实 inner schema 宽度。

## 并发与资源生命周期

本文件没有锁、任务、通道、文件或网络资源。`Joiner` 的运行方法接收 `&self`，内部不保存可变游标或临时输出；`Predicate` 要求 `Send + Sync` 且由 `Arc` 持有，因此不可变 `Joiner` 可安全共享其条件闭包。实际输出通过调用方独占的 `&mut Vec<Row>` 写入，并发 worker 不应共享同一个可变输出缓冲。

`#[derive(Clone)]` 对 `default_inner` 和投影索引做深拷贝，对 `Predicate` 只增加 `Arc` 引用计数。所有输入/输出行都是 owned `Value` 集合，拼接与投影会克隆值；其时间和内存成本随候选行数、列数及变长 `Bytes/Text` 大小增长，没有 Go chunk 的列式复用和临时 chunk 生命周期优化。

## 与 Go 版本的对应关系

`pkg/executor/join/joiner.go` 是语义基准：Go 的 `Joiner` interface 对应 Rust 单一 `Joiner` 加 `JoinType` 分派；Go 的多个具体 joiner 类型对应 Rust `match self.join_type`；`outerRowStatusFlag`、`NAAJType`、`TryToMatchInners`、`TryToMatchOuters`、`OnMissMatch` 均有直接概念映射。

已保持的关键语义包括：左右顺序由 outer 方向决定；inline projection 在结果输出时生效但条件读取完整 joined row；Semi 命中后早停；普通 anti/semi 保留 condition NULL；false 支配 CNF 中此前 NULL；NAAJ other condition 不传播 NULL；outer join miss 拼默认 inner；null-aware outer-build 路径仍未实现。

重要差异是：Rust 使用 `Vec<Value>`/闭包而非 session context、expression tree 和 chunk；没有 Go 的 iterator error、required rows、临时 filter chunk、向量化表达式开关和深拷贝 expression；Rust 构造器返回 `Result` 并校验参数；Rust 增加 `FullOuter`；`max_chunk_size` 只校验不执行容量限制；Rust `select` 静默跳过越界列。Go 测试 `TestJoinerOtherConditionChunkUsesInitChunkSize` 与 `TestRequiredRows` 覆盖的 chunk 容量行为在当前 Rust 模型中没有等价实现，不能写成已支持。

## 扩展指南

新增 join 类型时，应同步检查 `JoinType`、`try_to_match_inners`、`try_to_match_outers`、`on_miss_match`、`is_semi_join_without_condition`，并明确它是否传播 condition NULL、何时短路、miss 时输出什么。新增 NAAJ 状态时还要更新 `NaajType` 和 `naaj_match_row`，避免 `Unknown` 或 NULL 键组合落入错误布尔结果。

改变左右布局或投影时，应保持两个不变量：condition 始终读取未裁剪的完整 joined row；`left_used/right_used` 始终相对于原始左右 schema，而非 outer/inner 角色。相关测试应放在独立的 `pkg/executor/join/joiner_test.rs`，并联动 probe/hash/index/merge 的独立测试，而不要把测试内嵌到本文件。

若要接近 Go 的性能与容量语义，需要显式设计 chunk/iterator 层，而不能只使用传入但不保存的 `max_chunk_size`。这会影响所有上游调用者、输出部分成功时的错误契约、值克隆成本和 RequiredRows 行为，属于跨文件改造。若只增加 predicate，必须说明错误后的部分输出是否允许保留，并测试 false/NULL 顺序。

## 验证依据

- 目标实现：`pkg/executor/join/joiner.rs`，实际符号位于第 1288–1594 行；第 1–1286 行仅是注释化迁移草稿。
- crate 与模块：`pkg/executor/join/Cargo.toml`、`pkg/executor/join/lib.rs`。
- Rust 单元证据：`pkg/executor/join/joiner_test.rs` 覆盖 Inner/LeftOuter 投影与 miss、三态 status、Semi 早停、NAAJ、非法构造、普通 join 不暴露 NULL、false 覆盖 earlier NULL。
- Go 对照：`pkg/executor/join/joiner.go`、`pkg/executor/join/joiner_test.go`；后者额外验证 InitChunkSize 和 RequiredRows，明确了当前 Rust 尚未对齐的 chunk 行为。
- RustCodeGraph：`status` 确认索引含本文件；`query Joiner --kind struct`、`query try_to_match_inners/try_to_match_outers/on_miss_match --kind function` 定位真实符号；`callees try_to_match_inners` 核对内部调用；callers 空结果由 `rg` 的直接调用证据补充。
- 上游源码证据：`hash_join_v1.rs`、`hash_join_v2.rs`、`base_semi_join.rs`、各 probe、`merge_join.rs`、`index_lookup_join.rs`、`index_lookup_hash_join.rs`、`index_lookup_merge_join.rs`。
- 本任务是只读行为分析和文档新增，按计划不运行 Cargo；结构验证仅检查文件存在且恰有十一个固定二级标题。
