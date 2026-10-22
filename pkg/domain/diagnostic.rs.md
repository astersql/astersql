# `pkg/domain/diagnostic.rs`

## 文件定位

本文件由 `pkg/domain/lib.rs` 的 public diagnostic 模块加载，属于 `astersql-domain`（`pkg/domain/Cargo.toml`），为既有 Domain 添加诊断启动和统计加载方法。它不创建独立 Domain 类型。

## 核心职责

启动查询侧 schema reader，复用已有统计缓存初始化；不从正常 Domain::start 继承 DDL、server/topology 或其他维护 worker。

## 主要符号

`Domain::start_diagnostic(&self) -> Result<(), DomainError>` 调用 `begin_start` 和 `start_schema_reload_worker(true)`；`load_stats_diagnostic` 仅调用 `initialize_stats`。两个方法没有新增字段。

## 执行流程

调用方先构造并 init Domain，再选择 start_diagnostic。begin_start 检查初始化/关闭/重复启动，成功后强制启动 schema-reload。统计加载单独由调用方调用 load_stats_diagnostic，同步返回 initialize_stats 结果；这不是自动伴随启动执行的步骤。

## 数据与状态

复用 Domain 的 initialized、closed、started 原子状态、InfoCache、schema loader 和 worker 列表。reader 至少间隔 10ms，加载成功且版本严格更新时插入 InfoSchema；load_stats_diagnostic 复用既有统计 handle 和 init_stats_done。

## 依赖与调用关系

直接依赖 crate 内 Domain/DomainError；具体调度和统计加载位于 `pkg/domain/domain.rs`。`pkg/domain/diagnostic_test.rs::start_diagnostic_bypasses_ddl_and_starts_only_metadata_readers` 安装记录 DDL，验证 starts=0、schema version=7、worker 列表只有 schema-reload，并 close。冻结入口 main.rs 未调用此方法，不能声称诊断二进制启动已完整接线。

## 错误处理与边界

begin_start 返回 NotInitialized、Closed 或 AlreadyStarted。schema reader 的加载失败不替换旧缓存；线程创建等内部边界沿用 Domain。initialize_stats 捕获统计初始化 panic 为 Stats 错误，并确保 init_stats_done 置位。此文件自身不检查进程诊断开关，也不禁止直接写 API 或 SQL。

## 并发与资源生命周期

后台 reader 由 Domain worker 集合持有；Domain::close 幂等置 closed，先广播 stop 再 join worker，然后停止 DDL 并清理其余协作者。诊断方法不取得新的资源所有权；统计加载同步执行，不创建 Go 端的整组统计 reader worker。

## 与 Go 版本的对应关系

对照 `pkg/domain/diagnostic.go`：Go StartDiagnostic 在满足条件时维护独立 server-ID lease，并启动 schema sync 和 top-N slow query；Rust 此文件只启动 schema-reload，没有对应租约或 slow-query worker。Go LoadStatsDiagnostic 创建 handle 并启动 stats/histogram readers，Rust 仅 initialize_stats 一次；不能描述成完全等价或由此证明全局 kill 的 server ID 协调。

## 扩展指南

保持与正常 start 共享状态检查；如增加 reader，必须证明其不引入写入、owner、GC 或自动分析。扩展 server-ID 或周期统计属额外行为，应单独实现和验证，不能从 Go 方法名推定已支持。测试保持独立文件，并检查 worker 集合及关闭收敛。

## 验证依据

冻结范围为 `ad0a6f48804e48373cc79c5f53f0072daf99ceb9..2c944fbcbfb2c904cc8cbddafe17954138bd8867`。经 `git diff ad0a6f4..2c944fb -- pkg/domain/diagnostic.rs`、`git show 2c944fb:pkg/domain/diagnostic.rs` 核对，目标 blob 为 `7b248f5c77588dd9292e39e8e7d981a760bb1a13`。模块入口及 Cargo 声明均以同一提交读取。独立测试 `pkg/domain/diagnostic_test.rs` 和 Go 对照 `pkg/domain/diagnostic.go` 为静态证据，未运行测试。

RustCodeGraph 当前索引包含 Rust，但不是冻结快照；本篇结论以 Git blob 为准。Ready 文档校验确认 11 章节、引用存在和差异无空白错误；按计划不运行 Cargo，不验证真实网络或存储。
