# 任务 1: 来源SQL与表列表

批次：【批次 1】 无依赖

状态：已完成

目的：冻结实际 SQL，关联标量子查询和有序 array_agg 支持表列表，证明真实表出现。

来源任务：用户授权“扩展范围，继续修复 DataGrip 表和结构浏览”；2026-10-03 21:10:20 session 1533977259。

预计会话范围：每个编号任务使用独立会话，仅 PG 适配模块，按一个验证阶段推进；后续查询差异不吞入当前阶段。

## 文件

pkg/server/pg_catalog_query.rs、pkg/server/pg_catalog.rs、pkg/server/pg_datagrip_test.rs。测试统一在独立 Rust 测试文件，新增注册位于 pkg/server/lib.rs。

## 上下文

执行前读取根 AGENTS.md、PLANS.md、docs/agents/testing-flow.md 和本目录 plan.md。pkg/server/doc.go 不存在。PG 独立 CatalogQuery 执行器已提供参数绑定、JOIN、CTE、快照和有界工作量。

## 测试计划

行为：冻结实际 SQL，关联标量子查询和有序 array_agg 支持表列表，证明真实表出现。

先在 pkg/server/pg_datagrip_test.rs 或既有真实客户端回归加入测试；用完整日志 SQL 与真实元数据断言复现失败。

失败和通过验证命令（从仓库根执行、领取槽位并设置 CARGO_TARGET_DIR 后）：

    PROTOC=/opt/homebrew/opt/protobuf@21/bin/protoc cargo test -p astersql-server pg_datagrip_tables --lib -- --test-threads=1

预期失败原因：当前受限语法、提供器或协议格式尚未覆盖来源查询。真实 session/listener 优先，不 mock 系统目录。

## 步骤与里程碑

先冻结完整来源并编写失败回归；再实现最小 AST、元数据或格式接线。运行聚焦测试并核对真实对象行与类型。新增语法必须保留原有界限及错误恢复，完成后自审差异、运行适用 Ready 检查，按 git-commit 技能提交本任务。

## 验证和完成

记录确切命令、实际构建槽位、预期失败与通过数量。所有生产修改对应原始日志 SQL 或不可缺少的局部接线。编译成功和零测试不是完成证据。完整 UI 验收仅在批次 6 声明；当前阶段不能吞并其余任务。

## Progress

- [x] 冻结 session 1533977259 的 25 条去重完整 SQL；表列表失败回归收到 42601 expected )。
- [x] 修复及聚焦验证：pg_datagrip_tables 1 passed，真实 listener 的 constraints/joins 各 1 passed。
- [x] Ready 适用检查：cargo fmt --all、make lint、聚焦测试、git diff --check 通过；本阶段独立提交。

## Surprises & Discoveries

检查点 7464418c9813c87876c5b33743a65ff23f278daf 已包含完整来源 SQL 和初步实现。当前第一次重跑在 pg_catalog.rs:1164 失败（E0599：RefCell 没有 get），修复缓存借用后所有本阶段验证通过。仅空 pg_inherits 不能证明关联正确，故用真实 pg_class 的两张表补充非空关联身份断言。

## Decision Log

2026-10-03：每个编号任务使用独立会话，所有批次串行，共享当前项目工作区，复用现有 PG 边界且保留 MySQL 隔离。

## Outcomes & Retrospective

尚未获得本阶段完成证据。

发现：实现验证被并行任务更新中的 parquet 依赖阻塞，pkg/dumpformat/parquetfile/file_parser.rs 调用 logical_type，而当前依赖仅提供 logical_type_ref。未修改该范围外文件，继续完成 PG 局部工作后重试。首次失败回归已成功运行，证明来源语法缺口。

