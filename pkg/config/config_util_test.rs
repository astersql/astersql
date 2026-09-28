// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 配置工具函数的单元测试模块。
//
// 本模块针对 `config_util` 中的三个核心工具函数进行测试：
// - `CloneConf`：深拷贝配置对象，保证克隆体与原配置互不影响；
// - `MergeConfigItems`：将新配置中的变更合并到旧配置，只接受动态可调项
//   （即无需重启即可在线修改的配置项），拒绝静态项；
// - `FlattenConfigItems`：把嵌套的配置结构展平成 "a.b.c" 形式的扁平键值对，
//   便于逐项比较与合并。

use astersql_config::{
    CloneConf, Config, FlattenConfigItems, MergeConfigItems, dynamicConfigItems,
};
use serde_json::{Value, json};
use std::collections::HashMap;

/// 测试 `CloneConf` 的深拷贝语义：修改克隆体的字段（含原子布尔、动态数组）
/// 不应影响原始配置。
#[test]
fn test_clone_conf() {
    // 构造一份自定义配置：store 表示存储引擎类型（tikv 为分布式 KV 存储），
    // port 为服务端口，repair_table_list 为待修复表列表。
    let original = Config {
        store: "tikv".into(),
        port: 4000,
        repair_table_list: vec!["t1".into()],
        ..Config::default()
    };
    // 连续克隆两次，验证多级克隆后仍与原配置完全等价（通过 JSON 序列化比较）。
    let mut cloned = CloneConf(&CloneConf(&original).unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(&original).unwrap(),
        serde_json::to_value(&cloned).unwrap()
    );

    // 修改克隆体的普通字段、原子布尔（tidb_enable_ddl 控制本节点是否允许执行
    // DDL，即数据定义语句）以及向量字段。
    cloned.store = "tiflash".into();
    cloned.port = 2333;
    cloned.instance.tidb_enable_ddl.store(false);
    cloned.repair_table_list.push("t2".into());
    // 原始配置应保持不变，证明克隆是深拷贝而非共享内部状态。
    assert_eq!(original.store, "tikv");
    assert_eq!(original.port, 4000);
    assert!(original.instance.tidb_enable_ddl.load());
    assert_eq!(original.repair_table_list, ["t1"]);
}

/// 测试 `MergeConfigItems` 的合并规则：仅动态配置项（运行时可在线调整的项）
/// 会被接受并写入旧配置，静态项（如 store、port 等需重启生效的项）会被拒绝。
#[test]
fn test_merge_config_items() {
    let original = Config::default();
    let mut old_conf = CloneConf(&original).unwrap();
    let mut new_conf = CloneConf(&old_conf).unwrap();
    // 前 6 项均为动态可调项：并发度、内存上限、是否允许笛卡尔积连接、
    // 伪估算比例（统计信息过期时优化器使用的估算系数）、TiKV 客户端
    // store 请求限流、慢查询阈值。
    new_conf.performance.max_procs = 123;
    new_conf.performance.max_memory = 456;
    new_conf.performance.cross_join = false;
    new_conf.performance.pseudo_estimate_ratio = 0.42;
    new_conf.tikv_client.store_limit = 789;
    new_conf.instance.slow_threshold = 2345;
    // 后 3 项为静态项：存储引擎类型、端口、对外广播地址，合并时应被拒绝。
    new_conf.store = "tiflash".into();
    new_conf.port = 2333;
    new_conf.advertise_address = "1.2.3.4".into();

    // 执行合并：accepted 为被接受的动态项名称列表，rejected 为被拒绝的静态项。
    let (accepted, rejected) = MergeConfigItems(&mut old_conf, &new_conf);
    // 被接受的 6 项必须全部出现在动态配置项白名单 dynamicConfigItems 中。
    assert_eq!(accepted.len(), 6);
    assert!(
        accepted
            .iter()
            .all(|item| dynamicConfigItems().contains(item.as_str()))
    );
    // 被拒绝的 3 项都不应属于动态配置项白名单。
    assert_eq!(rejected.len(), 3);
    assert!(
        rejected
            .iter()
            .all(|item| !dynamicConfigItems().contains(item.as_str()))
    );
    // 动态项已被合并进旧配置。
    assert_eq!(old_conf.performance.max_procs, 123);
    assert_eq!(old_conf.performance.max_memory, 456);
    assert!(!old_conf.performance.cross_join);
    assert_eq!(old_conf.performance.pseudo_estimate_ratio, 0.42);
    assert_eq!(old_conf.tikv_client.store_limit, 789);
    assert_eq!(old_conf.instance.slow_threshold, 2345);
    // 静态项保持原值，未被新配置覆盖。
    assert_eq!(old_conf.store, original.store);
    assert_eq!(old_conf.port, original.port);
    assert_eq!(old_conf.advertise_address, original.advertise_address);
}

/// 测试 `FlattenConfigItems` 的展平逻辑：只有嵌套对象会被展开为 "父.子" 键，
/// 数组（即使元素是对象）作为整体值保留，不做展开。
#[test]
fn test_flatten_config() {
    // 构造包含标量、字符串、数组、对象数组以及嵌套对象的混合结构。
    let nested: HashMap<String, Value> = serde_json::from_value(json!({
        "k0": 233333,
        "k1": "v1",
        "k2": ["v2-1", "v2-2", "v2-3"],
        "k3": [{"k3-1":"v3-1"}, {"k3-2":"v3-2"}, {"k3-3":"v3-3"}],
        "k4": {"k4-1": [1,2,3,4], "k4-2": [5,6,7,8], "k4-3": [666]}
    }))
    .unwrap();
    // k0~k3 保持原键不变（数组不展开），k4 的三个子键被展平为 "k4.xxx"，
    // 因此总共 4 + 3 = 7 个扁平键。
    let flat = FlattenConfigItems(nested);
    assert_eq!(flat.len(), 7);
    assert_eq!(flat["k0"], json!(233333));
    assert_eq!(flat["k1"], json!("v1"));
    assert_eq!(flat["k2"], json!(["v2-1", "v2-2", "v2-3"]));
    assert_eq!(
        flat["k3"],
        json!([{"k3-1":"v3-1"}, {"k3-2":"v3-2"}, {"k3-3":"v3-3"}])
    );
    assert_eq!(flat["k4.k4-1"], json!([1, 2, 3, 4]));
    assert_eq!(flat["k4.k4-2"], json!([5, 6, 7, 8]));
    assert_eq!(flat["k4.k4-3"], json!([666]));

    // 再用真实的 TOML 配置片段验证：TOML 表（如 [log]、[isolation-read]）
    // 会被展平为带点号前缀的键。isolation-read.engines 指定隔离读允许的
    // 存储引擎列表（tikv 行存 / tiflash 列存 / tidb 内存表）。
    let value: toml::Value = toml::from_str(
        "port=4000\n[log]\nlevel='info'\nformat='text'\n[isolation-read]\nengines=['tikv','tiflash','tidb']",
    )
    .unwrap();
    let nested = serde_json::from_value(serde_json::to_value(value).unwrap()).unwrap();
    let flat = FlattenConfigItems(nested);
    assert_eq!(flat.len(), 4);
    assert_eq!(flat["port"], json!(4000));
    assert_eq!(flat["log.level"], json!("info"));
    assert_eq!(flat["log.format"], json!("text"));
    assert_eq!(
        flat["isolation-read.engines"],
        json!(["tikv", "tiflash", "tidb"])
    );
}
