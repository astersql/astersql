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

//! Go-equivalent tests for `br/pkg/task/config_test.go`.
//!
//! 聚焦恢复配置默认值、客户端参数装配，以及库表名大小写/临时库前缀匹配。
//! 直接验证生产配置与库表备份校验入口，避免测试内自实现掩盖缺失。

use std::collections::{HashMap, HashSet};

use crate::common::{Config, defaultSwitchInterval};
use crate::restore::{
    RestoreClientConfig, RestoreCommonConfig, RestoreConfig, VerifyDBAndTableInBackup,
    configureRestoreClient, defaultPiTRBatchCount, defaultPiTRBatchSize, defaultPiTRConcurrency,
    defaultRestoreConcurrency,
};
use crate::stubs::{
    DefaultMergeRegionKeyCount, DefaultMergeRegionSizeBytes, EncloseDBAndTable, EncloseName,
};

/// 对应 Go `TestRestoreConfigAdjust`：默认并发与合并 region 阈值。
#[test]
fn test_restore_config_adjust() {
    let mut cfg = RestoreConfig::default();
    // 空配置先 Adjust。
    cfg.Adjust();

    // Adjust 后并发应为恢复默认值。
    assert_eq!(cfg.Config.Concurrency, defaultRestoreConcurrency);
    assert_eq!(cfg.Config.SwitchModeInterval, defaultSwitchInterval);
    // 小 region 合并默认与 Go 常量一致。
    assert_eq!(
        cfg.RestoreCommonConfig.MergeSmallRegionKeyCount.Value,
        DefaultMergeRegionKeyCount
    );
    assert_eq!(
        cfg.RestoreCommonConfig.MergeSmallRegionSizeBytes.Value,
        DefaultMergeRegionSizeBytes
    );
    assert_eq!(cfg.SplitRegionIndexStep, 128);
    assert!(!cfg.CoarseScatter);
}

/// Mock restore client capturing configureRestoreClient setters.
/// Corresponds to Go snapclient fields exercised by `TestConfigureRestoreClient`.
/// 捕获 batch DDL / region scan / split / coarse scatter，模拟 Go snapclient setter。
#[derive(Default)]
struct MockRestoreClient {
    batch_ddl: u32,
    region_scan: u32,
    split_step: u32,
    coarse_scatter: bool,
}

impl RestoreClientConfig for MockRestoreClient {
    fn SetBatchDdlSize(&mut self, v: u32) {
        self.batch_ddl = v;
    }
    fn SetRegionScanConcurrency(&mut self, v: u32) {
        self.region_scan = v;
    }
    fn SetSplitRegionIndexStep(&mut self, v: u32) {
        self.split_step = v;
    }
    fn SetCoarseScatter(&mut self, v: bool) {
        self.coarse_scatter = v;
    }
}

impl MockRestoreClient {
    fn GetBatchDdlSize(&self) -> u32 {
        self.batch_ddl
    }
    fn GetRegionScanConcurrency(&self) -> u32 {
        self.region_scan
    }
    fn GetSplitRegionIndexStep(&self) -> u32 {
        self.split_step
    }
    fn GetCoarseScatter(&self) -> bool {
        self.coarse_scatter
    }
}

/// 对应 Go `TestConfigureRestoreClient`：setter 值应原样读回。
#[test]
fn test_configure_restore_client() {
    // 构造与 Go 用例相近的 Config/RestoreCommonConfig。
    let cfg = Config {
        Concurrency: 1024,
        ..Default::default()
    };
    // Online=true 模拟在线恢复场景。
    let restore_com = RestoreCommonConfig {
        Online: true,
        ..Default::default()
    };
    let restore_cfg = RestoreConfig {
        Config: cfg,
        RestoreCommonConfig: restore_com,
        DdlBatchSize: 128,
        RegionScanConcurrency: 3,
        SplitRegionIndexStep: 7,
        CoarseScatter: true,
        ..Default::default()
    };
    let mut client = MockRestoreClient::default();
    configureRestoreClient(&mut client, &restore_cfg);
    // batch DDL 大小回读。
    assert_eq!(client.GetBatchDdlSize(), 128);
    // region scan 并发回读。
    assert_eq!(client.GetRegionScanConcurrency(), 3);
    // split step 回读。
    assert_eq!(client.GetSplitRegionIndexStep(), 7);
    // coarse scatter 开启。
    assert!(client.GetCoarseScatter());
}

/// 对应 Go `TestAdjustRestoreConfigForStreamRestore`：流恢复强制系统表与默认并发。
#[test]
fn test_adjust_restore_config_for_stream_restore() {
    let mut restore_cfg = RestoreConfig::default();
    restore_cfg.adjustRestoreConfigForStreamRestore();
    assert_eq!(restore_cfg.PitrBatchCount, defaultPiTRBatchCount);
    assert_eq!(restore_cfg.PitrBatchSize, defaultPiTRBatchSize);
    assert_eq!(restore_cfg.PitrConcurrency, defaultPiTRConcurrency + 1);
}

