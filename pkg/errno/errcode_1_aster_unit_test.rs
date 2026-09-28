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

// 错误码数值区间对齐测试。
//
// 抽样校验 `errcode` 中若干关键常量与 Go `errcode.go` 数值一致，覆盖：
// - 经典 MySQL 区间边界（`ErrErrorFirst` / `ErrErrorLast`）；
// - MySQL 8 / MariaDB 扩展码；
// - TiDB 自有码与分布式存储（PD / TiFlash / keyspace）码。

use crate::errcode::*;

/// 经典 MySQL 错误码区间与若干中间锚点与 Go 一致。
#[test]
fn mysql_error_code_ranges_match_go() {
    assert_eq!(ErrErrorFirst, 1000);
    assert_eq!(ErrHashchk, ErrErrorFirst);
    assert_eq!(ErrIndexRebuild, 1187);
    assert_eq!(ErrFtMatchingKeyNotFound, 1191);
    assert_eq!(ErrErrorLast, 1863);
}

/// MySQL 8 / MariaDB 扩展错误码与 Go 一致。
#[test]
fn mysql_8_and_mariadb_codes_match_go() {
    assert_eq!(ErrForeignKeyCascadeDepthExceeded, 3008);
    assert_eq!(ErrTableWithoutPrimaryKey, 3750);
    assert_eq!(ErrSecondPasswordCannotBeEmpty, 3878);
    assert_eq!(ErrOnlyOneDefaultPartionAllowed, 4030);
    assert_eq!(ErrSequenceInvalidTableStructure, 4141);
}

/// TiDB 自有错误码（含 DDL、资源组）与 Go 一致。
#[test]
fn tidb_owned_error_code_ranges_match_go() {
    assert_eq!(ErrMemExceedThreshold, 8001);
    assert_eq!(ErrQueryExecStopped, 8180);
    assert_eq!(ErrUnsupportedDDLOperation, 8200);
    assert_eq!(ErrDDLAutoPausedByKVDiskFull, 8276);
    assert_eq!(ErrResourceGroupExists, 8248);
    assert_eq!(ErrResourceGroupInvalidForRole, 8257);
}

/// PD / TiFlash / keyspace 相关错误码与 Go 一致。
#[test]
fn distributed_storage_codes_match_go() {
    assert_eq!(ErrPDServerTimeout, 9001);
    assert_eq!(ErrTiFlashBackfillIndex, 9014);
    assert_eq!(ErrUserPrefixMismatch, 20003);
}
