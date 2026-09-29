// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// Port of `pkg/meta/meta_test.go`.
// Exercises the real Mutator / key helpers against the in-memory harness KV,
// matching Go assertions without mockstore/Unistore/session bootstrap.
//
// 使用 harness 内存 KV 验证 Mutator / 键助手 / 策略与 AutoID 等路径，
// 对齐 Go 断言且不依赖 mockstore、Unistore 或完整 session 启动。

use std::collections::HashSet;

use astersql_meta::*;

/// 构造绑定默认事务的空 Mutator，供各用例复用。
fn mutator() -> Mutator {
    new_mutator(kv::Transaction::default(), Vec::new())
}

#[test]
fn go_merge_12_starter_bootstrap_version_is_independent() {
    let mut m = mutator();
    assert_eq!(m.get_starter_bootstrap_version().unwrap(), 0);
    m.finish_bootstrap(7).unwrap();
    m.finish_starter_bootstrap(1).unwrap();
    assert_eq!(m.get_starter_bootstrap_version().unwrap(), 1);
    m.finish_starter_bootstrap(10).unwrap();
    assert_eq!(m.get_starter_bootstrap_version().unwrap(), 10);
    assert_eq!(m.get_bootstrap_version().unwrap(), 7);
}

#[test]
fn go_merge_18_reader_exposes_starter_bootstrap_version() {
    let mut m = mutator();
    m.finish_starter_bootstrap(8).unwrap();
    let reader: &dyn reader::Reader = &m;
    assert_eq!(reader.get_starter_bootstrap_version().unwrap(), 8);
}

/// Corresponds to Go `TestPlacementPolicy`.
/// Placement Policy（放置策略）的创建、更新与列表。
#[test]
fn test_placement_policy() {
    let mut m = mutator();
    let mut policy = model::PolicyInfo {
        id: 1,
        name: ast::CiString::new("aa"),
    };
    m.create_policy(&policy).unwrap();
    assert_eq!(policy.id, 1);
    assert!(m.create_policy(&policy).is_err());
    assert_eq!(m.get_policy(1).unwrap(), policy);

    policy.name = ast::CiString::new("bb");
    m.update_policy(&policy).unwrap();
    assert_eq!(m.get_policy(1).unwrap(), policy);
    assert_eq!(m.list_policies().unwrap(), vec![policy.clone()]);
}

/// Corresponds to Go `TestMaskingPolicy`.
/// Masking Policy（脱敏策略）CRUD 与列表。
#[test]
fn test_masking_policy() {
    let mut m = mutator();
    let mut policy = model::MaskingPolicyInfo {
        id: 1,
        name: ast::CiString::new("mp1"),
    };
    m.create_masking_policy(&policy).unwrap();
    assert!(m.create_masking_policy(&policy).is_err());
    assert!(m.get_masking_policy(2).is_err());
    assert_eq!(m.get_masking_policy(1).unwrap(), policy);

    policy.name = ast::CiString::new("mp1-updated");
    m.update_masking_policy(&policy).unwrap();
    assert_eq!(m.get_masking_policy(1).unwrap(), policy);
    assert_eq!(m.list_masking_policies().unwrap(), vec![policy]);
}

/// Corresponds to Go `TestResourceGroup`.
/// Resource Group（资源组）默认组、增删改与列表。
#[test]
fn test_resource_group() {
    let mut m = mutator();
    let groups = m.list_resource_groups().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0], default_group_meta_for_test());

    let group_id = 2;
    let mut rg = model::ResourceGroupInfo {
        id: group_id,
        name: ast::CiString::new("aa"),
        ru_per_sec: 100,
        burst_limit: 0,
        priority: ast::MEDIUM_PRIORITY_VALUE,
    };
    m.add_resource_group(&rg).unwrap();
    assert_eq!(m.get_resource_group(group_id).unwrap().ru_per_sec, 100);
    assert_eq!(m.list_resource_groups().unwrap().len(), 2);

    rg.ru_per_sec = 200;
    m.update_resource_group(&rg).unwrap();
    assert_eq!(m.get_resource_group(group_id).unwrap().ru_per_sec, 200);

    m.drop_resource_group(group_id).unwrap();
    assert!(m.get_resource_group(group_id).is_err());
}