/// Corresponds to Go `TestCheckRestoreDBAndTable` / `VerifyDBAndTableInBackup`.
/// Exercise production `VerifyDBAndTableInBackup` with the same case-insensitive
/// `EncloseName` fixtures Go builds.
/// 覆盖大小写混用与 `__TiDB_BR_Temporary_*` 临时库前缀剥离。
#[test]
fn test_check_restore_db_and_table() {
    struct Case {
        cfg_schemas: Vec<&'static str>,
        cfg_tables: Vec<&'static str>,
        backup_dbs: HashMap<&'static str, Vec<&'static str>>,
    }
    let cases = [
        // 普通库表大小写不一致仍应命中。
        Case {
            cfg_schemas: vec!["test"],
            cfg_tables: vec!["test.t", "test.t2"],
            backup_dbs: HashMap::from([("test", vec!["T", "T2"])]),
        },
        // mysql 对应备份侧临时库前缀。
        Case {
            cfg_schemas: vec!["mysql"],
            cfg_tables: vec!["mysql.t", "mysql.t2"],
            backup_dbs: HashMap::from([("__TiDB_BR_Temporary_mysql", vec!["T", "T2"])]),
        },
        // 表名大小写与备份侧相反。
        Case {
            cfg_schemas: vec!["test"],
            cfg_tables: vec!["test.T", "test.T2"],
            backup_dbs: HashMap::from([("test", vec!["t", "t2"])]),
        },
        // 库名全大写。
        Case {
            cfg_schemas: vec!["TEST"],
            cfg_tables: vec!["TEST.t", "TEST.T2"],
            backup_dbs: HashMap::from([("test", vec!["t", "t2"])]),
        },
        // 混杂大小写表名。
        Case {
            cfg_schemas: vec!["TeSt"],
            cfg_tables: vec!["TeSt.tabLe", "TeSt.taBle2"],
            backup_dbs: HashMap::from([("TesT", vec!["TablE", "taBle2"])]),
        },
        // 多库混合：普通库 + 临时 mysql。
        Case {
            cfg_schemas: vec!["TeSt", "MYSQL"],
            cfg_tables: vec!["TeSt.tabLe", "TeSt.taBle2", "MYSQL.taBle"],
            backup_dbs: HashMap::from([
                ("TesT", vec!["table", "TaBLE2"]),
                ("__TiDB_BR_Temporary_mysql", vec!["tablE"]),
            ]),
        },
        // sys 临时库前缀用例。
        Case {
            cfg_schemas: vec!["sys"],
            cfg_tables: vec!["sys.t", "sys.t2"],
            backup_dbs: HashMap::from([("__TiDB_BR_Temporary_sys", vec!["T", "T2"])]),
        },
    ];

    for ca in cases {
        // CLI/配置侧用反引号封闭名，与 Go Enclose* 一致。
        let schemas: HashSet<String> = ca.cfg_schemas.iter().map(|s| EncloseName(s)).collect();
        let tables: HashSet<String> = ca
            .cfg_tables
            .iter()
            .map(|fq| {
                let mut parts = fq.split('.');
                let db = parts.next().unwrap();
                let tbl = parts.next().unwrap();
                EncloseDBAndTable(db, tbl)
            })
            .collect();

        VerifyDBAndTableInBackup(&ca.backup_dbs, &schemas, &tables).unwrap();

        // Case-insensitive presence check mirroring VerifyDBAndTableInBackup success path.
        // 备份名剥离临时前缀后小写比对。
        let mut backup_names: HashSet<String> = HashSet::new();
        for (db, tbls) in &ca.backup_dbs {
            let logical = db
                .strip_prefix("__TiDB_BR_Temporary_")
                .unwrap_or(db)
                .to_ascii_lowercase();
            // 库名本身也加入集合。
            backup_names.insert(logical.clone());
            // 表名小写后与逻辑库名拼接。
            for t in tbls {
                backup_names.insert(format!("{logical}.{}", t.to_ascii_lowercase()));
            }
        }
        // 逐 schema 断言存在于备份名集合。
        for s in &schemas {
            let bare = s.trim_matches('`').to_ascii_lowercase();
            assert!(
                backup_names.contains(&bare),
                "schema {s} missing in backup names {backup_names:?}"
            );
        }
        // 逐 table 断言存在于备份名集合。
        for t in &tables {
            // "`db`.`tbl`" → db.tbl lower
            // 去掉反引号再小写，与 backup_names 键格式对齐。
            let cleaned = t.replace('`', "");
            let lower = cleaned.to_ascii_lowercase();
            assert!(
                backup_names.contains(&lower),
                "table {t} missing in backup names {backup_names:?}"
            );
        }
    }
}
