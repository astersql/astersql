// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! Go-equivalent tests for `br/pkg/backup/schema_merge_option_test.go`.
//!
//! 对齐 Go：校验 placement rule ID 格式，以及带 merge_option 的 schema 备份写出结果。
//! 不启真实 TiDB/TiKV：用 `MemMeta` + `MemKvStorage` + `MemMetaWriter` 固定元数据与写出侧。
//! Rule ID 必须与 DDL `label.NewRuleID` 一致（小写路径），否则 placement 规则无法命中。
//! `IsMergeOptionAllowed` / `PartitionMergeOptionAllowed` 在本场景应为空/false，
//! 证明备份元数据默认不放开 merge_option（与 Go 集成断言同向）。
//! Progress 计数应对齐写出 schema 条数（普通表 + 分区表 = 2）。
//! 覆盖子场景：普通表、单分区、多分区、DDL/BR 小写一致性、全流程 BackupSchemas。
//! `IdentityCodec` 保证 rule id 路径段不被额外转码，便于与字符串字面量直接比较。
//! `skip_checksum=true`：空 Snapshot 无法算真实校验和，本测试只关心 merge_option 标志。
//! `ts=u64::MAX` 对齐 Go `math.MaxUint64` 的“尽可能新”备份点语义。
//! 过滤器只用表级白名单，不按 schema 放行，避免无关库干扰断言。
//! MemMetaWriter 收集写出的 schema 字节，供事后反序列化 TableInfo。
//! 断言核心：merge_option 默认关闭，防止恢复侧误合并分区放置规则。
//! 与 Go 文件同名子测试一一对应，便于对照失败信息。
//! 不验证 checksum 数值正确性；那是 checksum 包测试的职责边界。
//! 分区定义 ID 21/22/23 仅作稳定夹具，不依赖真实分配器。
//! 普通表无 Partition 字段，写出后仍应为 None。

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

// client/schema 为被测入口；stubs 提供无集群依赖的内存替身。
use crate::client::{BuildBackupRangeAndInitSchema, BuildBackupSchemas};
use crate::schema::NewBackupSchemas;
use crate::stubs::Codec;
use crate::stubs::filter::{AllowList, Filter};
use crate::stubs::glue::Progress;
use crate::stubs::label;
use crate::stubs::meta::MemMeta;
use crate::stubs::metautil::MemMetaWriter;
use crate::stubs::model::{CIStr, DBInfo, PartitionDefinition, PartitionInfo, TableInfo};
use crate::stubs::{
    Context, IdentityCodec, KvClient, Result, Snapshot, Storage, Version, checksum,
};

/// 与 Go vardef 侧 checksum 并发默认值对齐，仅作 BackupSchemas 参数透传。
const DEF_CHECKSUM_TABLE_CONCURRENCY: u32 = 4;

/// 简易进度：Inc 原子加一，供断言 BackupSchemas 完成次数。
#[derive(Default)]
struct simpleProgress {
    // SeqCst：与 Go atomic 语义同向，避免测试误判进度。
    counter: AtomicI64,
}

impl Progress for simpleProgress {
    /// 每写出一张表的 schema 回调一次。
    fn Inc(&self) {
        self.counter.fetch_add(1, Ordering::SeqCst);
    }
}

impl simpleProgress {
    /// 读取累计进度，期望等于写出 schema 条数。
    fn get(&self) -> i64 {
        self.counter.load(Ordering::SeqCst)
    }
}

/// 空 Snapshot：本测试跳过 checksum，不读 KV。
struct EmptySnap;
impl Snapshot for EmptySnap {}

/// 空 KvClient：同样不参与数据路径。
struct EmptyKv;
impl KvClient for EmptyKv {}

/// 包装 MemMeta 的 Storage：版本固定 100，编解码用 IdentityCodec。
struct MemKvStorage {
    meta: Arc<MemMeta>,
}

