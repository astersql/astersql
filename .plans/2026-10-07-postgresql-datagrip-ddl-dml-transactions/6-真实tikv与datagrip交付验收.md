# 任务 6: 真实 TiKV 与 DataGrip 交付验收

批次：【批次 5】 依赖批次 4

状态：受阻（DataGrip 2025.1.3 完整元数据同步尚失败）

目的：在真实 TiKV、PG JDBC 和 DataGrip UI 上验收建表、字段变更、CRUD、事务与刷新闭环，并完成 Ready 交付检查。

来源任务：无

预计会话范围：以验收、必要的局部接线修复和文档更新为主；不新增计划外 PostgreSQL 系统对象兼容。

## 文件

- 修改：`pkg/server/pg_client_integration_test.rs`
- 修改：`docs/postgresql-protocol-first-phase.md`
- 修改：`docs/rust/rust-postgresql-tikv-runbook.md`
- 测试：`pkg/server/pg_client_integration_test.rs`

## 上下文

- 先阅读 `docs/agents/testing-flow.md` 的 RealTiKV 生命周期要求和 `docs/postgresql-protocol-first-phase.md` 现有 UI marker 流程。
- 使用隔离端口与专用数据库；不得停止占用默认 5432 的其他任务服务。
- UI 必须实际刷新 Database Explorer 并查看字段，不以 JDBC 查询结果替代 UI 证据。

## Cargo 共享槽位规则（仅 Rust/Cargo 任务）

1. 同一仓库的所有计划和任务共用仓库根目录下的 `target/rust-slot-1` 至 `target/rust-slot-10`，不得为任务、会话或计划另建构建目录，也不得按任务编号取模固定分配。
2. 每次构建/验证阶段开始前，创建仓库根目录下的 `target/rust-slot-locks`；依次用原子命令 `mkdir target/rust-slot-locks/slot-N.lock` 尝试领取槽位，`N` 为 1–10。仅 `mkdir` 成功者获得该槽位；这些相对路径命令从仓库根目录执行。
3. 领取成功后，在自己的锁目录中记录计划、任务、会话标识和持有进程 PID。同一槽位同时只允许一个持有者；已占用时尝试下一个，仅全部 10 个槽位都占用时等待并重试。
4. 将 `CARGO_TARGET_DIR` 设置为领取槽位的绝对路径，例如领取槽位 3 后，从仓库根目录执行 `export CARGO_TARGET_DIR="$PWD/target/rust-slot-3"`。示例编号不代表固定分配。
5. 所有 Cargo 构建、检查、测试以及调用 Cargo 的脚本和子进程均继承该设置，不得通过 `--target-dir` 或其他配置绕过槽位；生成的二进制也从该槽位定位。
6. 该阶段的 Cargo、脚本以及使用槽位产物的子进程全部结束后，删除自己写入的占用记录，并用 `rmdir` 释放自己的锁目录；保留槽位编译缓存供后续任务复用。
7. 正常结束、失败和中断均须清理自己的锁；在后台进程或其他使用槽位产物的进程仍运行时不得提前释放。规划清理步骤时覆盖这三种退出路径。
8. 遗留锁须先核实持有会话及相关进程已结束；无法确认则保持占用，不得删除其他活跃任务的锁、进程或缓存。
9. 生成或更新计划时，统一移除与这些规则冲突的单一 Rust 共享构建空间、逐任务独立目录和 Cargo 缓存全局独占窗口要求；仍须遵守源码文件占用、真实业务依赖和其他共享验证资源的互斥，不能仅因缓存分槽就把任务判定为可并行。
10. 最终验证记录包含实际槽位、绝对路径 `CARGO_TARGET_DIR`、确切命令、退出码和有效测试数量；Rust 代码修改后先运行 `cargo fmt --all`，再运行相应验证。编译成功或零测试不能替代行为验收证据。

## 测试计划

- 行为：真实 TiKV 上，DataGrip 创建 PG 风格表并刷新字段；增加、改名、改类型/默认/NULL、删除字段均可见；编辑器完成参数化 CRUD；提交/回滚和失败事务恢复符合任务 5。
- 失败验证测试：先在交付分支运行完整场景，任何 UI/客户端失败都保存确切 SQL、SQLSTATE 与阶段，必要时将其转成聚焦回归测试后再修复。
- 失败验证命令：`CARGO_TARGET_DIR="$CARGO_TARGET_DIR" PROTOC=/opt/homebrew/opt/protobuf@21/bin/protoc cargo test -p astersql-server --lib pg_client_integration_test::pg_introspection_clients -- --exact --test-threads=1 --nocapture`
- 预期失败原因：真实 TiKV、驱动或 UI 可能暴露 MockTiKV 和协议单测未覆盖的目录刷新、参数格式或事务边界。
- 通过验证命令：同上，加真实 TiKV server/DataGrip 手动场景和 Ready 检查。
- 模拟策略：最终验收不 mock TiKV、JDBC 或 DataGrip；MockTiKV 仅用于先行回归。

