# 常用类型与错误状态实施计划

本活文档遵循根目录 PLANS.md。plan.md 始终只读。

## Purpose / Big Picture


PG 3.2 客户端可通过 OID 和文本值区分整数、布尔、文本、数值、日期/时间和 NULL；主键冲突和语法错误返回不同 SQLSTATE。只暴露引擎真实元数据，不改 MySQL 编码或 SQL 执行语义。

## Progress


- [x] 阅读技能、根协议、总计划、任务及测试指南，确认批次 4 完成。
- [x] 使用 RustCodeGraph 与新文件读取检查共享接口。
- [x] 真实会话失败验证 bool OID 25/16，记录共享原始类型缺口。
- [x] 获得明确桥接授权，恢复实施并扩展任务白名单。
- [x] 增加协议无关 NativeType 及只读元数据入口，保留完整布尔标志与显式 CAST 类型。
- [x] 实施 PG 类型/SQLSTATE 编码，添加分文件真实 TCP 和边界回归。
- [x] 取得唯一键 SQLSTATE red 证据，再恢复实现。
- [x] 格式化、库编译、lint 和隔离审查通过。
- [x] 临时排除两个无关缺失夹具消费者时，PG 15 项及 MySQL/runtime 定向检查通过，恢复注册。
- [x] 延后回归完成：夹具已恢复，完整注册下类型 4 项、PG 20 项及 MySQL/runtime 定向检查通过；修复关闭测试 acceptance 竞态，编号任务删除。

## Surprises & Discoveries


真实 SELECT 1 AS v 和 SELECT TRUE AS v 的共享 ColumnInfo/Value 完全相同。布尔 flag 位为 1<<19，MySQL flags 为 u16；部分结果字段缺失并被填为 VAR_STRING。既有元数据解析器对 CAST 也可能回退 text，新增只读入口保留 AST 显式类型。实现期间无关 MySQL 清单文件消失，两个 include_str 阻止全部库测试构建。

## Decision Log


- Decision: 最初因共享元数据缺失停止并记录阻塞。
  Rationale: 值和列名无法区分布尔与整数，禁止猜类型。缺口与替代方案在编号任务详述。
  Date/Author: 2026-09-30 / Codex
- Decision: 按明确补充授权增加 conn.rs/runtime.rs/control.rs 的协议无关元数据桥接以及 lib.rs/pg_query_test.rs 的注册和夹具调整，恢复实施。
  Rationale: NativeType 不含 OID/SQLSTATE/PG 判断；保持 MySQL columns 与值不变；describe_result_fields 复用 AST/catalog，不执行或注册 SQL。CAST 的类型精化仅在新入口。
  Date/Author: 2026-09-30 / Codex
- Decision: PG 在原始元数据缺失/数量不匹配时拒绝编码，未知执行错误 XX000；已知引擎唯一键前缀映射 23505。
  Rationale: 避免伪造元数据或凭任意包含的错误文本分类。引擎已经将错误转换成字符串，可信前缀是当前可用证据。
  Date/Author: 2026-09-30 / Codex
- Decision: 用临时独立 integration target 验证真实 TCP；随后临时排除两个缺失夹具消费者以运行库定向回归，完成后全部恢复/移除并标为待回归。
  Rationale: 不伪造缺失清单、不修改既有 MySQL 测试交付；条件性通过与原始工作树失败明确区分。无依赖增加或本地覆盖。
  Date/Author: 2026-09-30 / Codex

## Outcomes & Retrospective


实现完成。类型 4 项、PG 15 项及 MySQL listener/type/runtime 各 1 项通过，但库测试是在明确临时排除无关夹具消费者的条件下运行；真实 TCP 独立 target 三项也通过。完整注册恢复后仅缺失清单阻止编译，故编号任务保留。下一步恢复该夹具并统一回归。

## Context and Orientation


