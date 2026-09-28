// Copyright 2026 AsterSQL.
// Copyright 2023-2023 PingCAP, Inc.
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

// CHECK 约束（检查约束）在 DDL 状态变更过程中的行为测试。
//
// 背景：在分布式数据库中，添加 CHECK 约束的 DDL（数据定义语言）操作不能一步完成，
// 需要经过多个 schema 状态（None -> WriteOnly -> WriteReorganization -> Public）
// 逐步推进（即在线 schema 变更，参考 F1/Online DDL 论文），
// 以保证集群中不同节点在不同 schema 版本下并发读写时数据仍然一致。
// 本测试用简化的内存表模型模拟各状态下约束的写入检查与存量数据校验逻辑。

use crate::column::TableInfo;
use crate::constraint::{ConstraintInfo, ConstraintTableState, advance_add_check_constraint};

/// schema 状态：描述约束在 DDL 演进过程中所处的阶段。
///
/// 派生 `PartialOrd`/`Ord` 使状态可以按声明顺序比较，
/// 从而用 `state >= WriteOnly` 判断“该约束是否已对写入生效”。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum SchemaState {
    /// 约束尚未生效，对读写均不可见。
    None,
    /// 只写状态：新写入的数据必须满足约束，但约束尚未对外公开。
    WriteOnly,
    /// 写重组状态：写入受约束限制，同时后台校验存量数据。
    WriteReorganization,
    /// 公开状态：约束完全生效，对所有读写可见。
    Public,
}

/// CHECK 约束的简化模型：对每行数据执行谓词检查。
#[derive(Clone)]
struct CheckConstraint {
    /// 约束名，用于生成违反约束时的错误消息。
    name: String,
    /// 当前所处的 schema 状态，决定约束是否参与写入检查。
    state: SchemaState,
    /// 是否强制执行（对应 SQL 的 ENFORCED/NOT ENFORCED），
    /// 未强制的约束即使状态生效也不会拦截写入。
    enforced: bool,
    /// 检查谓词：入参为行的两列值 (a, b)，返回 true 表示满足约束。
    check: fn(i64, i64) -> bool,
}

/// 简化的内存表：两列整数行 + 一组 CHECK 约束。
#[derive(Default)]
struct Table {
    /// 表中已有的行数据，每行为 (a, b) 两列。
    rows: Vec<(i64, i64)>,
    /// 表上定义的全部 CHECK 约束。
    constraints: Vec<CheckConstraint>,
}

impl Table {
    /// 插入一行数据，模拟 DML 写入路径上的约束检查。
    ///
    /// 只有同时满足“已强制执行”且“状态达到 WriteOnly 及以上”的约束
    /// 才会拦截不合法的写入；任一约束被违反则返回错误且不写入。
    fn insert(&mut self, row: (i64, i64)) -> Result<(), String> {
        // 查找第一个生效且被该行违反的约束：
        // state >= WriteOnly 表示约束已进入对写入可见的阶段。
        if let Some(constraint) = self.constraints.iter().find(|constraint| {
            constraint.enforced
                && constraint.state >= SchemaState::WriteOnly
                && !(constraint.check)(row.0, row.1)
        }) {
            return Err(format!(
                "[table:3819]Check constraint '{}' is violated.",
                constraint.name
            ));
        }
        self.rows.push(row);
        Ok(())
    }

    /// 校验表中全部存量行是否满足给定约束，
    /// 模拟 DDL 在 WriteReorganization 阶段对历史数据的回填检查。
    fn validate(&self, constraint: &CheckConstraint) -> Result<(), String> {
        if self.rows.iter().all(|row| (constraint.check)(row.0, row.1)) {
            Ok(())
        } else {
            Err(format!(
                "[ddl:3819]Check constraint '{}' is violated.",
                constraint.name
            ))
        }
    }

    /// 按名称删除约束，模拟 ALTER TABLE ... DROP CONSTRAINT。
    fn drop_constraint(&mut self, name: &str) {
        self.constraints
            .retain(|constraint| constraint.name != name);
    }
}

/// 公用检查谓词：要求第一列大于 10（对应 CHECK (a > 10)）。
fn greater_than_ten(a: i64, _: i64) -> bool {
    a > 10
}

/// 测试约束的添加与删除：
/// 已公开的约束应拦截非法写入；新添加的 WriteOnly 约束立即对写入生效；
/// 删除约束后其检查不再执行。
#[test]
fn test_alter_constraint_add_drop() {
    let mut table = Table {
        constraints: vec![
            CheckConstraint {
                name: "a".to_owned(),
                state: SchemaState::Public,
                enforced: true,
                check: |a, _| a > 1,
            },
            CheckConstraint {
                name: "a_b".to_owned(),
                state: SchemaState::Public,
                enforced: true,
                check: |a, b| a < b,
            },
        ],
        ..Table::default()
    };
    // (2, 3) 同时满足 a > 1 与 a < b，可以插入；(4, 3) 违反 a < b，被拒绝。
    assert_eq!(Ok(()), table.insert((2, 3)));
    assert!(table.insert((4, 3)).is_err());

    // 模拟新增一条处于 WriteOnly 状态的约束 cc：虽未公开，但已拦截新写入。
    table.constraints.push(CheckConstraint {
        name: "cc".to_owned(),
        state: SchemaState::WriteOnly,
        enforced: true,
        check: |_, b| b < 5,
    });
    assert_eq!(
        Err("[table:3819]Check constraint 'cc' is violated.".to_owned()),
        table.insert((5, 6))
    );
    // 删除约束 cc 后，约束列表中不应再存在同名约束。
    table.drop_constraint("cc");
    assert!(
        table
            .constraints
            .iter()
            .all(|constraint| constraint.name != "cc")
    );
}