impl Storage for MemKvStorage {
    /// 返回空快照；checksum 被跳过时不会真正读取。
    fn GetSnapshot(&self, _ver: Version) -> Box<dyn Snapshot> {
        Box::new(EmptySnap)
    }
    /// 返回空客户端，避免误触远程 KV。
    fn GetClient(&self) -> Box<dyn KvClient> {
        Box::new(EmptyKv)
    }
    /// IdentityCodec：rule id 路径段与字面量一致。
    fn GetCodec(&self) -> Box<dyn Codec> {
        Box::new(IdentityCodec)
    }
    /// 固定版本，避免依赖真实 PD TSO。
    fn CurrentVersion(&self, _scope: &str) -> Result<Version> {
        Ok(Version::New(100))
    }
}

/// TestBackupSchemaMergeOptionRuleIDFormat
/// 对应 Go 子测试：普通表、单分区、多分区、以及 DDL/BR 小写一致性。
#[test]
fn test_backup_schema_merge_option_rule_id_format() {
    let codec = IdentityCodec;

    // normal table rule ID format
    // 空分区名 → `schema/{db}/{table}`，无第四段。
    {
        let db_name = "test";
        let table_name = "t1";
        let expected_rule_id = label::NewRuleID(&codec, db_name, table_name, "");
        assert_eq!("schema/test/t1", expected_rule_id);
    }

    // partition table rule ID format
    // 分区名非空 → 追加 `/p0`，与 TiDB placement 规则键一致。
    {
        let db_name = "test";
        let table_name = "pt1";
        let partition_name = "p0";
        let expected_rule_id = label::NewRuleID(&codec, db_name, table_name, partition_name);
        assert_eq!("schema/test/pt1/p0", expected_rule_id);
    }

    // multiple partitions rule ID format
    // 多分区各自独立 rule ID，互不覆盖。
    {
        let db_name = "test";
        let table_name = "pt2";
        let partitions = ["p0", "p1", "p2"];
        let expected = [
            "schema/test/pt2/p0",
            "schema/test/pt2/p1",
            "schema/test/pt2/p2",
        ];
        for (i, part_name) in partitions.iter().enumerate() {
            let rule_id = label::NewRuleID(&codec, db_name, table_name, part_name);
            assert_eq!(
                expected[i], rule_id,
                "partition {part_name} rule ID should match"
            );
        }
    }

    // rule ID format matches DDL behavior - case insensitive (.L lowercase)
    // Go 注释强调 DDL/BR 均用 .L；此处输入已是小写，验证两侧 NewRuleID 输出相同。
    {
        let ddl_rule_id = label::NewRuleID(&codec, "testdb", "testtable", "");
        let ddl_partition_rule_id = label::NewRuleID(&codec, "testdb", "testtable", "partition0");
        let br_rule_id = label::NewRuleID(&codec, "testdb", "testtable", "");
        let br_partition_rule_id = label::NewRuleID(&codec, "testdb", "testtable", "partition0");

        assert_eq!(
            ddl_rule_id, br_rule_id,
            "DDL and BR should generate same table rule ID"
        );
        assert_eq!(
            ddl_partition_rule_id, br_partition_rule_id,
            "DDL and BR should generate same partition rule ID"
        );
        assert_eq!("schema/testdb/testtable", ddl_rule_id);
        assert_eq!("schema/testdb/testtable/partition0", ddl_partition_rule_id);
    }
}

