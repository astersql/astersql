# `pkg/util/urls.rs`

## 文件定位

[`urls.rs`](urls.rs) 属于 `astersql-util` crate；[`lib.rs`](lib.rs) 通过 `pub mod urls` 将它公开为 `astersql_util::urls`。文件只提供服务端点列表的解析门面，具体的单个 URL 校验和格式化由同一 crate 的 [`service_url.rs`](service_url.rs) 完成。

当前 Rust 仓库中没有检索到生产代码调用 `ParseHostPortAddr`。RustCodeGraph 的 `node ParseHostPortAddr` 反向边只指向 [`urls_test.rs`](urls_test.rs)，文本检索还找到独立测试目标 [`security_2_aster_unit_test.rs`](security_2_aster_unit_test.rs)；因此它是已公开、已有测试的工具 API，但不能据此声称已经接入某条 Rust 生产请求主链。

## 核心职责

- 将一个逗号分隔的字符串拆成有序的服务端点列表；每项在解析前去除首尾空白。
- 对每项调用 `parse_service_url(entry, "http")`，使没有显式 scheme 的 `host:port` 按 HTTP 规则校验。
- 通过 `ServiceURL::Endpoint(entry.contains("://"))` 决定结果是否保留 scheme：显式 URL 保留 scheme，裸 `host:port` 保持裸形式；`unix`/`unixs` 即使调用方不要求也由 `Endpoint` 强制保留 scheme。
- 使用 `Result<Vec<String>>` 表达整批成功或失败；任意一项解析失败都会使整个列表失败，不返回部分结果。

这些职责均直接体现在唯一公开函数 `ParseHostPortAddr`（[`urls.rs`](urls.rs) 第 21 行）及其两个下游符号 `parse_service_url`、`ServiceURL::Endpoint` 中。

## 主要符号

- `pub fn ParseHostPortAddr(input: &str) -> anyhow::Result<Vec<String>>`：本文件唯一的模块级符号和公开 API。输入借用字符串，成功时分配并返回与逗号分段一一对应的 `Vec<String>`。
- `parse_service_url(raw: &str, default_scheme: &str) -> Result<ServiceURL>`：定义在 [`service_url.rs`](service_url.rs)，是 crate 内部函数。本文件固定传入默认 scheme `"http"`；它负责空输入、支持的 scheme、`host:port` 形态、空 host/port 以及 URL path 等校验。
- `ServiceURL::Endpoint(&self, with_scheme: bool) -> String`：定义在 [`service_url.rs`](service_url.rs)。普通 HTTP(S) 地址按 `with_scheme` 决定输出形式，Unix family 总是输出可拨号的完整 URL。

函数名沿用 Go 导出符号的 PascalCase；[`lib.rs`](lib.rs) 在 crate 级允许 `non_snake_case`，所以这不是条件编译分支。本文件没有常量、类型、trait、`impl` 或条件编译项。

## 执行流程

1. `input.split(',')` 按字面逗号切分并保持原有次序。连续逗号、首尾逗号或空字符串都会产生空分段，而不是被忽略。
2. 闭包对每个分段执行 `trim()`，仅去除分段两端空白。
3. `parse_service_url(entry, "http")` 校验并构造 `ServiceURL`。裸地址必须符合 `host:port` 外形；显式地址只允许 `http`、`https`、`unix`、`unixs`，更细的规则见 [`service_url.rs`](service_url.rs)。
4. `entry.contains("://")` 记录原输入是否显式给出 scheme，再传给 `Endpoint`。因此裸 `127.0.0.1:2379` 输出仍是裸地址，而 `https://127.0.0.1:2379` 保留 `https://`。
5. iterator 的 `collect()` 利用 `Result` 的收集语义：全部元素成功才产生 `Ok(Vec<String>)`；遇到第一个 `Err` 立即停止并原样向上传播。