pg_result.rs 将 QueryResult 编码为 RowDescription/DataRow/CommandComplete，pg_conn.rs 查询并写 ErrorResponse。conn.rs 新 NativeType 存原引擎类型 code/完整 flags/length/decimal，QueryResult.native_types 是原始列类型向量。runtime.rs 从 ConcreteRecordSet 或 session.describe_result_fields 获取类型；control.rs 为既有 AST/catalog 解析器的只读入口，MySQL 元数据解析路径保持不变。源码和测试分文件。

## Plan of Work


先真实会话复现，再桥接原始元数据；PG 模块按原类型选择 OID 与文本编码，unsigned uint64 使用 numeric 保留范围，bytea 用十六进制文本，NULL 用长度 -1。布尔输出 t/f；日期零值与扩展 TIME duration 明确拒绝。结果写入前全部编码，避免错误发生后已发送部分列/行。Parser 语法错误 42601，已知唯一键错误 23505，不支持 0A000，未知执行错误 XX000。

## Concrete Steps


仓库根执行以下命令。精确结果与日志位置见编号任务最终交接。

    cargo test -p astersql-server pg_types_test --lib
    cargo test -p astersql-server sqlstate_unique_syntax_and_recovery --lib
    cargo fmt --all
    cargo test -p astersql-server pg_ --lib
    cargo test -p astersql-server real_listener_serves_handshake_ping_select_and_drains_connection --lib
    cargo test -p astersql-server mysql_type_packets_expose_correct_type_flags_charset_and_decimal --lib
    cargo test -p astersql-server concrete_session_driver_authenticates_and_returns_real_sql_results --lib
    cargo check -p astersql-server --lib
    make lint
    git diff --check

## Validation and Acceptance


真实 TCP 3.2 startup 后 Query 可观察 bigint/bool/text/numeric/date/time/timestamp 的 OID 与文本，以及 NULL -1；重复主键 23505、语法错误 42601，多语句拒绝 0A000，错误后连接可继续查询。共享桥接对照 integer/boolean 保持既有 MySQL ColumnInfo/Value 相同，同时原始布尔标志不同。没有完整工作树 green 前不删除编号任务。

## Idempotence and Recovery


不回滚其他会话改动。所有临时 Cargo 注册、integration harness 和 cfg 排除都已经撤回。下一步恢复真实缺失清单后重跑定向命令；不得生成空清单绕过既有回归。无外部依赖变更，不需要上游 tag。

## Artifacts and Notes


/tmp/pg-task5-red-exact.log：bool OID 25/16 失败。
/tmp/pg-task5-error-red.log：唯一键 CXX000/C23505 失败。
/tmp/pg-task5-types-final.log：4 通过；pg-final.log：15 通过；mysql-listener.log/mysql-types.log/runtime.log 各 1 通过；这些日志均 /tmp/pg-task5- 前缀。
/tmp/pg-task5-harness.log：独立真实 TCP 3 通过。
/tmp/pg-task5-restored.log：恢复原始测试注册后缺失清单编译失败。
/tmp/pg-task5-fmt.log、lint.log、check-final.log：格式化、lint、库编译证据。

## Interfaces and Dependencies


依赖方向为 PG → conn 的 QueryResult/NativeType，runtime → session.describe_result_fields。共享模块不引用 pg_*；OID/SQLSTATE/状态转换仍只在 pg_*.rs。原始类型缺失明确不支持。没有 Cargo 依赖修改、Go/Bazel 文件变更或新外部条件。

更新说明（2026-09-30）：完成实现与条件性定向验证；恢复全部临时测试注册后记录无关缺失夹具，保留任务等待统一回归。

## 延后回归决策（2026-09-30）


- Decision: 最小扩展 pkg/server/pg_conn_test.rs，使用 SSLRequest/N 明确同步 pending 连接已被 PG listener 接受，然后再关闭。
  Rationale: 当前完整注册 PG 全套失败仅发生于未被接受的 pending TCP 连接，ConnectionReset 是关闭监听队列的合法结果；单独运行通过。同步 acceptance 避免竞态且保留强 EOF 断言，不改生产协议、MySQL 或内核。red 证据 /tmp/pg-task5-resume-pg.log: 19 passed/1 failed，line295 ConnectionReset；单独复跑通过。
  Date/Author: 2026-09-30 / Codex

