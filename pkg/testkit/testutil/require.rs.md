# `pkg/testkit/testutil/require.rs`

## 文件定位

本文件属于 `astersql-testkit-testutil` crate，是测试辅助层中的断言、固定夹具与随机字符串工具实现，不在数据库服务的 SQL 请求生产链路上。`pkg/testkit/testutil/lib.rs` 通过私有 `require` 模块 `include!("require.rs")` 引入实现，再以 `pub use require::*` 将本文件的公开函数提升到 crate 根；因此使用者看到的是 `astersql_testkit_testutil::DatumEqual` 等 API，而不是公开的 `require` 子模块。

crate 边界由 `pkg/testkit/testutil/Cargo.toml` 定义。与本文件直接相关的依赖是 `rand = "0.9"`；`Datum`、二进制排序规则和 `Handle` 则分别经 `lib.rs` 中的 `types`、`collate`、`kv` 重导出模块来自 `astersql-util-codec` 与 `astersql-kv`。Cargo 元数据把该 crate 对应到 Go 包 `pkg/testkit/testutil`。

## 核心职责

本文件提供七个公开测试辅助函数和两个模块级固定数据：

- `DatumEqual`、`HandleEqual` 将常用领域对象比较收敛成失败即 panic 的 Rust 测试断言。
- `CompareUnorderedStringSlice` 按多重集而非顺序比较字符串切片，同时保留 Go `nil` slice 与空 slice 的差异。
- `DefaultSessionConnectAttrsJSON`、`DefaultSessionConnectAttrsSlowLogLine`、`RequireContainsDefaultSessionConnectAttrs` 共享并检查慢日志连接属性夹具。
- `RandStringRunes` 生成只包含 ASCII 大小写字母的定长随机字符串。

这些函数服务于测试数据构造和断言，不封装业务状态，也不把失败转换为可恢复的数据库错误。

## 主要符号

- `pub fn DatumEqual(expected: types::Datum, actual: types::Datum)`：使用 `collate::GetBinaryCollator()` 和 `Datum::Compare` 比较值。比较发生错误时以 `expect("compare datums")` panic；比较结果不为零时由 `assert_eq!` panic。参数按值传入。
- `pub fn HandleEqual(expected: &dyn kv::Handle, actual: &dyn kv::Handle)`：先比较 `Handle::IsInt()`，再比较 `Handle::String()`；既检查句柄种类，也检查其可见表示。
- `pub fn CompareUnorderedStringSlice(a: Option<&[String]>, b: Option<&[String]>) -> bool`：`Option` 显式承载 Go slice 的 nil 状态，以 `HashMap<&str, usize>` 计数实现重复项敏感的无序比较。
- `static letterRunes: &[char]`：52 个 ASCII 字母的只读字符表；名称沿用 Go 实现，受 crate 根的 `#![allow(non_upper_case_globals)]` 允许。
- `const defaultSessionConnectAttrsJSON: &str`：包含 `_client_name`、`_os`、`app_name` 的固定 JSON 文本。
- `pub fn DefaultSessionConnectAttrsJSON() -> String`：复制固定 JSON 为拥有所有权的 `String`。
- `pub fn DefaultSessionConnectAttrsSlowLogLine() -> String`：在同一 JSON 前拼接 `# Session_connect_attrs: `。
- `pub fn RequireContainsDefaultSessionConnectAttrs(attrsText: &str)`：逐项检查六个带引号的键和值片段；它验证包含关系，不解析 JSON，也不要求字段顺序或禁止额外字段。
- `pub fn RandStringRunes(n: isize) -> String`：拒绝负长度，从 `letterRunes` 均匀抽取 `n` 次并收集为字符串。

本文件没有类型、trait、`impl` 或条件编译项；所有函数均公开，两个固定数据保持模块私有。

## 执行流程

`DatumEqual` 的流程是取得二进制 collator，以默认的无警告语句上下文调用 `actual.Compare(..., &expected, ...)`，随后依次检查“比较成功”和“结果为零”。`pkg/testkit/testutil/migration_aster_unit_test.rs::datum_and_handle_assertions_follow_go_comparison_rules` 证明字符串比较大小写敏感：`"A"` 与 `"A"` 通过，`"A"` 与 `"a"` 触发 panic。

`CompareUnorderedStringSlice` 先处理状态和长度的快速分支：两个 `None` 相等，只有一侧 `None` 不等，两侧存在但长度不同也不等。之后遍历 `a` 建立每个字符串的出现次数，再遍历 `b` 递减；遇到不存在的键立即返回 `false`，计数归零即删除键，最终以表是否为空决定结果。这个流程同时覆盖顺序无关、重复次数敏感以及 nil/空切片不等三个契约。