相关测试还确认 HTTP(S) URL 的 query/fragment 不进入 endpoint，例如 `http://host:2379?x=1` 输出 `http://host:2379`；这是下游 `parse_service_url` 只保存 authority 中 `host:port` 的结果，不是本文件自行剥离参数。

## 数据与状态

本文件不保存全局或跨调用状态。输入是只读 `&str`；处理中产生临时的切分借用、修剪后的 `&str`、单项 `ServiceURL`，最终为每个端点分配一个拥有所有权的 `String` 并汇集到 `Vec<String>`。

输出顺序与输入分段顺序一致，也不会去重。列表长度在成功时等于 `split(',')` 产生的分段数；由于空分段会被下游拒绝，成功结果实际上不会包含由空项生成的字符串。函数没有缓存、环境读取或配置依赖。

## 依赖与调用关系

- crate 边界：[`Cargo.toml`](Cargo.toml) 将该目录定义为 `astersql-util`，库入口为 `lib.rs`，并声明直接依赖 `anyhow = "1"`；本文件使用其 `Result` 别名。
- 模块内下游：`ParseHostPortAddr -> parse_service_url` 和 `ParseHostPortAddr -> ServiceURL::Endpoint`。RustCodeGraph 的 `callees ParseHostPortAddr` 明确给出这两条 Rust 边。
- 下游外部依赖：`parse_service_url` 位于 [`service_url.rs`](service_url.rs)，其 HTTP(S) 解析使用 Cargo 中的 `url = "2"`；这属于间接行为依赖，不是本文件直接导入。
- 已确认上游：RustCodeGraph 的节点反向边包含 `urls_test.rs -> ParseHostPortAddr`；仓库文本检索还确认 [`security_2_aster_unit_test.rs`](security_2_aster_unit_test.rs) 调用它。没有找到 Rust 生产调用者。
- 模块导出：[`lib.rs`](lib.rs) 的 `pub mod urls` 使外部 crate 可以通过模块路径调用该函数；同一文件在 `cfg(test)` 下把 [`urls_test.rs`](urls_test.rs) 挂为独立测试模块。

## 错误处理与边界

本文件不包装错误，而是用 `?` 等价的 `Result` 收集语义传播 `parse_service_url` 返回的首个 `anyhow::Error`。失败时没有部分 `Vec` 暴露给调用方。

由 [`urls_test.rs`](urls_test.rs)、[`security_2_aster_unit_test.rs`](security_2_aster_unit_test.rs) 和下游实现共同确认的边界包括：

- 缺少端口的裸主机、空字符串/空分段、空 Unix 地址、未知 scheme、畸形 IPv6/括号结构会失败。
- HTTP(S) 显式 URL 的 host 或 port 为空会失败，带 path 会失败；query 和 fragment 可以被解析但不会进入 endpoint 输出。
- 端口只做语法形态校验，不要求是数字或处于 TCP 数值范围；测试明确接受服务名 `mysql` 和 `65536`。
- Go `net.SplitHostPort` 允许 `:2379`、`localhost:` 和 `:` 通过形态解析，Rust 独立测试也把这些裸地址列为有效；而显式 HTTP(S) URL 另有非空 host/port 检查。扩展校验时不能把这两类输入混为一谈。
- Unix family 允许文件系统式地址并始终保留 scheme；`unix://` 因地址为空失败。

## 并发与资源生命周期

函数是同步、无锁、无异步任务且无共享可变状态的纯解析过程。每次调用的临时值都局限在 iterator/闭包和返回值生命周期内；输入只在调用期间借用，返回值不借用输入，因此可独立跨线程移动。

资源成本随输入长度和端点数量线性增长：每个成功端点至少构造一个拥有所有权的输出字符串，显式 URL 的解析还委托给 `url` crate。没有文件、socket、事务、channel 或需要显式释放的外部资源。

## 与 Go 版本的对应关系

