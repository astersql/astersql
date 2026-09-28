// Copyright 2026 AsterSQL.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 配置工具函数的单元测试（对应 config_util 模块）。
//
// 覆盖三类能力：
// - `CloneConf`：通过 JSON 序列化/反序列化实现配置的深拷贝；
// - `MergeConfigItems`：将新配置合并到旧配置，仅接受“动态配置项”
//   （即运行时可在线修改、无需重启数据库进程的配置），拒绝静态项；
// - `FlattenConfigItems`：把嵌套的配置映射按 `a.b` 点号路径展平，
//   便于按键名逐项比较与合并。

use astersql_config::*;
use serde_json::{Value, json};
use std::collections::HashMap;

/// 验证 `CloneConf` 是基于 JSON 的深拷贝：
/// 克隆结果与原配置逐字段相等，且修改克隆体不会影响原配置。
#[test]
fn clone_conf_is_a_deep_json_clone() {
    let original = Config {
        // store 表示底层存储引擎类型（tikv 为分布式 KV 存储引擎）
        store: "tikv".to_owned(),
        port: 4000,
        repair_table_list: vec!["t1".to_owned()],
        ..Config::default()
    };

    let mut cloned = CloneConf(&original).expect("config should round-trip through JSON");
    // 克隆结果在 JSON 表示上应与原配置完全一致
    assert_eq!(
        serde_json::to_value(&original).unwrap(),
        serde_json::to_value(&cloned).unwrap()
    );

    // 修改克隆体后，原配置必须保持不变，证明是深拷贝而非共享引用
    cloned.store = "tiflash".to_owned();
    cloned.port = 2333;
    cloned.instance.tidb_enable_ddl.store(false);
    cloned.repair_table_list.push("t2".to_owned());
    assert_eq!(original.store, "tikv");
    assert_eq!(original.port, 4000);
    assert!(original.instance.tidb_enable_ddl.load());
    assert_eq!(original.repair_table_list, ["t1"]);
}

/// 验证 `MergeConfigItems` 只接受动态配置项：
/// 动态项（如性能参数、慢查询阈值）被合并进旧配置，
/// 静态项（如 Store、Port、AdvertiseAddress，需要重启才能生效）被拒绝且保持原值。
#[test]
fn merge_config_items_accepts_only_dynamic_paths() {
    let original = Config::default();
    let mut old_conf = CloneConf(&original).unwrap();
    let mut new_conf = CloneConf(&old_conf).unwrap();

    // 前 6 项为动态配置：包括最大并发线程数、内存上限、
    // 是否允许笛卡尔积连接（cross join）、伪估算比例（统计信息过期时
    // 优化器用于行数估算的比例）、TiKV 客户端 store 限流、慢查询阈值
    new_conf.performance.max_procs = 123;
    new_conf.performance.max_memory = 456;
    new_conf.performance.cross_join = false;
    new_conf.performance.pseudo_estimate_ratio = 0.42;
    new_conf.tikv_client.store_limit = 789;
    new_conf.instance.slow_threshold = 2345;
    // 以下 3 项为静态配置：存储引擎、监听端口、对外通告地址，运行时不可修改
    new_conf.store = "tiflash".to_owned();
    new_conf.port = 2333;
    new_conf.advertise_address = "1.2.3.4".to_owned();

    // 合并返回被接受与被拒绝的配置项名称列表
    let (mut accepted, mut rejected) = MergeConfigItems(&mut old_conf, &new_conf);
    accepted.sort();
    rejected.sort();

    // 被接受的 6 项必须都属于动态配置项集合
    assert_eq!(accepted.len(), 6);
    assert!(
        accepted
            .iter()
            .all(|item| dynamicConfigItems().contains(item.as_str()))
    );
    assert_eq!(rejected, ["AdvertiseAddress", "Port", "Store"]);
    // 动态项应已同步为新值
    assert_eq!(
        old_conf.performance.max_procs,
        new_conf.performance.max_procs
    );
    assert_eq!(
        old_conf.performance.max_memory,
        new_conf.performance.max_memory
    );
    assert_eq!(
        old_conf.performance.cross_join,
        new_conf.performance.cross_join
    );
    assert_eq!(
        old_conf.performance.pseudo_estimate_ratio,
        new_conf.performance.pseudo_estimate_ratio
    );
    assert_eq!(
        old_conf.tikv_client.store_limit,
        new_conf.tikv_client.store_limit
    );
    assert_eq!(
        old_conf.instance.slow_threshold,
        new_conf.instance.slow_threshold
    );
    // 静态项被拒绝，仍保持初始值
    assert_eq!(old_conf.store, original.store);
    assert_eq!(old_conf.port, original.port);
    assert_eq!(old_conf.advertise_address, original.advertise_address);
}

/// 验证 `FlattenConfigItems` 只递归展平嵌套对象（键用 `.` 连接），
/// 而数组（包括元素为对象的数组）作为整体值保留、不被展开。
#[test]
fn flatten_config_items_flattens_objects_but_not_arrays() {
    // 构造包含标量、字符串、数组、对象数组、嵌套对象的多形态输入
    let nested: HashMap<String, Value> = serde_json::from_value(json!({
        "k0": 233333,
        "k1": "v1",
        "k2": ["v2-1", "v2-2", "v2-3"],
        "k3": [{"k3-1": "v3-1"}, {"k3-2": "v3-2"}, {"k3-3": "v3-3"}],
        "k4": {"k4-1": [1, 2, 3, 4], "k4-2": [5, 6, 7, 8], "k4-3": [666]}
    }))
    .unwrap();

    let flat = FlattenConfigItems(nested);
    // k4 的三个子键被展平，其余键保持不变，共 7 项
    assert_eq!(flat.len(), 7);
    assert_eq!(flat["k0"], json!(233333));
    assert_eq!(flat["k1"], json!("v1"));
    assert_eq!(flat["k2"], json!(["v2-1", "v2-2", "v2-3"]));
    assert_eq!(
        flat["k3"],
        json!([{"k3-1": "v3-1"}, {"k3-2": "v3-2"}, {"k3-3": "v3-3"}])
    );
    assert_eq!(flat["k4.k4-1"], json!([1, 2, 3, 4]));
    assert_eq!(flat["k4.k4-2"], json!([5, 6, 7, 8]));
    assert_eq!(flat["k4.k4-3"], json!([666]));
}

/// 验证从 TOML 配置文件解析出的嵌套结构展平后，数组值（如隔离读引擎列表）
/// 仍作为整体保留。isolation-read.engines 指定 SQL 查询允许读取的存储引擎
/// （tikv 行存 / tiflash 列存 / tidb 内存表）。
#[test]
fn flatten_config_items_preserves_toml_arrays() {
    // 先解析 TOML 文本，再转换为 JSON 形态的配置映射
    let nested: HashMap<String, Value> = toml::from_str::<toml::Value>(
        "port=4000\n[log]\nlevel='info'\nformat='text'\n[isolation-read]\nengines=['tikv','tiflash','tidb']\n",
    )
    .and_then(|value| value.try_into())
    .expect("TOML should convert to the JSON-shaped config map");

    let flat = FlattenConfigItems(nested);
    // [log] 与 [isolation-read] 两个表被展平为点号路径键
    assert_eq!(flat.len(), 4);
    assert_eq!(flat["port"], json!(4000));
    assert_eq!(flat["log.level"], json!("info"));
    assert_eq!(flat["log.format"], json!("text"));
    assert_eq!(
        flat["isolation-read.engines"],
        json!(["tikv", "tiflash", "tidb"])
    );
}
