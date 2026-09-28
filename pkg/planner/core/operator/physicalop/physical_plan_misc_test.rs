// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

use std::sync::Arc;

use parser_ast::NewCIStr;
use types::metadata::{FieldName, NameSlice};

use crate::{PhysPlanPartInfo, emptyPartitionInfoSize};

fn empty_part_info(column_names: NameSlice) -> PhysPlanPartInfo {
    PhysPlanPartInfo {
        PruningConds: Vec::new(),
        PartitionNames: Vec::new(),
        Columns: Vec::new(),
        ColumnNames: column_names,
    }
}

#[test]
fn ordinary_clone_deep_clones_column_names_like_go() {
    let field_name = Arc::new(FieldName {
        ColName: NewCIStr("answer"),
        ..FieldName::default()
    });
    let info = empty_part_info(NameSlice(vec![Some(Arc::clone(&field_name))]));

    let cloned = info.Clone();
    let cloned_name = cloned.ColumnNames.0[0].as_ref().unwrap();

    assert_eq!(cloned_name.ColName, field_name.ColName);
    assert!(!Arc::ptr_eq(cloned_name, &field_name));
}

#[test]
fn plan_cache_clone_shares_column_names_like_go() {
    let field_name = Arc::new(FieldName {
        ColName: NewCIStr("answer"),
        ..FieldName::default()
    });
    let info = empty_part_info(NameSlice(vec![Some(Arc::clone(&field_name))]));

    let cloned = info.CloneForPlanCache();
    let cloned_name = cloned.ColumnNames.0[0].as_ref().unwrap();

    assert!(Arc::ptr_eq(cloned_name, &field_name));
}

#[test]
fn memory_usage_includes_cistr_storage_like_go() {
    let partition_name = NewCIStr("PartitionA");
    let expected = emptyPartitionInfoSize + partition_name.memory_usage();
    let mut info = empty_part_info(NameSlice(Vec::new()));
    info.PartitionNames.push(partition_name);

    assert_eq!(info.MemoryUsage(), expected);
}
