# `pkg/util/sqlexec/mock/mock.rs`

## 文件定位

本文件属于独立 crate `astersql-util-sqlexec-mock`，crate 根在
`pkg/util/sqlexec/mock/lib.rs`。`lib.rs` 以私有模块 `mock` 装载本文件，再通过
`pub use mock::*` 将其中的公开项导出；工作区门面随后在 `pkg/lib.rs` 的
`util::sqlexec::mock` 下再次导出该 crate。

它位于受限 SQL 执行接口与测试替身之间：父 crate
`pkg/util/sqlexec/restricted_sql_executor.rs` 定义 `RestrictedSQLExecutor`，相邻的
`restricted_sql_executor_mock.rs` 实现该 trait 的严格 mock，而本文件只提供识别该
mock 的上下文键 `RestrictedSQLExecutorKey`。它不解析或执行 SQL，也不保存期望队列。

## 核心职责

- 定义零大小、可复制的公开键类型 `RestrictedSQLExecutorKey`。
- 用 `RestrictedSQLExecutorKey::String` 返回固定身份字符串
  `"__MockRestrictedSQLExecutor"`，与 `pkg/util/sqlexec/mock/mock.go` 完全一致。
- 实现 `std::fmt::Display`，使 Rust 的 `to_string()`、格式化输出与 Go
  `fmt.Stringer` 风格的 `String()` 得到相同身份。

该字符串是兼容约定而不是展示文案；修改它会破坏 Rust/Go 对同一逻辑键身份的
约定。当前 Rust 源码尚未把此键接入一个动态 session context 映射，不能据此声称
Rust 生产路径已经支持替换受限 SQL 执行器。

## 主要符号

- `pub struct RestrictedSQLExecutorKey;`：无字段的 unit struct。派生
  `Clone`、`Copy`、`Debug`、`Default`、`Eq`、`Hash`、`PartialEq`，因而可以按值
  复制、比较和作为哈希键使用；构造无需分配，也没有实例状态。
- `pub fn RestrictedSQLExecutorKey::String(&self) -> &'static str`：返回静态字符串
  切片，既无所有权转移也无堆分配。名称保留 Go 风格大写，crate 根通过
  `#![allow(non_snake_case)]` 接受这种移植命名。
- `impl fmt::Display for RestrictedSQLExecutorKey`：`fmt` 将 `String()` 的结果写入
  调用方提供的 `fmt::Formatter`，原样转发 `fmt::Result`。

文件没有常量、枚举、trait 定义、条件编译分支或私有辅助函数。公开面只有键类型、
固有方法 `String`，以及经 trait 获得的格式化能力。

## 执行流程

1. 调用方以 `RestrictedSQLExecutorKey` 或 `RestrictedSQLExecutorKey::default()` 构造
   零大小键；两者没有初始化逻辑。
2. 直接调用 `key.String()` 时，方法返回编译进二进制的静态文本
   `"__MockRestrictedSQLExecutor"`。
3. 使用 `format!`、`to_string()` 或 `{key}` 格式化时，标准库调用
   `Display::fmt`；该实现取得相同静态文本，并交给 `Formatter::write_str`。

相邻 `MockRestrictedSQLExecutor` 的 `EXPECT()`、FIFO 期望消费和 `verify()` 流程均在
`restricted_sql_executor_mock.rs`，不经过本文件。Go 生产逻辑的完整接线是：测试把
`MockRestrictedSQLExecutor` 以此键写入 session context，
`pkg/statistics/handle/util/util.go::ExecRowsWithCtx` 在测试模式下按同一键读取并调用
`ExecRestrictedSQL`；这条上下文读写链目前没有对应的 Rust 调用边。

## 数据与状态

`RestrictedSQLExecutorKey` 是零大小类型，不持有指针、执行器、SQL 文本或可变状态。
所有实例都相等，哈希语义也仅来自类型和值的派生实现。`String()` 返回的
`&'static str` 生命周期覆盖整个程序，不依赖键实例。

真正的 mock 状态位于相邻文件的 `MockRestrictedSQLExecutor`：其 `State` 保存调用
记录和期望队列。该状态与本键没有字段级引用；二者只通过“上下文槽位用于存放该
mock”这一 Go 侧协议产生概念关联。

## 依赖与调用关系

本文件唯一直接依赖是 Rust 标准库 `std::fmt`。`Display::fmt` 的唯一被调操作是
`RestrictedSQLExecutorKey::String` 与 `Formatter::write_str`，没有 I/O、SQL 或网络
依赖。

crate 边界由 `pkg/util/sqlexec/mock/Cargo.toml` 确认：该 crate 直接依赖父 crate
`astersql-util-sqlexec`，但这项依赖服务于 crate 根再导出及相邻 mock 实现，本文件
自身不引用它。crate 无 feature 声明，设置了 `autotests = false`、`doctest = false`，
测试由 `lib.rs` 的 `#[cfg(test)]` 模块显式挂载。

RustCodeGraph 将本文件列为被 `pkg/util/sqlexec/restricted_sql_executor.rs` 使用，但对
`RestrictedSQLExecutorKey` 的精确 callers/callees/impact 查询没有返回调用边。源码
检索进一步确认：Rust 侧唯一直接使用点是
`pkg/util/sqlexec/mock/migration_aster_unit_test.rs`；统计模块 Cargo 清单中的相关依赖
位于 `cfg(any())` 区段，当前恒为禁用。Go 侧直接调用者是统计模块的测试与
`ExecRowsWithCtx`，详见下文。

## 错误处理与边界

`String()` 不返回 `Result`，且固定返回静态文本，没有失败分支。
`Display::fmt` 不吞掉格式化错误，而是原样返回 `Formatter::write_str` 的
`fmt::Result`。文件不触发 panic，也不处理相邻 mock 的“缺少期望”或“期望未消费”
错误。