/// 测试添加约束的状态变更过程中，通过故障注入绕过检查写入“脏行”的场景：
/// 存量数据校验通过后、约束真正挂载前，被注入的非法行仍会留在表中。
#[test]
fn test_alter_add_constraint_state_change_allows_injected_dirty_row() {
    let mut table = Table::default();
    table.insert((12, 0)).unwrap();
    let constraint = CheckConstraint {
        name: "c0".to_owned(),
        state: SchemaState::WriteReorganization,
        enforced: true,
        check: greater_than_ten,
    };
    // 存量数据 (12, 0) 满足 a > 10，回填校验通过。
    assert_eq!(Ok(()), table.validate(&constraint));

    // The Go failpoint temporarily clears the in-memory constraint cache.
    // 原 Go 版本通过 failpoint（故障注入点）临时清空内存中的约束缓存，
    // 使得约束尚未挂载时插入的 (1, 0) 绕过检查成为“脏行”。
    table.insert((1, 0)).unwrap();
    table.constraints.push(constraint);
    assert_eq!(vec![(12, 0), (1, 0)], table.rows);
}

/// 测试对已含“脏数据”（不满足约束的历史行）的表添加约束：
/// 存量校验应失败，约束不会被挂载到表上。
#[test]
fn test_alter_add_constraint_state_change_rejects_dirty_table() {
    let mut table = Table::default();
    table.insert((12, 0)).unwrap();
    table.insert((1, 0)).unwrap();
    let constraint = CheckConstraint {
        name: "c1".to_owned(),
        state: SchemaState::WriteOnly,
        enforced: true,
        check: greater_than_ten,
    };
    assert_eq!(
        Err("[ddl:3819]Check constraint 'c1' is violated.".to_owned()),
        table.validate(&constraint)
    );
    // 校验失败意味着 DDL 回滚，约束未被加入表。
    assert!(table.constraints.is_empty());
}

/// 测试约束在 WriteOnly、WriteReorganization、Public 三个阶段均对写入生效：
/// 任一阶段插入违反约束的行都应被拒绝，且表数据保持不变。
#[test]
fn test_constraint_is_writable_during_reorganization() {
    // 依次覆盖三个已对写入生效的状态。
    for (name, state) in [
        ("c2", SchemaState::WriteOnly),
        ("c3", SchemaState::WriteReorganization),
        ("c3", SchemaState::Public),
    ] {
        let mut table = Table {
            rows: vec![(12, 0)],
            constraints: vec![CheckConstraint {
                name: name.to_owned(),
                state,
                enforced: true,
                check: greater_than_ten,
            }],
        };
        assert_eq!(
            Err(format!(
                "[table:3819]Check constraint '{name}' is violated."
            )),
            table.insert((1, 0))
        );
        assert_eq!(vec![(12, 0)], table.rows);
    }
}

/// 测试 None 状态的约束不参与写入检查：
/// 约束尚未生效时，违反谓词的行也允许插入。
#[test]
fn test_none_state_constraint_is_not_writable() {
    let mut table = Table {
        constraints: vec![CheckConstraint {
            name: "pending".to_owned(),
            state: SchemaState::None,
            enforced: true,
            check: greater_than_ten,
        }],
        ..Table::default()
    };
    assert_eq!(Ok(()), table.insert((1, 0)));
}

/// 测试 ALTER CONSTRAINT ... ENFORCED 的状态切换：
/// 约束由未强制改为强制后，应立即开始拦截违反约束的写入。
#[test]
fn test_alter_enforced_constraint_state_change() {
    let mut table = Table {
        rows: vec![(12, 0)],
        constraints: vec![CheckConstraint {
            name: "c1".to_owned(),
            state: SchemaState::WriteOnly,
            enforced: false,
            check: greater_than_ten,
        }],
    };
    // 模拟 ALTER TABLE ... ALTER CONSTRAINT c1 ENFORCED，将约束切换为强制执行。
    table.constraints[0].enforced = true;
    assert_eq!(
        Err("[table:3819]Check constraint 'c1' is violated.".to_owned()),
        table.insert((1, 0))
    );
}

#[test]
fn rollback_add_constraint_removes_only_the_named_constraint_like_go() {
    let table = TableInfo::new(1, "t");
    let existing = ConstraintInfo {
        id: 7,
        name: "existing".to_owned(),
        table_name: "t".to_owned(),
        columns: Vec::new(),
        expression: "a > 0".to_owned(),
        enforced: true,
        in_column: false,
        state: crate::column::SchemaState::WriteOnly,
    };
    let target = ConstraintInfo {
        id: 8,
        name: "target".to_owned(),
        ..existing.clone()
    };
    let mut state = ConstraintTableState {
        max_constraint_id: 8,
        constraints: vec![existing.clone(), target.clone()],
    };
    let mut incoming = ConstraintInfo { id: 7, ..target };
    let mut schema_version = 0;

    advance_add_check_constraint(
        &table,
        &mut state,
        &mut incoming,
        true,
        &mut schema_version,
        true,
    )
    .unwrap();

    assert_eq!(
        vec!["existing"],
        state
            .constraints
            .iter()
            .map(|constraint| constraint.name.as_str())
            .collect::<Vec<_>>()
    );
}