/// TestBackupSchemaWithMergeOption
/// 对应 Go 全流程：构建 range/schema → BackupSchemas 写出 → 校验 merge_option 标志。
#[test]
fn test_backup_schema_with_merge_option() {
    // 内存库：一张普通表 + 一张三分区表，过滤白名单只含二者。
    let meta = Arc::new(MemMeta::default());
    meta.dbs.lock().unwrap().push(DBInfo {
        ID: 1,
        Name: CIStr::new("test"),
        PlacementPolicyRef: None,
    });
    let normal = TableInfo {
        ID: 10,
        Name: CIStr::new("t_normal"),
        Version: 1,
        HasAutoInc: true,
        ..Default::default()
    };
    let partition = TableInfo {
        ID: 20,
        Name: CIStr::new("t_partition"),
        Version: 1,
        HasAutoInc: true,
        Partition: Some(PartitionInfo {
            Definitions: vec![
                PartitionDefinition {
                    ID: 21,
                    Name: CIStr::new("p0"),
                },
                PartitionDefinition {
                    ID: 22,
                    Name: CIStr::new("p1"),
                },
                PartitionDefinition {
                    ID: 23,
                    Name: CIStr::new("p2"),
                },
            ],
        }),
        ..Default::default()
    };
    meta.tables
        .lock()
        .unwrap()
        .insert(1, vec![normal, partition]);

    let kv = MemKvStorage { meta: meta.clone() };
    // ts=MAX 表示“尽可能新”的备份时间点语义，与 Go math.MaxUint64 用法同向。
    let ts = u64::MAX;
    let test_filter = AllowList {
        schemas: vec![],
        tables: vec![
            ("test".into(), "t_normal".into()),
            ("test".into(), "t_partition".into()),
        ],
    };
    // 初始化阶段应得到 2 个 backup schema 条目。
    let (_ranges, backup_schemas, _policies) =
        BuildBackupRangeAndInitSchema(&kv, &test_filter, ts, false, meta.as_ref()).unwrap();
    assert_eq!(backup_schemas.unwrap().Len(), 2);

    let ctx = Context::Background();
    let mw = MemMetaWriter::default();
    let update_ch = simpleProgress::default();
    // 跳过 checksum：空 Snapshot 不足以算真实校验和。
    let skip_checksum = true;

    let filter: Arc<dyn Filter> = Arc::new(AllowList {
        schemas: vec![],
        tables: vec![
            ("test".into(), "t_normal".into()),
            ("test".into(), "t_partition".into()),
        ],
    });
    let meta2 = meta.clone();
    // NewBackupSchemas 注入与 Go 相同的 BuildBackupSchemas 回调。
    let schemas = NewBackupSchemas(
        Arc::new(move |storage, fn_| {
            BuildBackupSchemas(storage, filter.as_ref(), ts, false, meta2.as_ref(), fn_)
        }),
        2,
    );
    schemas
        .BackupSchemas(
            &ctx,
            &mw,
            None,
            &kv,
            None,
            ts,
            None,
            1,
            DEF_CHECKSUM_TABLE_CONCURRENCY,
            skip_checksum,
            Some(&update_ch),
        )
        .unwrap();
    // 进度应恰好为 2（两张表各一次 Inc）。
    assert_eq!(update_ch.get(), 2);

    let schemas_out = mw.schemas.lock().unwrap().clone();
    assert_eq!(schemas_out.len(), 2);

    // 按表名拆开，分别校验分区元数据与 merge_option 标志。
    let mut normal_table = None;
    let mut partition_table = None;
    for schema in schemas_out {
        let info: TableInfo = serde_json::from_slice(&schema.Table).unwrap();
        if info.Name.O == "t_normal" {
            normal_table = Some((schema, info));
        } else if info.Name.O == "t_partition" {
            partition_table = Some((schema, info));
        }
    }
    let (normal_schema, normal_info) = normal_table.expect("normal table should be found");
    let (part_schema, part_info) = partition_table.expect("partition table should be found");

    // 分区元数据必须完整保留，不能被 flatten。
    assert!(
        normal_info.Partition.is_none(),
        "normal table should not have partition info"
    );
    assert!(
        part_info.Partition.is_some(),
        "partition table should have partition info"
    );
    assert_eq!(
        part_info.Partition.as_ref().unwrap().Definitions.len(),
        3,
        "should have 3 partitions"
    );

    // 默认备份路径不允许 merge_option；分区级允许列表也应为空。
    assert!(!normal_schema.IsMergeOptionAllowed);
    assert!(!part_schema.IsMergeOptionAllowed);
    assert!(normal_schema.PartitionMergeOptionAllowed.is_empty());
    assert!(part_schema.PartitionMergeOptionAllowed.is_empty());
    // 保留对 checksum 类型的链接，避免未使用导入在部分配置下告警。
    let _ = checksum::ChecksumResponse::default();
}