中断交接：用户中断时，已完成表列表关联标量子查询、有序 bigint array_agg 与目录字段的初步实现，但最后一次 cargo test 的结果没有取得，不能标记完成。新增 pg_datagrip_tables 和 pg_datagrip_structure 回归；后者尚未实现数组/ANY，预期仍失败。构建使用 target/rust-slot-1，退出后已检查无 cargo/rustc/test 进程并释放自有锁。界面服务 getApp(DataGrip) 启动失败，UI 未验证。

为解除并行 Parquet 工作造成的编译障碍，在 pkg/executor/importer/import.rs 的 OpenParquetParser 返回路径增加 Box<dyn Parser + Send> 到 Box<dyn Parser> 的显式类型转换，不改变解析行为；该文件还有其他任务改动，提交时必须仅选择本 hunk，或交给其所属任务一并处理。


## 本会话交付证据（2026-10-03）

本阶段已完成。之前记录的 42601 expected ) 为检查点前的来源 SQL 失败证据；本会话没有回滚已提交实现重新制造该错误。新观察到的 E0599 编译失败已通过实际构建复现并修复。

生产修改依据：pg_catalog.rs 的 pg_get_indexdef/pg_get_constraintdef 缓存访问增加 providers.borrow()，是检查点把缓存改为 RefCell 后不可缺少的局部编译接线，未新增阶段 2 行为。已有检查点中的 ScalarSubquery/ArrayAgg AST、关联绑定、单次快照缓存和共享工作预算对应 1869280142.sql 的 ancestors/successors；pg_class 的 relispartition/relam/relpartbound 及辅助函数对应该 SQL 的实际投影。复用这些已提交实现，没有继续扩大子系统。

测试修改：pg_datagrip_test.rs 补充真实两表关联 OID 与单元素有序数组、零行标量/聚合 NULL，保留完整表列表 SQL、降序多元素聚合、rename/drop 可见性。未修改 pg_datagrip_structure 或其来源 SQL。

原子锁为 target/rust-slot-1/.agent-lock（mkdir 领取，pid 文件记录进程；EXIT trap 只释放自有锁，保留缓存）。CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1；首次失败 PID=52520，修复后 PID=54378，listener 验证 PID=55016；三次锁均已释放。

从仓库根执行的准确验证命令（Rust 命令均使用上述 CARGO_TARGET_DIR）：

    cargo fmt --all
    PROTOC=/opt/homebrew/opt/protobuf@21/bin/protoc cargo test -p astersql-server pg_datagrip_tables --lib -- --test-threads=1
    PROTOC=/opt/homebrew/opt/protobuf@21/bin/protoc cargo test -p astersql-server pg_introspection_constraints_live --lib -- --test-threads=1
    PROTOC=/opt/homebrew/opt/protobuf@21/bin/protoc cargo test -p astersql-server pg_introspection_joins_live --lib -- --test-threads=1
    make lint
    git diff --check

结果：pg_datagrip_tables 修复前构建退出 101（E0599），修复后 1 passed / 0 failed / 202 filtered；constraints_live 与 joins_live 各 1 passed / 0 failed / 202 filtered；make lint 退出 0；格式化与差异空白检查通过。

使用 Ready 的适用检查要求，因为本阶段交付 Rust 代码与回归；.agents/skills/tidb-verify-profile/SKILL.md 缺失，未声称加载该技能。纯 Rust 变更不触发 bazel_prepare；未运行 bazel_lint_changed。本次未运行 Go 单元测试，Rust 测试不涉及 Go failpoint 开关。

风险与验证限制：本次生产改动仅缓存借用接线，不改变返回值、协议或查询复杂度；聚焦测试覆盖实际 session 元数据及真实 listener，但未运行全套服务器测试、两版 JDBC 或 DataGrip UI。阶段 2–6 的结构查询和最终 UI 验收不在本阶段完成声明内。下一步执行任务 2。plan.md 始终只读，其他任务差异不纳入提交。此前 Parquet/importer 障碍已由现有上游工作区提交解除，本会话未修改这些文件。