/// Corresponds to Go `TestMeta` (core database/table/auto-id paths).
/// 核心路径：建库建表、AutoID 读写、全局 ID、SchemaDiff、DDL 历史。
#[test]
fn test_meta() {
    let mut m = mutator();
    let db = model::DbInfo {
        id: 1,
        name: ast::CiString::new("a"),
        charset: "utf8mb4".into(),
        collate: "utf8mb4_bin".into(),
    };
    m.create_database(&db).unwrap();
    assert!(m.create_database(&db).is_err());

    let mut table = model::TableInfo {
        id: 1,
        name: ast::CiString::new("t"),
        version: model::TABLE_INFO_VERSION_5,
        has_auto_increment_column: true,
        auto_random_bits: 5,
        ..Default::default()
    };
    m.create_table_and_set_auto_id(
        1,
        &table,
        &model::AutoIdGroup {
            row_id: 10,
            increment_id: 20,
            random_id: 30,
        },
    )
    .unwrap();
    table.name = ast::CiString::new("t2");
    m.update_table(1, &mut table).unwrap();
    assert_eq!(table.revision, 1);

    let loaded = m.get_table(1, 1).unwrap().unwrap();
    assert_eq!(loaded.name.original, "t2");
    assert_eq!(loaded.revision, 1);
    assert_eq!(
        m.list_tables(&context::Context::default(), 1)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(m.list_simple_tables(1).unwrap()[0].name.original, "t2");

    let mut accessors = m.get_auto_id_accessors(1, 1);
    assert_eq!(
        accessors.get().unwrap(),
        model::AutoIdGroup {
            row_id: 10,
            increment_id: 20,
            random_id: 30
        }
    );
    assert_eq!(accessors.row_id().inc(3).unwrap(), 13);
    drop(accessors);

    assert_eq!(m.gen_global_id().unwrap(), 1);
    assert_eq!(m.gen_global_ids(3).unwrap(), vec![2, 3, 4]);
    assert_eq!(m.get_global_id().unwrap(), 4);

    assert_eq!(m.gen_schema_versions(2).unwrap(), 2);
    assert_eq!(m.get_schema_version_with_non_empty_diff().unwrap(), 1);
    m.set_schema_diff(&model::SchemaDiff { version: 2 })
        .unwrap();
    assert_eq!(m.get_schema_version_with_non_empty_diff().unwrap(), 2);

    let job = model::Job {
        id: 9,
        schema_name: "a".into(),
        table_name: "t2".into(),
        raw_args: Vec::new(),
    };
    m.add_history_ddl_job_public(&job, true).unwrap();
    assert_eq!(m.get_history_ddl_count().unwrap(), 1);
    assert_eq!(m.get_history_ddl_job(9).unwrap().unwrap(), job);
}

/// Corresponds to Go `TestSnapshot` (reader sees committed mutator state).
/// 快照 Reader 应能读到同一底层 state 上 Mutator 写入的库信息。
#[test]
fn test_snapshot() {
    let txn = kv::Transaction::default();
    let snapshot = txn.snapshot();
    let mut m = new_mutator(txn, Vec::new());
    let db = model::DbInfo::public_system(7, "snap", "utf8mb4", "utf8mb4_bin");
    m.create_database(&db).unwrap();
    let reader = new_reader(snapshot);
    assert_eq!(reader.get_database(7).unwrap().unwrap().name.lower, "snap");
}

/// Corresponds to Go `TestElement`.
/// Element（列/索引元素）编解码与非法前缀错误。
#[test]
fn test_element() {
    for type_key in [COLUMN_ELEMENT_KEY.as_slice(), INDEX_ELEMENT_KEY.as_slice()] {
        let element = Element {
            id: -9,
            type_key: type_key.to_vec(),
        };
        let encoded = element.encode_element();
        assert_eq!(encoded.len(), 13);
        let decoded = decode_element(&encoded).unwrap();
        assert_eq!(decoded.id, -9);
        assert_eq!(decoded.type_key, type_key);
    }
    assert!(decode_element(b"_bad_12345678").is_err());
}

/// Corresponds to Go `BenchmarkGenGlobalIDs` / `BenchmarkGenGlobalIDOneByOne`.
/// 批量与逐个生成全局 ID 后水位一致。
#[test]
fn test_gen_global_ids_paths() {
    let mut m = mutator();
    let ids = m.gen_global_ids(32).unwrap();
    assert_eq!(ids.len(), 32);
    assert_eq!(ids[0], 1);
    assert_eq!(ids[31], 32);
    for _ in 0..8 {
        m.gen_global_id().unwrap();
    }
    assert_eq!(m.get_global_id().unwrap(), 40);
}

/// Corresponds to Go key helper tests.
/// 各类 meta 键的编码、前缀判断与解析往返。
#[test]
fn test_key_helpers() {
    let db_id = 10_i64;
    let db = db_key(db_id);
    assert!(is_db_key(&db));
    assert_eq!(parse_db_key(&db).unwrap(), db_id);

    let table = table_key(db_id);
    assert!(is_table_key(&table));
    assert_eq!(parse_table_key(&table).unwrap(), db_id);

    let auto = auto_table_id_key(db_id);
    assert!(is_auto_table_id_key(&auto));
    assert_eq!(parse_auto_table_id_key(&auto).unwrap(), db_id);

    let random = auto_random_table_id_key(db_id);
    assert!(is_auto_random_table_id_key(&random));
    assert_eq!(parse_auto_random_table_id_key(&random).unwrap(), db_id);

    let seq = sequence_key(db_id);
    assert!(is_sequence_key(&seq));
    assert_eq!(parse_sequence_key(&seq).unwrap(), db_id);
}

/// Corresponds to Go `TestIterDatabases`.
/// 遍历数据库列表，以及回调中途返回错误时停止。
#[test]
fn test_iter_databases() {
    let mut m = mutator();
    for (id, name) in [(1, "db1"), (2, "db2"), (3, "db3")] {
        m.create_database(&model::DbInfo {
            id,
            name: ast::CiString::new(name),
            charset: String::new(),
            collate: String::new(),
        })
        .unwrap();
    }
    let mut names = Vec::new();
    m.iter_databases(|info| {
        names.push(info.name.original.clone());
        Ok(())
    })
    .unwrap();
    names.sort();
    assert_eq!(names, vec!["db1", "db2", "db3"]);

    let mut count = 0;
    let err = m
        .iter_databases(|_| {
            count += 1;
            if count == 2 {
                Err(errors::new("stop"))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
    assert!(err.to_string().contains("stop"));
    assert_eq!(count, 2);
}

/// Corresponds to Go `TestCreateMySQLDatabase`.
/// 确保 `mysql` 系统库存在；nextgen 使用固定 SYSTEM_DATABASE_ID。
#[test]
fn test_create_mysql_database() {
    let mut m = mutator();
    let db_id = m.create_mysql_database_if_not_exists().unwrap();
    if kerneltype::is_next_gen() {
        assert_eq!(db_id, metadef::SYSTEM_DATABASE_ID);
    } else {
        assert_eq!(db_id, 1);
    }
    let another = m.create_mysql_database_if_not_exists().unwrap();
    assert_eq!(db_id, another);
}

/// Corresponds to Go `TestIsTableInfoMustLoad`.
/// 表 JSON 含外键/分区/affinity 等特殊属性时必须加载完整 TableInfo。
#[test]
fn test_is_table_info_must_load() {
    assert!(is_table_info_must_load_public(
        br#"{"fk_info":null,"partition":{"expr":"a"},"Lock":null,"tiflash_replica":null,"temp_table_type":0,"policy_ref_info":null,"ttl_info":null}"#
    ));
    assert!(is_table_info_must_load_public(
        br#"{"fk_info":null,"partition":null,"Lock":null,"tiflash_replica":null,"temp_table_type":0,"policy_ref_info":null,"ttl_info":null,"affinity":{"level":"s"}}"#
    ));
    assert!(is_table_info_must_load_public(
        br#"{"fk_info":[{"id":1}],"partition":null,"Lock":null,"tiflash_replica":null,"temp_table_type":0,"policy_ref_info":null,"ttl_info":null}"#
    ));
    assert!(!is_table_info_must_load_public(
        br#"{"fk_info":null,"partition":null,"Lock":null,"tiflash_replica":null,"temp_table_type":0,"policy_ref_info":null,"ttl_info":null}"#
    ));
}

/// Corresponds to Go `TestIsTableInfoMustLoadSubStringsOrder`.
/// must-load 子串检查顺序与 Go 常量表一致。
#[test]
fn test_is_table_info_must_load_sub_strings_order() {
    assert_eq!(CHECK_ATTRIBUTES_IN_ORDER.len(), 7);
    assert_eq!(CHECK_ATTRIBUTES_IN_ORDER[0].attr, r#""partition":null"#);
    assert_eq!(CHECK_ATTRIBUTES_IN_ORDER[6].attr, r#""affinity":{"#);
}

/// Corresponds to Go `TestTableNameExtract`.
/// 从表 JSON 快速提取 id/name，含转义字符。
#[test]
fn test_table_name_extract() {
    assert_eq!(unescape(r#"a\"b\\c"#), "a\"b\\c");
    let encoded = br#"{"id":11,"name":{"O":"Orders\"X","L":"orders\"x"},"cols":[]}"#;
    let info = fast_unmarshal_table_name_info(encoded).unwrap();
    assert_eq!(info.id, 11);
    assert_eq!(info.name.original, "Orders\"X");
}

/// Corresponds to Go `TestNameExtractFromJob`.
/// 从 DDL Job JSON 提取 schema/table 名并做匹配。
#[test]
fn test_name_extract_from_job() {
    let job = br#"{"schema_name":"app","table_name":"orders"}"#;
    let (schema, table) = extract_schema_and_table_name_from_job(job).unwrap();
    assert_eq!(schema, "app");
    assert_eq!(table, "orders");
    let schemas = HashSet::from(["app".to_owned()]);
    let tables = HashSet::from(["orders".to_owned()]);
    assert!(is_job_match(job, &schemas, &tables).unwrap());
    assert!(!is_job_match(job, &HashSet::from(["other".to_owned()]), &tables).unwrap());
}

/// Corresponds to Go bootstrap-related special-attribute checks that do not
/// need a full session/DDL stack: must-load markers stay true for FK/partition/
/// affinity payloads used after bootstrap.
/// Bootstrap 后常见特殊属性载荷仍应标记为 must-load。
#[test]
fn test_infoschema_special_attribute_markers() {
    for payload in [
        br#"{"fk_info":[{"id":1}],"partition":null,"Lock":null,"tiflash_replica":null,"temp_table_type":0,"policy_ref_info":null,"ttl_info":null}"#.as_slice(),
        br#"{"fk_info":null,"partition":{"expr":"a"},"Lock":null,"tiflash_replica":null,"temp_table_type":0,"policy_ref_info":null,"ttl_info":null}"#.as_slice(),
        br#"{"fk_info":null,"partition":null,"Lock":null,"tiflash_replica":null,"temp_table_type":1,"policy_ref_info":null,"ttl_info":null}"#.as_slice(),
    ] {
        assert!(is_table_info_must_load_public(payload));
    }
}

/// Corresponds to Go `TestIsDatabaseExist`.
/// 按 ID 判断库是否存在。
#[test]
fn test_is_database_exist() {
    let mut m = mutator();
    assert!(!m.is_database_exist(123).unwrap());
    m.create_sys_database_by_id("aaa", 123).unwrap();
    assert!(m.is_database_exist(123).unwrap());
}

/// Corresponds to Go `TestBootTableVersion`.
/// nextgen 启动表版本与 DDL 表版本读写。
#[test]
fn test_boot_table_version() {
    let mut m = mutator();
    assert_eq!(
        m.get_nextgen_boot_table_version().unwrap(),
        NextGenBootTableVersion::Init as i32
    );
    m.set_nextgen_boot_table_version(NextGenBootTableVersion::Base)
        .unwrap();
    assert_eq!(
        m.get_nextgen_boot_table_version().unwrap(),
        NextGenBootTableVersion::Base as i32
    );
    assert_eq!(
        m.get_ddl_table_version().unwrap(),
        DDLTableVersion::Init as i32
    );
}

/// Corresponds to Go `TestCreateSysDatabaseByIDIfNotExists`.
/// 按固定 ID 创建系统库，重复调用幂等。
#[test]
fn test_create_sys_database_by_id_if_not_exists() {
    let mut m = mutator();
    m.create_sys_database_by_id_if_not_exists("aaa", 123)
        .unwrap();
    assert!(m.is_database_exist(123).unwrap());
    m.create_sys_database_by_id_if_not_exists("aaa", 123)
        .unwrap();
}

/// Corresponds to Go `TestSetGetDXFScheduleTuneFactors` (classic skips).
/// DXF 调度调优因子（仅 nextgen）；classic 内核直接跳过。
#[test]
fn test_set_get_dxf_schedule_tune_factors() {
    if !kerneltype::is_next_gen() {
        return;
    }
    let mut m = mutator();
    assert!(m.get_dxf_schedule_tune_factors("ks").unwrap().is_none());
    let factors = schstatus::TtlTuneFactors {
        amplify_factor: 1.5,
    };
    m.set_dxf_schedule_tune_factors("ks", &factors).unwrap();
    assert_eq!(
        m.get_dxf_schedule_tune_factors("ks")
            .unwrap()
            .unwrap()
            .amplify_factor,
        1.5
    );
}

/// Corresponds to Go must-load / name-info microbenchmarks.
/// must-load 与表名快速反序列化热路径冒烟。
#[test]
fn test_must_load_and_name_info_hot_paths() {
    let payload = br#"{"fk_info":null,"partition":null,"Lock":null,"tiflash_replica":null,"temp_table_type":0,"policy_ref_info":null,"ttl_info":null}"#;
    for _ in 0..64 {
        assert!(!is_table_info_must_load_public(payload));
        let _ = fast_unmarshal_table_name_info(br#"{"id":1,"name":{"O":"t","L":"t"}}"#).unwrap();
    }
}
