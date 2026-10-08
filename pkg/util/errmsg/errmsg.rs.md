# `pkg/util/errmsg/errmsg.rs`

## 文件定位

[`errmsg.rs`](errmsg.rs) 是 `astersql-util-errmsg` crate 的核心实现文件，负责在 SQL 错误发送给客户端之前，按全局配置中的正则规则为 `mysql::SQLError.Message` 追加说明后缀。crate 边界由 [`Cargo.toml`](Cargo.toml) 和 [`lib.rs`](lib.rs) 定义：`lib.rs` 将本文件作为私有 `errmsg` 模块加载，再对外重导出 `Extend`。顶层 [`pkg/lib.rs`](../../lib.rs) 又通过 `util::errmsg` 门面重导出该 crate。

这是一个窄而专一的错误呈现层：它不创建 SQL 错误、不改变错误码或 SQLSTATE，也不编译正则。正则的编译、排序和发布由 [`pkg/config/config.rs`](../../config/config.rs) 负责。

## 核心职责

- `Extend` 对空错误指针和空规则快照做快速返回。
- 按配置层已确定的优先级顺序扫描规则，跳过空后缀，只应用第一条匹配规则。
- `extendErrorMessage` 统一标点：移除原消息和后缀末尾的所有 `.`，再组合成 `"{message}, {suffix}."`。
- 保持 `SQLError.Code` 和 `SQLError.State` 不变，只就地替换 `Message`。

## 主要符号

- `pub fn Extend(m: Option<&mut mysql::SQLError>)`：唯一公开 API。`Option<&mut ...>` 对应 Go 的可空 `*mysql.SQLError`；可变借用保证调用期间对该错误值的独占修改权。
- `fn extendErrorMessage(m: &mut mysql::SQLError, msg: &str)`：私有格式化辅助函数。它使用 `trim_end_matches('.')` 清理两侧末尾句点，然后分配新 `String` 写回 `Message`。

本文件没有模块级常量、自定义类型、trait、`impl` 或条件编译项。RustCodeGraph 将目标文件索引为 3 个节点（文件及两个函数），与源码结构一致。

## 执行流程

1. 调用者将待呈现的 `SQLError` 以 `Some(&mut error)` 传入 `Extend`；`None` 立即返回。
2. `Extend` 调用 `config::get_error_message_extensions()` 取得已编译规则的拷贝快照；空集合立即返回。
3. 依次检查快照中的规则。空 `suffix` 被跳过；其余规则通过 `ErrorMessageExtension::matches(&m.Message)` 匹配当前原始消息。
4. 首次匹配时调用 `extendErrorMessage`，将规范化后的原消息、逗号空格、规范化后的后缀和一个终止句点组合起来。
5. 写回后立即返回，所以每次最多追加一个后缀；没有规则匹配时错误完全不变。

优先级不是由本文件的循环计算的。`prepare_error_message_extensions` 会先按 `pattern` 字节长度降序排列，再以 `pattern` 和 `suffix` 做稳定的确定性比较；因而“首个匹配”通常表示“更长、更具体的模式”。

## 数据与状态

本文件自身不持有全局状态。输入状态有两部分：可变 `SQLError` 和配置 crate 返回的 `Vec<ErrorMessageExtension>` 快照。输出是同一个 `SQLError` 中可能更换过的 `Message: String`。

`get_error_message_extensions` 从 `PREPARED_EXTENSIONS: RwLock<Arc<Vec<_>>>` 中取值并克隆出独立 `Vec`。因此一次 `Extend` 遍历的是一致快照：同期 `store_global_config` 可以发布新配置，但不会改写已取得的本地规则集合。代价是每次调用会克隆规则向量，匹配成本为按优先级线性扫描，命中后提前终止。

## 依赖与调用关系

直接下游依赖是 `crate::config` 中的 `get_error_message_extensions` 和 `ErrorMessageExtension::matches`，以及 `crate::parser::mysql::SQLError`。它们由 [`lib.rs`](lib.rs) 分别桥接至 `astersql-config` 和 `astersql-parser-mysql`。[`Cargo.toml`](Cargo.toml) 还列出 `astersql-errors`、`backtrace`、`semver`、`serde`、`serde_json` 和 `unicode-general-category`，但本文件不直接使用这些依赖。

RustCodeGraph 文件查询确认本 crate 的源文件边界；精确全库搜索只找到 [`errmsg_test.rs`](errmsg_test.rs) 和 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 调用 Rust `Extend`。`pkg/server/Cargo.toml` 虽声明了 `astersql-util-errmsg` 依赖，但 Rust server 源码尚无调用边；因此当前不能声称该 Rust 实现已进入 Rust 服务端错误写包主链。

Go 生产链的真实上游是 [`pkg/server/conn.go`](../../server/conn.go) 的 `clientConn.writeError`：它将内部错误转换为 `*mysql.SQLError`，调用 `errmsg.Extend(m)`，随后把 `Code`、`State` 和已扩展的 `Message` 编码到 MySQL 错误包。这条 Go 调用边给出了 Rust 未来接线时应保持的时序。

## 错误处理与边界

- `Extend(None)` 是明确的无操作，对应 Go nil 防御。
- 空规则快照、未匹配消息和空后缀都不改写消息。
- 无效或空白正则不在本文件报错。配置校验模式下，`prepare_error_message_extensions(..., false)` 返回错误；全局配置发布模式下，`(..., true)` 跳过无效规则，所以 `Extend` 只看到可匹配的已编译规则。
- 尾部清理只处理 ASCII 句点 `.`，不去除空白、逗号或其他 Unicode 标点。空原消息或只由句点组成的消息没有专门分支，会按同一格式化规则处理。
- 本函数不返回 `Result`。配置锁中毒会在配置访问函数内因 `expect` panic；正则匹配和字符串组合的正常路径没有可恢复错误通道。
- 重复对同一条错误调用 `Extend` 不保证幂等；若扩展后的消息仍匹配某条规则，后缀可能再次追加。现有主链意图是每个待发送错误只调用一次。

