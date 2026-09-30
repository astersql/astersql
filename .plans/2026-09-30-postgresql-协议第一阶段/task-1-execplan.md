# PostgreSQL 3.2 startup 边界实施

本活文档遵循根目录 PLANS.md。总计划 plan.md 只读；任务 1 无依赖。

## Purpose / Big Picture


提供独立的 PostgreSQL 3.2 启动包解析器，拒绝非法长度、损坏参数和其他版本。启动包是客户端连接时发送的无消息类型前缀报文。本任务不开放生产 listener，不改 MySQL 或 SQL 执行语义。

## Progress


- [x] (2026-09-30) 阅读规则、计划、共享执行接口和旧项目。
- [x] 新增独立失败测试并运行指定 Cargo 命令；停止时撤回临时源文件修改。
- [x] 获取预期解析缺失失败证据，同时发现既有 MySQL fixture 缺失阻塞。
- [x] 实现解析与原始 TCP 客户端基线 v1；独立原源码 harness 三项测试通过。
- [x] 格式化、make lint 与边界审查已运行。
- [x] (2026-09-30) 延后 Cargo/MySQL 回归、生产编译通过；复用任务 8 完整 lint 成功证据。

## Surprises & Discoveries


pkg/server/doc.go 和 .agents/skills/tidb-verify-profile 不存在。旧项目 /Users/Shared/work/dir/data/codes/TiDB-for-PostgreSQL/server/pg_conn.go 使用 196608 (3.0)，ReceiveStartupMessage 直接依据客户端长度分配。当前工作区有其他任务的 Cargo.lock、session 修改，保持不动。

## Decision Log


- Decision: 不引入 pgwire；选用源码内原始 TCP 客户端基线 v1，显式发送版本 196610。
  Rationale: 本任务只需字节解析，标准库足够，避免 Cargo 白名单外修改。后续 listener/鉴权任务仍须真实服务端验收。
  Date/Author: 2026-09-30 / Codex
- Decision: 单独创建本 ExecPlan，作为任务明确要求的规划文档例外；代码白名单仍仅三个文件。
  Rationale: 用户要求维护独立活文档且禁止修改总计划。
  Date/Author: 2026-09-30 / Codex

- Decision: 标记已阻塞，停止实施，不恢复白名单外 tests/mysqlcompat/compatibility-cases.json，不禁用 MySQL 测试；撤回临时 PG 测试注册与测试文件。
  Rationale: 指定 Cargo 测试退出 101，除预期 E0583 外，mysql_catalog_protocol_test.rs:17 和 mysql_compat_manifest_test.rs:12 的 include_str! 均因 fixture 不存在失败。必须等待文件拥有者恢复或用户明确授权更新计划；独立测试不足以替代 Cargo/MySQL 验收。
  Date/Author: 2026-09-30 / Codex

## Outcomes & Retrospective


实现与聚焦验证已完成，状态为已完成，待回归。安全替代方案是直接编译仓库原始源码与原始独立测试；三项通过，但完整 Cargo/MySQL 回归仍需要权威 fixture。总计划只读。

## Context and Orientation


pkg/server/lib.rs 注册模块。conn.rs 的 TiDBContext 提供 authenticate、execute_query、prepare_statement、execute_prepared_statement、in_transaction 与 cancel；本任务不调用或更改这些接口。PG 专属实现只在 pg_protocol.rs，测试单列 pg_protocol_test.rs。参考 PostgreSQL 18 protocol-message-formats 的 StartupMessage 定义：四字节大端总长度、四字节版本、以零结尾的名称和值对，最后一个零结束。

## Plan of Work


里程碑一注册测试，验证 cargo test 因缺少 pg_protocol 失败。里程碑二实现 parse_startup 与 10000 字节上限；参数使用 UTF-8，保留未知名称、允许空值、拒绝重复名称与尾随字节。里程碑三用回环 TCP 客户端发送显式 3.2 包验证 framing，再执行交付检查。TCP 测试只验证解析边界，不代表已实现生产握手。

## Concrete Steps


在仓库根运行 cargo test -p astersql-server startup_packet_bounds --lib，保存失败证据后实现；运行 cargo fmt --all，重复前述命令，再运行 cargo test -p astersql-server pg_protocol_test --lib 和 cargo test -p astersql-server mysql_protocol_connection_commands_match_mysql_80 --lib，最后 make lint、git diff --check。预期目标测试通过。纯 Rust 改动不触发 Bazel prepare；Rust 目标无 failpoint 依赖，无需 Go failpoint 改写。