慢日志夹具函数均从同一个常量派生：一个返回原 JSON 的拥有副本，一个构造完整慢日志行，包含检查函数则逐一执行 `str::contains`。Go 调用证据包括 `pkg/executor/slow_query_test.go`、`pkg/executor/slow_query_sql_test.go`、`pkg/executor/cluster_table_test.go` 和 `pkg/infoschema/test/clustertablestest/cluster_tables_test.go`；Rust 当前由 `migration_aster_unit_test.rs::shared_fixture_and_random_string_match_go_contract` 直接覆盖。

`RandStringRunes` 先断言 `n >= 0`，创建当前线程使用的随机数生成器，在 `0..n` 的每次迭代中从 52 字符表随机选一个字符，最后收集成 `String`。零长度自然生成空字符串。

## 数据与状态

本文件不维护可变全局状态。`defaultSessionConnectAttrsJSON` 和 `letterRunes` 均为静态只读数据；每次返回 JSON 都会分配新的 `String`，调用者修改返回值不会改变后续结果。

多重集比较的唯一临时状态是函数栈上的 `HashMap<&str, usize>`。键借用输入字符串，只在函数调用期间存活；空间复杂度为不同字符串数量的数量级，时间复杂度在通常哈希表假设下为 `O(a.len() + b.len())`。随机字符串函数持有局部 RNG 与结果缓冲，不缓存历史结果，也不承诺确定性或加密安全性。

## 依赖与调用关系

向下调用关系可由源码符号直接核对：`DatumEqual -> collate::GetBinaryCollator -> types::Datum::Compare`；`HandleEqual -> kv::Handle::{IsInt,String}`；`CompareUnorderedStringSlice -> std::collections::HashMap`；`RequireContainsDefaultSessionConnectAttrs -> str::contains`；`RandStringRunes -> rand::{rng,Rng::random_range}`。`lib.rs` 是必要接线层，负责把 `collate`、`kv`、`types` 名称带入 `include!` 模块并重导出公开 API。

RustCodeGraph 将 `require.rs` 索引为 8 个符号；精确源码节点确认了上述七个函数及其实现。当前仓库的 Rust 直接调用证据位于 `pkg/testkit/testutil/require_test.rs` 和 `pkg/testkit/testutil/migration_aster_unit_test.rs`。其他 Rust 表达式测试中出现的 `testutil.DatumEqual` 多为保留的 Go 对照注释，不能据此声称 Rust 已实际调用。

crate 层面的潜在使用者可由 Cargo 声明看到，包括 `pkg/session/test`、`pkg/executor` 及其若干测试 crate、`pkg/infoschema`、`pkg/ddl`、`pkg/util/keydecoder`；这只证明它们依赖 `astersql-testkit-testutil`，不等于每个 crate 都调用本文件的每个符号。

## 错误处理与边界

这是断言工具，所以失败策略有意采用 panic，而非 `Result`：`DatumEqual` 在 `Compare` 返回错误或非零结果时 panic；`HandleEqual` 在类型标志或字符串表示不等时 panic；连接属性缺项时 panic；`RandStringRunes` 的负长度也 panic。调用者若需要业务级错误传播，不应直接复用这些接口。

`CompareUnorderedStringSlice` 不 panic 来表达普通不等，并对 nil、长度、缺失元素逐级短路。递减不会下溢：只有已存在的键才能进入递减分支，而计数到零时键立即删除，后续重复项会走“不存在”分支。它按 Rust 字符串字节内容精确匹配，不做大小写折叠、排序规则转换或 Unicode 归一化。

`HandleEqual` 只检查 `IsInt` 和 `String`，没有比较句柄的全部内部编码；这是 Go 对照函数的既有契约。`RequireContainsDefaultSessionConnectAttrs` 只检查片段，畸形 JSON 只要包含所有片段也可能通过。`RandStringRunes` 的输出长度以字符数为 `n`，由于字符表全为单字节 ASCII，其字节长度也等于 `n`；极大正数可能带来相应的时间和内存成本。

## 并发与资源生命周期

所有 API 都是无状态的同步函数，没有锁、通道、异步任务、文件、网络连接或事务生命周期。只读静态数据可被并发调用安全共享；`HashMap`、RNG 句柄和结果 `String` 均局限于单次调用并在返回后释放或转移所有权。