## 并发与资源生命周期

`Extend` 不启动任务、线程或通道，不持有 I/O 资源，也不持锁跨越正则匹配。配置读锁的生命周期限于 `get_error_message_extensions` 内部克隆快照的短临界区；返回后，循环拥有独立的 `Vec`。对 `SQLError` 的 `&mut` 借用覆盖整次调用，阻止同一错误值被安全 Rust 代码并发改写。

[`errmsg_test.rs`](errmsg_test.rs) 和 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 都使用配置写线程、扩展线程和原子配置字段写线程做 1000 次并发循环，验证快照发布期间扩展结果保持正确。测试中的 `ERRMSG_CONFIG_TEST_LOCK` 和 `ConfigScope` 只用来串行化会改写进程级配置的测试，不是生产路径的一部分。

## 与 Go 版本的对应关系

Rust [`errmsg.rs`](errmsg.rs) 逐结构对应 Go [`errmsg.go`](errmsg.go)：`Extend` 的 nil/空配置快速返回、空后缀跳过、正则匹配、首次命中返回，以及 `extendErrorMessage` 的句点规范化格式均保持一致。Rust 的 `Option<&mut SQLError>` 对应 Go 指针的 nil 可能性；`trim_end_matches('.')` 对应 Go `strings.TrimRight(..., ".")`。

规则准备也对齐：Rust `pkg/config/config.rs::prepare_error_message_extensions` 和 Go `pkg/config/config.go::prepareErrorMessageExtensions` 都跳过发布时的无效正则，并按模式长度、模式文本、后缀文本排序。两个独立 Rust 测试文件覆盖了 Go 表驱动用例、nil 防御、无效正则、更长模式优先和并发发布。

关键迁移差距是接线状态：Go `pkg/server/conn.go::clientConn.writeError` 已在编码 MySQL 错误包前调用 `errmsg.Extend`；当前 Rust 全库搜索未找到对 Rust `Extend` 的生产调用。因而实现与单元语义已迁移，Rust 服务主链集成则尚未由现有代码证据证实。

## 扩展指南

- 修改匹配或格式化语义时，主要接入点是 `Extend` 和 `extendErrorMessage`；必须同步更新独立的 [`errmsg_test.rs`](errmsg_test.rs) 与 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)，不应把测试内嵌到生产文件。
- 修改规则编译、优先级或快照发布时，真实实现位于 `pkg/config/config.rs` 的 `ErrorMessageExtension`、`prepare_error_message_extensions`、`get_error_message_extensions` 和 `store_global_config`；还要同步 `pkg/config/config_test.rs`。
- 将功能接入 Rust 服务端时，应以 Go `clientConn.writeError` 为时序基准：在错误已转换为 `SQLError`、但尚未计算包长和写入协议包之前调用一次。应新增 server 层独立回归测试，证明客户端实际收到扩展消息。
- 兼容风险集中在用户可见消息的精确文本、规则优先级和单次追加约定；性能风险集中在每个错误克隆全部规则及线性匹配。如果优化为借用 `Arc` 或其他结构，必须保留单次调用的快照一致性。
- 为保持 Go/Rust 移植对齐，任何行为变更都应比对 [`errmsg.go`](errmsg.go) 和 [`errmsg_test.go`](errmsg_test.go)；若是 Rust 有意分歧，需明确记录理由和兼容影响。

## 验证依据

- RustCodeGraph：`status` 显示索引覆盖 7032 个 Rust 文件；`files --filter pkg/util/errmsg` 列出 `errmsg.rs`、`lib.rs`、两个 Rust 测试及 Go 对照文件，并标识 `errmsg.rs` 有 3 个节点。`query extendErrorMessage --kind function --json` 同时找到 Go/Rust 私有辅助函数。当次 `explore/node/callers/callees` 未返回可用调用边，因此调用关系又用下述精确源码搜索补齐，未把空图结果当作生产已接线的证据。
- 实现与 crate 边界：[`errmsg.rs`](errmsg.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)、[`pkg/lib.rs`](../../lib.rs)。
- 配置和状态语义：[`pkg/config/config.rs`](../../config/config.rs) 的 `ErrorMessageExtension::matches`、`prepare_error_message_extensions`、`PREPARED_EXTENSIONS`、`get_error_message_extensions` 和 `store_global_config`；[`pkg/config/config_test.rs`](../../config/config_test.rs) 验证规则拷贝与无效配置。
- Go 对照与生产入口：[`errmsg.go`](errmsg.go)、[`errmsg_test.go`](errmsg_test.go)、[`pkg/config/config.go`](../../config/config.go) 和 [`pkg/server/conn.go`](../../server/conn.go) 的 `clientConn.writeError`。
- Rust 独立测试：[`errmsg_test.rs`](errmsg_test.rs) 覆盖表驱动匹配、空配置、无效正则、最长模式优先和并发发布；[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 额外覆盖 `Extend(None)` 并作为 Go 迁移回归。
- 调用面搜索：`rg -n --glob '*.rs' '\bExtend\s*\(' pkg` 只命中本实现和两个 Rust 测试；`rg -n --glob '*.go' 'errmsg\.Extend\s*\(' pkg cmd tests` 命中 `pkg/server/conn.go`。
- 按总计划要求，本任务是纯文档分析，未运行 Cargo。结构验证应确认本文档存在且恰有任务指定的 11 个二级标题。
