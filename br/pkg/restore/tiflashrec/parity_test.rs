// Copyright 2026 AsterSQL.

//! TiFlashRecorder 公开契约的 Go/Rust 对照测试。
//! 用内存 InfoSchema 覆盖增删改写、DDL 生成与缺表跳过路径，不依赖真实域。
//! 断言聚焦公开 API 语义，避免依赖 HashMap 遍历顺序。

use std::collections::HashMap;

use crate::{CIStr, InfoSchema, TableMeta, TiFlashRecorder, TiFlashReplicaInfo};

/// 内存表字典：仅实现 TableByID，供 DDL 生成查找库表名。
struct MemIS {
    tables: HashMap<i64, (TableMeta, CIStr)>,
}

impl InfoSchema for MemIS {
    // 查找失败返回 None，驱动 DDL 生成跳过缺表。
    fn TableByID(&self, id: i64) -> Option<(TableMeta, CIStr)> {
        self.tables.get(&id).cloned()
    }
}

/// 锁定与 Go 一致的公开行为：Add/Get/Iterate、Rewrite、Load、DDL、缺表跳过。
#[test]
fn go_rust_public_contract_matches() {
    let mut rec = TiFlashRecorder::New();
    // 固定 Count=2 与双标签，便于校验 LOCATION LABELS 拼接。
    let replica = TiFlashReplicaInfo {
        Count: 2,
        LocationLabels: vec!["zone".into(), "rack".into()],
    };

    // 正常路径：Add / Get / Iterate 应看到同一副本配置。
    rec.AddTable(10, replica.clone());
    assert_eq!(rec.GetItems().get(&10), Some(&replica));
    let mut n = 0;
    rec.Iterate(|id, r| {
        assert_eq!(id, 10);
        assert_eq!(r.Count, 2);
        n += 1;
    });
    assert_eq!(n, 1);

    // 边界：同 ID Rewrite 为空操作；跨 ID 迁移后 DelTable 清空。
    rec.Rewrite(10, 10);
    assert!(rec.GetItems().contains_key(&10));
    rec.Rewrite(10, 20);
    assert!(!rec.GetItems().contains_key(&10));
    assert_eq!(rec.GetItems().get(&20), Some(&replica));
    rec.DelTable(20);
    assert!(rec.GetItems().is_empty());

    // Load 整体替换内部 map，而非逐条 merge。
    let mut m = HashMap::new();
    m.insert(1, replica.clone());
    rec.Load(m);
    assert_eq!(rec.GetItems().len(), 1);

    // DDL：存在表时生成 SET REPLICA；Reset 先 0 再恢复原 Count。
    let is = MemIS {
        tables: HashMap::from([(
            1,
            (
                TableMeta {
                    Name: CIStr::new("t"),
                },
                CIStr::new("test"),
            ),
        )]),
    };
    let ddls = rec.GenerateAlterTableDDLs(&is);
    assert_eq!(ddls.len(), 1);
    // 前缀与标签片段分别断言，避免整串对顺序敏感。
    assert!(ddls[0].starts_with("ALTER TABLE `test`.`t` SET TIFLASH REPLICA 2"));
    assert!(ddls[0].contains("LOCATION LABELS 'zone', 'rack'"));

    let reset = rec.GenerateResetAlterTableDDLs(&is);
    // Reset 路径必须产出两条：清零与恢复。
    assert_eq!(reset.len(), 2);
    assert!(reset[0].contains("SET TIFLASH REPLICA 0"));
    assert!(reset[1].contains("SET TIFLASH REPLICA 2"));

    // 缺表 ID 在生成 DDL 时被跳过，不增加输出条数。
    rec.AddTable(99, replica);
    assert_eq!(rec.GenerateAlterTableDDLs(&is).len(), 1);
}
