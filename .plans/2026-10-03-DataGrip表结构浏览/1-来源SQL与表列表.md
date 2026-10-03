# 任务 1: 来源SQL与表列表

批次：【批次 1】 无依赖

状态：未开始

目的：冻结实际 SQL，关联标量子查询和有序 array_agg 支持表列表，证明真实表出现。

来源任务：用户授权“扩展范围，继续修复 DataGrip 表和结构浏览”；2026-10-03 21:10:20 session 1533977259。

预计会话范围：仅 PG 适配模块，按一个验证阶段推进；后续查询差异不吞入当前阶段。

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
- [ ] 修复及聚焦验证。
- [ ] Ready 适用检查与提交。

## Surprises & Discoveries

待执行阶段记录实际发现。

## Decision Log

2026-10-03：所有阶段串行，复用现有 PG 边界且保留 MySQL 隔离。

## Outcomes & Retrospective

尚未获得本阶段完成证据。

发现：实现验证被并行任务更新中的 parquet 依赖阻塞，pkg/dumpformat/parquetfile/file_parser.rs 调用 logical_type，而当前依赖仅提供 logical_type_ref。未修改该范围外文件，继续完成 PG 局部工作后重试。首次失败回归已成功运行，证明来源语法缺口。

中断交接：用户中断时，已完成表列表关联标量子查询、有序 bigint array_agg 与目录字段的初步实现，但最后一次 cargo test 的结果没有取得，不能标记完成。新增 pg_datagrip_tables 和 pg_datagrip_structure 回归；后者尚未实现数组/ANY，预期仍失败。构建使用 target/rust-slot-1，退出后已检查无 cargo/rustc/test 进程并释放自有锁。界面服务 getApp(DataGrip) 启动失败，UI 未验证。

为解除并行 Parquet 工作造成的编译障碍，在 pkg/executor/importer/import.rs 的 OpenParquetParser 返回路径增加 Box<dyn Parser + Send> 到 Box<dyn Parser> 的显式类型转换，不改变解析行为；该文件还有其他任务改动，提交时必须仅选择本 hunk，或交给其所属任务一并处理。