## Validation and Acceptance


确认合法 3.2 参数正确解析、3.0 和未知版本明确 UnsupportedVersion、截断/错误长度/缺终止符/重复参数/无效 UTF-8 拒绝，上限内接受、上限外拒绝。最终审查只有模块注册及 pg_*.rs 变化，MySQL 定向回归通过。

## Idempotence and Recovery


验证命令可重复；失败时记录证据，禁止修改白名单外依赖来修复。不要撤销其他任务的工作区变更。

## Artifacts and Notes


`cargo test -p astersql-server startup_packet_bounds --lib` 退出 101：E0583 缺 pg_protocol；另有两处 couldn't read tests/mysqlcompat/compatibility-cases.json。未运行通过验证、格式化、make lint 或 MySQL 回归。阻塞期间无代码交付，仅保留任务状态与本活文档。

## Interfaces and Dependencies


parse_startup(packet: &[u8]) -> Result<StartupMessage, StartupError>；StartupMessage.parameters 为 BTreeMap<String, String>。仅标准库，无共享执行或 MySQL 模块依赖。

## 继续执行的决策与证据


- Decision: 用户要求继续后，在白名单内完成实现，用临时 #[path] harness 验证原源码，保留任务文件为已完成，待回归。
  Rationale: 聚焦验证无需 fixture；唯一历史快照的 fixture 是 []，恢复它会丢失真实用例，不采用。未改共享接口或 MySQL 测试。
  Date/Author: 2026-09-30 / Codex

证据：原源码独立测试 3 passed；cargo fmt --all 和 git diff --check 退出 0。指定 Cargo 通过验证仍退出 101，只有既有 fixture 缺失；make lint 退出 0 但输出 Go 文件遍历与 macOS find 错误，不算完整通过。精确 harness 命令、fixture 调用路径及影响记录在任务文件。

最终补充：cargo check -p astersql-server --lib 退出 101，既有 extract_runtime.rs:138/:286 缺正常依赖 astersql_meta_model/astersql_util_plancodec；MySQL 定向测试退出 101，缺既有 fixture。均未修改白名单外文件。

## 延后回归完成（2026-09-30）


用户授权补跑延后回归并允许最小必要范围扩大。当前共享工作树先前编译缺口已解除，未发现需要本任务修复的失败，没有扩大白名单或新增代码修改。当前使用 Ready 档位，最终任务状态为已完成。

以下精确命令均退出 0：

    cargo test -p astersql-server pg_protocol_test --lib
    cargo test -p astersql-server startup_packet_bounds --lib
    cargo test -p astersql-server mysql_protocol_connection_commands_match_mysql_80 --lib
    cargo check -p astersql-server --lib
    cargo fmt --all
    git diff --check

PG 边界组 3 passed，指定 startup 1 passed，真实 MySQL 连接命令回归 1 passed（6.93s）。生产库编译 Finished dev profile，原 dev-dependencies 缺口已由后续任务解决。失败前证据仍为本活文档记录的 E0583，与当前指定 Cargo 通过形成前后验证。

复用任务 8 Ready 的 make lint（退出 0，/tmp/pg-task8-lint.log），实查日志无 error/warning/failed/illegal option 诊断。本次未改代码，覆盖仍有效，不重复昂贵检查。当前 MySQL/PG 模块边界查询 conn.rs/runtime.rs 无 PG/PostgreSQL/SQLSTATE 引用；总计划 git diff 无输出。仅更新本活文档并删除任务 1 文件，未修改其他任务状态。

正确性风险：仅完成 startup 结构与版本解析；会话参数和鉴权由后续 PG 层校验。兼容性：仅支持明确请求 3.2 的客户端，3.0 明确拒绝。性能：startup 总长限制 10000 字节，无新增性能修改。未单独验证 RealTiKV、生产鉴权/TLS、完整 PostgreSQL SQL、ORM/JDBC 全组合或性能基准。建议主会话汇总各任务交付证据；本任务不再需要补跑。

更新说明：补充当前真实 Cargo 与 MySQL 回归证据，关闭待回归状态，按执行契约删除编号任务 1，保留 ExecPlan。