直接对照文件是 [`urls.go`](urls.go)，对应测试是 [`urls_test.go`](urls_test.go)。两版主流程一致：按逗号拆分、逐项 `TrimSpace`/`trim`、以 HTTP 为默认 scheme 调用 service URL 解析器、依据原项是否含 `://` 选择输出形式，并在首个错误处返回整批失败。

Rust 使用 iterator 的 `map(...).collect::<Result<Vec<_>>>()` 表达 Go 的显式循环、`append` 与错误早退；这是控制流写法差异，不改变列表顺序或全有全无语义。Rust 的 [`urls_test.rs`](urls_test.rs) 覆盖了 Go 表格中的裸地址、多地址、HTTP(S) 和 Unix URL，并增加了 query、空 host/port、服务名/越界数值端口和畸形括号等边界。

实际解析一致性还依赖 [`service_url.rs`](service_url.rs) 对 [`service_url.go`](service_url.go) 的移植。特别是 Rust 使用 WHATWG `url` crate，但为 Go 接受服务名端口的行为处理了 `InvalidPort`；若修改本文件的默认 scheme 或输出策略，应同时复核两侧 service URL 实现，不能只比较这两个薄门面。

## 扩展指南

- 若只调整列表语义（分隔符、空项策略、顺序或重复项处理），修改 `ParseHostPortAddr`，并在独立的 [`urls_test.rs`](urls_test.rs) 增加回归；不要把测试嵌入生产源文件。
- 若调整允许的 scheme、host/port/path/query 规则或 endpoint 格式，应修改 [`service_url.rs`](service_url.rs) 的 `parse_service_url`/`Endpoint` 及其独立测试，并同步核对 Go 的 [`service_url.go`](service_url.go) 和 [`urls.go`](urls.go)。
- 保持“裸 HTTP 地址不被强制加 scheme”和“Unix family 必须保留 scheme”这两个兼容契约，除非调用方迁移方案明确允许破坏性变化。
- 新增生产调用者前，应决定调用方需要整批原子失败还是容忍部分有效地址；当前 API 只支持前者。若改变为部分成功，需要新的返回契约而不是静默丢弃错误。
- 性能扩展应保留单次线性扫描和有序输出；对大列表做额外规范化或去重会增加分配并改变兼容语义，应先增加针对顺序、重复项和失败位置的测试。
- 当前没有 Rust 生产调用边，接线时应在实际所属子系统增加集成层验证，不能仅依赖 util 单元测试证明应用级可达性。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`query ParseHostPortAddr` 找到 Rust/Go 两个定义和 Rust 测试导入。
- RustCodeGraph `node --file pkg/util/urls.rs --offset 1 --limit 240` 与 `node ParseHostPortAddr`：核对完整 29 行源文件、唯一公开函数、测试反向边，以及 `parse_service_url`/`Endpoint` 下游边。
- RustCodeGraph `callees ParseHostPortAddr --limit 50`：确认 Rust 实现的两个直接被调用符号。独立 `callers` 查询未在 60 秒内返回，已中止；调用方结论由节点反向边与全仓 Rust 文本检索交叉验证。
- 已阅读生产/边界文件：[`urls.rs`](urls.rs)、[`service_url.rs`](service_url.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)。`pkg/util` 下没有 `doc.go`，因此以 crate 入口 `lib.rs` 作为最近模块契约。
- 已阅读 Go 对照：[`urls.go`](urls.go)、[`service_url.go`](service_url.go)；已阅读独立测试：[`urls_test.rs`](urls_test.rs)、[`security_2_aster_unit_test.rs`](security_2_aster_unit_test.rs)、[`urls_test.go`](urls_test.go)。
- 仓库文本检索 `rg -n 'ParseHostPortAddr' --glob '*.rs' --glob '!pkg/util/urls.rs'` 仅命中上述两个 Rust 测试文件，支持“当前无 Rust 生产调用者”的限定结论。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的命令验证文档存在且恰有 11 个固定二级章节，并人工复核没有把未观察到的生产接线描述为已支持。