## 最终完整注册回归（2026-09-30）


状态：已完成。tests/mysqlcompat/compatibility-cases.json 已由后续任务恢复，所有测试模块正常注册，不再需要临时排除或 harness。本轮新增修改仅 pkg/server/pg_conn_test.rs 的 pending SSLRequest/N acceptance 同步及本任务记录，没有生产代码修改。已按最小范围授权记录白名单扩展，没有编辑任何其他编号任务/ExecPlan 或只读 plan.md。

Ready 档位用于解除延后回归并交付。下列精确命令从仓库根执行，全部退出 0：

    cargo test -p astersql-server pg_types_test --lib
    cargo test -p astersql-server pg_ --lib
    cargo test -p astersql-server real_listener_serves_handshake_ping_select_and_drains_connection --lib
    cargo test -p astersql-server mysql_type_packets_expose_correct_type_flags_charset_and_decimal --lib
    cargo test -p astersql-server concrete_session_driver_authenticates_and_returns_real_sql_results --lib
    cargo fmt --all
    make lint
    git diff --check

类型 4 通过（8.39s），最终 PG 全套 20 通过（10.53s，包含类型、错误、简单查询、扩展查询和真实 libpq 客户端）；MySQL listener/type/runtime 三条各 1 通过。日志 /tmp/pg-task5-resume-types.log、/tmp/pg-task5-resume-pg-final.log、/tmp/pg-task5-resume-mysql-listener.log、/tmp/pg-task5-resume-mysql-types.log、/tmp/pg-task5-resume-runtime.log、/tmp/pg-task5-resume-fmt.log、/tmp/pg-task5-resume-lint.log。

本轮 red/green：第一次 PG 全套 19 passed/1 failed（/tmp/pg-task5-resume-pg.log），pending.read 收到 ConnectionReset，源码表明 connect 后未确认 accept；单独复跑通过（/tmp/pg-task5-resume-lifecycle.log）。只增加 acceptance 同步后，同样 PG 全套命令 20 passed，保留 authenticated/pending 均 EOF 的强断言。原任务 bool OID 与唯一键 SQLSTATE red/green 证据仍保留在前述历史日志。

边界检查命令均无匹配/变更：

    rg -n 'pg_|postgres|PostgreSQL|SQLSTATE' pkg/server/conn.rs pkg/server/runtime.rs pkg/session/runtime/control.rs
    rg -n 'cfg\(all\(test, any' pkg/server/lib.rs
    git diff --stat -- '.plans/2026-09-30-postgresql-协议第一阶段/plan.md'

仅 Rust PG 测试修复，无新增依赖或 Go/Bazel 变更；不触发 bazel_prepare。Rust 使用已有动态 failpoint，无 Go enable/disable 代码生成。保留原版权，源码/测试继续分文件。Ready 技能缺失，按根协议与 testing-flow 执行适用检查。

正确性风险：类型与错误映射仍限于首阶段支持集；无原始类型/特殊类型/零日期/扩展 TIME 明确拒绝，未知错误 XX000。兼容性：无 MySQL 生产或 SQL 语义变化；测试只消除关闭时未 accept 的竞态，没有接受新的错误结果。性能：本轮仅测试同步，无生产影响；既有全量物化、元数据解析成本未做基准。未验证完整工作区、RealTiKV、生产鉴权/TLS、默认 3.0 客户端和完整 PostgreSQL SQL/事务兼容。

Outcomes & Retrospective 更新：此前缺失 fixture 的外部阻碍已解除，本任务获得正常注册状态下的全部作用域通过证据，无需进一步用户决策。建议后续继续以首阶段支持边界进行集成验收。按执行技能删除编号任务 5，保留本 ExecPlan 的实现、决策与验证证据。
