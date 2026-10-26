# PostgreSQL interval 三分量保真

任务32，2026-10-26。前置任务31的提交为
`44cae19a23cf8e2701cefd44c0f67ef101e60751`，其实际证据保留于
`docs/postgresql18-tz-diff.md`。本任务只完成 IV-COMPONENTS 及必要局部接线。

## 实现与来源

第3章间隔契约要求月、日、微秒保持独立。新增
`pkg/types/pg_temporal.rs::PgInterval`，通过
`from_components(months:i32, days:i32, time_microseconds:i64)`、`components()`、
`from_binary()`、`to_binary()` 构造和读写。文本使用整数十进制运算，
`1.5 months` 的小数部分归到15日；整数月不折日，整数日不折小时，24小时不进日。
布局为网络序i64微秒、i32日、i32月，共16字节；全最小/全最大组合识别为无限哨兵。

生产修改依据：

- `pkg/types/pg_temporal.rs`：上述规范表示、postgres基线输入输出、溢出22015。
- `pkg/parser/mysql/type.rs`：独立 `TypePgInterval`；
  `pkg/parser/types/etc.rs`、`field_type.rs`：类型名称与Restore。
- `pkg/parser/lexer.rs`、`parser_actions/expression.rs`：PG interval cast和typed literal
  接入已有类型/时间字面量语法，生成真实Cast AST；MySQL语法保持原路径。
- `pkg/server/pg_extended.rs`：OID1186准入、text/binary参数，错误沿规范值传播。
- `pkg/server/pg_result.rs`：1186/16/-1、两格式结果；NULL与零间隔分开。
- `pkg/session/runtime/control.rs`、`planning.rs`：准备参数身份、OID推导和已有typed binder。
- `pkg/session/runtime/query.rs`、`relational_value.rs`：真实cast求值和分析类型保真。

独立测试修改为 `pkg/types/pg_temporal_test.rs`、`pkg/parser/parser_test.rs`、
`pkg/session/runtime/relational_value_test.rs`、`pkg/server/pg_temporal_test.rs`。
用户续发授权允许补齐上述不可缺少的局部接口；没有扩建存储、通用类型系统或运算框架。
没有外部依赖、Go源或Bazel元数据修改，未触发bazel_prepare。

## 行为证据

`target/iv32-red-clean.log`：基线编译成功后实际执行1项测试，退出101，
`SELECT $1 AS v` 的Parse对1186返回0A000。早期夹具服务关闭API编译失败不计作红灯。

同一 `pg_temporal_iv_components` 测试使用CreateAnalyzeSession与真实TCP PgService，
保留原断言，覆盖3条真实SQL入口（参数直接投影、::interval、CAST AS interval），
四种参数/结果格式组合、独立符号、分量边界、无限哨兵、NULL、零行、首行NULL、
多行及全NULL、重复Execute和格式切换。检查ParameterDescription、statement/portal
Describe的1186/16/-1、原始行字节与NULL长度。错误分别检查22007、22015、22021、
08P01、22P03，Sync后SELECT 1成功；事务错误状态E经ROLLBACK恢复。

规范输入包括1 month、30 days、1 day、24 hours、1.5 months，其期望三分量分别为
(1,0,0)、(0,30,0)、(0,1,0)、(0,0,86400000000)、(1,15,0)。

`target/iv32-reference.log` 保存隔离PostgreSQL **18.4(Homebrew)** 的实际version、
IntervalStyle=postgres、上述5值、混合负号及i32/i64边界的文本与interval_send十六进制。
参考通过临时initdb/pg_ctl启动，查询退出0，随后停止并删除自己的临时数据。
这只是本项期望值的参考采样，不是后续全族libpq差分或18.6通过证据。

## 验证命令与资源

WIP用于红灯定位，Ready用于交付；验证profile技能缺失，按根协议执行。
有效红灯、Ready全部动态领取槽位1：

    CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1

早期重叠的夹具修正阶段曾动态领取槽位2并终止冷缓存编译；进程结束后已释放。
每阶段以原子mkdir领取，记录任务/会话/PID，EXIT/INT/TERM清理自己的锁，缓存保留。
确切命令（仓库根执行，Cargo继承上述环境）：

    cargo fmt --all
    env -u LDFLAGS cargo test --locked -p astersql-types --lib pg_interval -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-parser --lib pg_interval -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-session --lib pg_interval -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_temporal_iv_components -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_types_test -- --nocapture
    make parser parser_yacc parser_fmt
    tools/check/failpoint-state.sh enable go
    gofmt -w pkg/parser/ast/binding__failpoint_binding__.go
    make parser_unit_test
    tools/check/failpoint-state.sh disable go
    make lint
    git diff --check
    git diff --cached --check

Go准备首次因failpoint工具遍历正在写入的Cargo缓存返回1；错开后重试退出0，
parser及子包通过，failpoints最终refcount=0。临时Go重写已还原。
独立session测试曾因新增测试缺少mut编译失败，修正后重跑；不将编译失败当行为红灯。

## 限制与风险

没有验证18.6、持久TiKV或性能；SQL/ISO输入、其他IntervalStyle、fields/p与桥接等
由后续任务逐项验收，不能从本项推导完整interval兼容。正确性风险集中在未覆盖的输入
拼写；兼容性范围为postgres基线三分量往返，MySQL独立语法测试保留。
实现增加有界标量解析与规范文本载体分配，性能未测。

## 最终结果

`target/iv32-verified.log`退出0：最终fmt后，types 1项、session 1项、真实TCP 1项、
pg_types_test 7项全部通过。parser独立1项在`target/iv32-ready.log`通过，代码此后未改。
合计11项Rust测试通过（不是零测试或仅编译）。`target/iv32-lint.log`的make lint退出0；
`target/iv32-parser-go-retry.log`的Go parser测试及清理退出0；parser/parser_yacc/parser_fmt
已在`target/iv32-parser-go.log`中成功执行。git diff检查通过。
所有本任务Cargo锁已释放，临时参考PG已停止并清理。
