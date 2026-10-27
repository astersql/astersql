# `pkg/types/pg_jsonb.rs`

## 定位与接口

`PgJsonb` 是独立于原文 `PgJson` 和 MySQL BinaryJSON 的不透明规范文本载体，OID 常量为3802。仅经 `FromStr` / `from_bytes` 校验构造；`as_str`、`as_bytes`、`Display` 返回同一规范文本。SQL NULL使用外层Option，与JSON null及字符串null分别保留。Rust Eq/Hash比较规范拼写，包括numeric显示scale，不是SQL jsonb数值等价运算。

本模块提供规范值入口，不负责SQL定型、参数推导、typed NULL元数据或协议text/binary四方向。后续批次必须沿真实消费者接线，不能用OID常量冒充这些能力已完成。

## 规范化与诊断

复用 `PgJson` 的完整UTF8/原始NUL及JSON词法、结构检查，再解码每个字符串（包括最终被重复键覆盖的值）。serde仅用于单个字符串解码及转义，不构造浮点/serde_json::Value数值树。代理对合并，解码后键按UTF8字节长度、再按字节排序；相同解码键只保留最后值，不进行Unicode NFC归一化。数组保持顺序和重复元素，所有根标量保持种类。数字直接经现有 `PgNumeric` 精确入口输出，无f64或MySQL decimal转换；完整numeric边界及比例验收由下一批补齐。

语法及孤立/错配代理为22P02，解码后U+0000为22P05，非法UTF8/原始NUL为22021；numeric错误沿 `PgNumericError` 传播。错误不回显原始用户输入。孤立代理的分类以PostgreSQL18.6参考行为为准。共享词法完整校验先于语义解码；复合多错误输入的参考错误优先级尚未穷举。

## 实现与资源

显式Frame栈加非递归arena索引暂存标量、数组和对象，输出也用显式Action栈；销毁arena不会沿子值递归。对象HashMap按解码键覆盖，输出时独立排序，不用BTreeMap字典序。规范化结束只保存规范String，丢弃临时arena。总扫描与输出线性，各对象额外O(k log k)键排序，辅助内存O(输入+输出)，比较长键的成本另计。不照搬MySQL深度100，不定义未经参考取证的PG资源上限。独立测试包含4096层嵌套及1000次重复键；性能需单独测量。

## 验证入口与边界

独立pg_jsonb_test.rs覆盖解码重复键、UTF8排序、所有根种类、全部BMP转义、代理边界、容器组合、NULL区分、错误恢复、深嵌套与重复键。规范值测试不代表SQL或协议接线验收。

在仓库根动态领取共享槽位并设置绝对CARGO_TARGET_DIR后执行：

    cargo fmt --all
    cargo fmt --all -- --check
    cargo test -p astersql-types --lib pg05_jsonb_object -- --nocapture --test-threads=1
    cargo test -p astersql-types --lib pg_json_test -- --nocapture --test-threads=1
    cargo test -p astersql-types --lib pg_numeric_test -- --nocapture --test-threads=1
    make lint
    git diff --check

未验证本族SQL定型/参数/结果/客户端、完整PG资源上限、所有复合非法输入错误优先级、性能及持久化。此模块不覆盖其他类型族的客户端验证。