`RandStringRunes` 每次调用通过 `rand::rng()` 获取线程局部生成器接口，调用之间不共享本文件自建的可变状态。文档不对跨线程调用顺序、种子或输出可复现性作保证。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/testkit/testutil/require.go`，回归测试为 `pkg/testkit/testutil/require_test.go`。七个 Rust 公共函数逐一保留了 Go 函数名和主要语义：二进制 collator 的 Datum 比较、Handle 类型与字符串比较、无序多重集比较、慢日志夹具以及 ASCII 字母随机串。

重要的语言适配如下：Go 断言函数接收 `testing.TB` 和可选消息，使用 `testify/require` 报告失败；Rust 版本省略测试句柄和自定义消息，使用 `assert!`、`assert_eq!`、`expect` panic。Go 的 `[]string(nil)` 与空 slice 可直接区分，Rust 以 `Option<&[String]>` 表示，`None` 对应 nil。Go `RandStringRunes(int)` 依赖包级 `math/rand`，Rust 使用 `isize` 与 `rand 0.9` 的线程 RNG，并显式拒绝负数。Go JSON getter 返回字符串值，Rust getter 为匹配拥有语义执行一次分配。

`require_test.go::TestCompareUnorderedString` 与 `require_test.rs::test_compare_unordered_string` 覆盖相同的顺序、长度、重复计数和 nil/空切片案例；`migration_aster_unit_test.rs` 额外覆盖二进制排序规则、Handle、夹具内容、随机串字符集和长度。Rust 版本当前并未移植 Go 仓库中所有广泛调用点，不能把 Go 测试调用图等同于 Rust 调用图。

## 扩展指南

新增比较器时，应把实现放在本文件、保持公开函数由 `lib.rs` 现有 `pub use require::*` 导出，并把回归测试放在独立的 `require_test.rs` 或同目录其他独立测试文件，不能把测试内嵌回生产源文件。若需要新 crate 依赖，再更新 `pkg/testkit/testutil/Cargo.toml`；仅使用 `lib.rs` 已重导出的 codec、collate、kv、types 能力时优先复用现有边界。

修改 `CompareUnorderedStringSlice` 时必须保留或明确变更三项兼容契约：`None != Some(&[])`、顺序无关、重复次数敏感。修改 `DatumEqual` 时要同步验证二进制排序规则和比较错误路径；修改 Handle 断言时要注意扩大到内部编码比较可能偏离 Go 契约。扩展连接属性夹具时应同时修改常量、包含检查函数、Rust 迁移测试，并核查上述执行器/infoschema Go 测试的期望文本。随机生成逻辑若改字符集，需同步验证字符数与字节数语义、分布需求及是否仍与 Go 测试用途兼容。

性能风险主要在热循环中反复分配 JSON `String`、对大切片建立计数表，以及生成极长随机串；不过当前定位是测试工具。兼容风险主要来自公开函数签名、panic 行为、nil/空区分以及固定慢日志文本，修改前应搜索所有 Rust 实际调用和 Go 对照调用。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、`pkg/testkit/testutil/require.rs` 包含 8 个符号；`node --file pkg/testkit/testutil/require.rs --offset 1 --limit 260` 返回了完整 134 行实现。调用边命令对精确文件运行时长时间无结果，已中止，故调用者结论改由索引覆盖确认后结合仓库精确符号搜索核验，未虚构图边。
- 源码与接线：`pkg/testkit/testutil/require.rs`、`pkg/testkit/testutil/lib.rs`。
- crate 边界：`pkg/testkit/testutil/Cargo.toml`，以及仓库内依赖 `astersql-testkit-testutil` 的 Cargo manifests。
- Rust 独立测试：`pkg/testkit/testutil/require_test.rs`、`pkg/testkit/testutil/migration_aster_unit_test.rs`。
- Go 对照：`pkg/testkit/testutil/require.go`、`pkg/testkit/testutil/require_test.go`；代表性调用证据来自 `pkg/executor/slow_query_test.go`、`pkg/executor/slow_query_sql_test.go`、`pkg/executor/cluster_table_test.go`、`pkg/infoschema/test/clustertablestest/cluster_tables_test.go`、`pkg/ddl/primary_key_handle_test.go` 和 `pkg/privilege/privileges/privileges_test.go`。
- 人工复核结论：本文件存在是为了让 Rust 测试共享与 Go 一致的领域断言和夹具；执行均为同步、局部、失败即 panic 的测试流程；安全扩展点与必须同步的独立测试已在“扩展指南”列出。按任务约束未运行 Cargo。
