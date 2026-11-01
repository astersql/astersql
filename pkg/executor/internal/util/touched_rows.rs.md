# `pkg/executor/internal/util/touched_rows.rs`

## 文件定位

executor/internal/util 的 MV 列比较助手；由 lib.rs 导出 MarkTouchedRowsByColumn。

## 核心职责

比较 old/new Column 并将变化标记到 update ordinal 对应的位图，保留已有位。只定义 MV touched 合约，不替换 SQL 比较器。

## 主要符号

MarkTouchedRowsByColumn 接受 update_rows、bitmap、stride、bit_pos、两列、可选 FieldType、not_null 和上下文。

## 执行流程

先检查 field 是否缺失，再允许空 update_rows 返回。按 EvalType 选择比较分支；nullable 列先判断 NULL 相等性，再读取有效负载。stride=1 直接按 ordinal 寻址，其余使用 ordinal*stride+bit_pos/8。

## 数据与状态

整数按 signed/unsigned 读取；BIT 任一列为变长时比较原始字节，避免前置 NULL 造成定长错位。FLOAT 用 f32，DOUBLE 用 f64，正负零相等、NaN 不等。DECIMAL 使用数值 Compare。string 使用二进制字节；ENUM/SET 跳过八字节数值前缀只比较名称。时间、duration 按值，JSON/vector 按原始负载。

## 依赖与调用关系

依赖 parser/types 的 FieldType/EvalType 和 util/chunk 的 Column。COMPLETE builder 直接传入输入列；FAST builder 将已有 Datum 目标批量物化为列后调用。独立测试挂在 migration_aster_unit_test.rs；Datum 依赖仅用于测试。

## 错误处理与边界

缺失 field 返回带 context 的错误；不支持的 EvalType 返回错误。与来源相同，update row、bitmap 布局、两列类型和 not_null 保证由调用者负责；非法下标会触发安全 Rust 越界检查。NULL 分支不会读取变长 NULL 的空负载。

## 并发与资源生命周期

无全局状态或线程任务；只借用列及可变 bitmap，调用结束释放借用。比较器不拥有事务、锁或外部资源。

## 与 Go 版本的对应关系

逐分支对应来源 fba22fa599b70bb8e688c8ca4fb6c287fd1a4d53 的 touched_rows.go。Rust 用一个共有 NULL/mark 循环闭包保留 Go 各分支行为，typed getter 直接读取元素，避免 Rust 批量 getter 返回 Vec 的额外复制。二进制名称直接切原始字节，等价于 Go binary collator 的字节比较。源码已接入两种真实 MV writer；不是仅测试可见助手。

## 扩展指南

新增类型必须同时核对 Go EvalType 分支、NULL 存储、projection 物理布局、位图 stride 和乱序 update ordinal，测试应放独立文件。不得以 SQL collation 替代二进制 touched 规则。