边界约束包括：

- 固定字符串的大小写、前导双下划线和拼写都属于兼容接口。
- unit struct 的值语义保证任意构造出的键相等；若未来加入字段，会改变比较、哈希
  和构造方式。
- `Display` 必须继续与 `String()` 一致，否则显式调用与通用格式化会产生两个身份。
- 该类型本身不能证明上下文存取的类型安全；Go 侧
  `v.(*mock.MockRestrictedSQLExecutor)` 仍依赖存入值类型正确，错误类型会 panic。

## 并发与资源生命周期

本类型没有资源所有权、锁、任务、通道或析构逻辑。静态字符串无需释放；键可
`Copy`，复制不会产生共享可变状态。派生 trait 的组合使键值本身可安全地跨线程
传递或共享，但本文件没有显式声明或管理并发协议。

相邻 mock 的并发性来自 `Arc<Mutex<State>>`，与本键的生命周期独立。若未来把键接入
Rust context 容器，需要由容器定义值的所有权、并发和清理规则，不能把相邻 mock 的
锁语义误归到 `RestrictedSQLExecutorKey`。

## 与 Go 版本的对应关系

Go 对照文件 `pkg/util/sqlexec/mock/mock.go` 同样只定义空结构体
`RestrictedSQLExecutorKey`，并以值接收者的 `String() string` 返回完全相同的固定
文本。Rust 版本的 unit struct 对应 Go 的空结构体；`&self` 不改变其值语义，而
`&'static str` 避免了为固定文本创建拥有所有权的 `String`。

Rust 额外派生了比较、哈希、复制、调试和默认构造 trait，并额外实现 `Display`，以
补足 Rust 通用格式化能力。迁移测试
`pkg/util/sqlexec/mock/migration_aster_unit_test.rs::restricted_sql_executor_key_has_the_go_context_identity`
同时断言 `String()` 与 `to_string()` 的结果。

Go 的真实使用证据为：

- `pkg/statistics/handle/autoanalyze/autoanalyze_test.go::WrapAsSCtx` 和
  `pkg/statistics/handle/lockstats/unlock_stats_test.go::wrapAsSCtx` 用此键向 session
  context 写入 mock。
- `pkg/statistics/handle/util/util.go::ExecRowsWithCtx` 仅在 `intest.InTest` 时读取该键，
  命中后调用 mock 的 `ExecRestrictedSQL`，否则调用 session 的真实受限执行器。

Rust 当前不存在上述 session context 读写实现的源码调用点，因此这里只完成了键
身份及格式化语义的移植，生产接线状态与 Go 不等价。

## 扩展指南

若只是新增 mock 执行器能力，应修改
`restricted_sql_executor_mock.rs` 及其独立测试，而不是扩充本键。若确需改变键：

1. 保持 `RestrictedSQLExecutorKey::String`、`Display::fmt` 与 Go
   `mock.go::RestrictedSQLExecutorKey.String` 三者一致。
2. 扩展 `pkg/util/sqlexec/mock/migration_aster_unit_test.rs` 中的独立键测试；不要把测试
   嵌入 `mock.rs`。
3. 若新增 Rust context 接线，应在拥有 session/context 行为的 crate 中实现存取，
   并新增独立回归测试覆盖“命中 mock”和“回退真实执行器”两条分支；不要仅依赖本
   文件的字符串测试证明接线完成。
4. 若增加字段或去掉 `Copy`/`Hash` 等派生，先审查所有上下文容器对键构造、相等性、
   哈希与线程边界的要求。

兼容风险集中在跨语言键名和上下文查找失败；性能风险很低，因为当前路径只有零大小
值和静态字符串。未来上下文接线的性能取决于容器查找与 mock 存储方式，不由本文件
决定。

## 验证依据

- RustCodeGraph：`status` 确认索引含 7,032 个 Rust 文件；`files --filter
  pkg/util/sqlexec/mock` 确认本 crate 的 Rust/Go 文件集合；`node --file
  pkg/util/sqlexec/mock/mock.rs --offset 1 --limit 240` 读取全部 39 行；`query
  RestrictedSQLExecutorKey --json` 定位 Rust/Go 两个定义；对 Rust 限定符执行
  `callers`、`callees`、`impact --depth 2` 均无符号边输出。
- Rust 源码：`pkg/util/sqlexec/mock/mock.rs`（目标实现）、
  `pkg/util/sqlexec/mock/lib.rs`（模块装配、再导出和显式测试模块）、
  `pkg/util/sqlexec/mock/restricted_sql_executor_mock.rs`（真实 mock 状态与 trait 实现）、
  `pkg/util/sqlexec/restricted_sql_executor.rs`（被替换接口）。
- 清单与门面：`pkg/util/sqlexec/mock/Cargo.toml`、`pkg/util/sqlexec/Cargo.toml`、根
  `Cargo.toml`、`pkg/lib.rs`；它们确认 crate 依赖、工作区注册与公开导出路径。
- Rust 独立测试：`pkg/util/sqlexec/mock/migration_aster_unit_test.rs`，其中
  `restricted_sql_executor_key_has_the_go_context_identity` 覆盖固定身份和
  `Display`；同文件其他测试属于相邻执行器 mock，不是本键的内部行为。
- Go 对照与调用者：`pkg/util/sqlexec/mock/mock.go`、
  `pkg/statistics/handle/util/util.go`、
  `pkg/statistics/handle/autoanalyze/autoanalyze_test.go`、
  `pkg/statistics/handle/lockstats/unlock_stats_test.go`。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以固定十一个二级标题的结构命令验证
  文档形态，并人工复核所有“当前已支持”陈述均有上述源码或索引证据。