## 步骤

1. 按仓库规范启动带唯一 tag/端口的 TiUP playground，并用 trap 覆盖成功、失败、中断清理。
2. 启动 Rust server 的隔离 MySQL/PG/status 端口，创建专用数据库。
3. 运行 libpq/JDBC 客户端回归，再用 DataGrip UI 完成建表、字段生命周期、CRUD 和事务场景；保存版本、SQL、SQLSTATE、可观察结果与截图路径。
4. 若发现失败，只做可复现的局部修复并新增聚焦失败测试；不顺带补齐无关系统对象。
5. 更新两份文档的支持矩阵、非目标、启动方式和验收日期。
6. 执行 `cargo fmt --all`、聚焦 Rust 回归、`make lint`、差异检查和自审；本任务不触发 Bazel metadata 条件，若实际新增/移动 Go 文件或修改 Go import，先运行 `make bazel_prepare`。
7. 停止 server/playground，删除本任务 TiUP 数据并验证 PD 端点不可达；保留用户既有服务和数据。

## 验证

- 运行：`cargo fmt --all`
- 运行：`cargo test -p astersql-server --lib pg_ -- --test-threads=1 --nocapture`
- 运行：`make lint`
- 运行：`git diff --check && git diff --cached --check`
- 预期：聚焦测试有效数量大于零，两个 JDBC 版本通过，DataGrip UI 实际显示最终字段结构，RealTiKV 数据提交/回滚结果正确，所有进程和测试数据清理完成。
- 所需证据：Ready profile 选择理由、确切命令/退出码/测试数、槽位与 `CARGO_TARGET_DIR`、JDBC/DataGrip/TiKV 版本、UI 观察、清理检查、未验证项。

## 完成

只有真实 UI、JDBC 和 RealTiKV 场景均有证据才标记 `已完成`。完成后使用 `$git-commit` 提交本任务且删除任务文件；因无关本地环境无法完成真实回归时标记 `已完成，待回归` 并保留文件。

## 2026-10-09 真实环境执行记录

- 依赖提交已存在：`520e080fa8`、`10557da5e9`、`da7e4ac62b`、`d04cdc518f`、`8c66c5b1ad`。
- 真实 TiKV：TiUP 1.17.1 启动 TiKV 8.5.1，隔离 tag `astersql-task6-20261009`，PD `127.0.0.1:13379`；Rust server 使用 MySQL `14001`、PG `15432`、status `20080`，专用库 `pg_task6_delivery`。
- MockTiKV 先行回归 `pg_introspection_clients`：1 通过、0 失败；JDBC 42.7.13 与 42.7.3 的 27 条冻结 DataGrip SQL、RetrieveColumns/RetrieveIndexColumns 与 prepared CRUD 均通过；libpq 18 的 3.0/3.2 目录、CRUD、参数、I/T/E 与取消通过。
- RealTiKV JDBC：42.7.13 和 42.7.3 均通过 PreparedStatement INSERT/SELECT/UPDATE/DELETE、COMMIT、ROLLBACK，语句错误 `42703` 后为 `25P02`，ROLLBACK 后恢复。
- RealTiKV libpq/psql 18：真实 TCP 上 CREATE TABLE 及 DML COMMIT/ROLLBACK 通过；回滚行不可见，提交行可见。显式事务内 DDL 按已声明边界返回不支持。
- DataGrip 2025.1.3（build 251.26094.87）使用 JDBC 42.7.13，真实 UI Test Connection 显示成功、`18.0 (AsterSQL)`、ping 21 ms。UI 编辑器实际执行 CREATE TABLE，以及 ADD COLUMN、RENAME COLUMN、ALTER TYPE、SET DEFAULT、SET NOT NULL、DROP COLUMN；MySQL 侧验证最终为 `id int NOT NULL PRIMARY KEY` 与 `points bigint NOT NULL DEFAULT 9`。
- Ready 检查：`cargo fmt --all` 和 `make lint` 通过。`cargo test -p astersql-server --lib pg_ -- --test-threads=1 --nocapture` 为 81 通过、1 失败；单独复跑 `pg_catalog_test::pg_introspection_relations_live` 稳定失败，类型断言期望 69、实际 84。本任务未修改 Rust 生产/测试代码，该既有回归失败与 UI 阻塞一并保留。
- 清理：删除专用库，正常停止 Rust server 与 TiUP playground，删除 `/tmp/astersql-task6-tiup`，确认 PD `13379` 不可达，释放 `target/rust-slot-locks/slot-1.lock`；保留 `target/rust-slot-1` 编译缓存。
- 阻塞：DataGrip 完整自动内省发出尚未覆盖的查询，包括 `DateStyle`、`pg_catalog.pg_timezone_names` 和跨库/非 public relation；同步后 Database Explorer 仍显示 ALTER 前的 `id + note` 缓存，不能提供“UI 实际显示最终字段结构”证据。这是本任务的产品兼容缺口，不是无关本地环境，因此不标记完成、不删除本文件。
