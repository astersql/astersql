// Copyright 2026 AsterSQL.

use super::{Context, Rhs, RuleId};
use crate::ast;
use std::any::Any;

use super::super::{
    GENERATED_MAIN_ACTION_REQUIRED, RULE_IDS_BY_REDUCTION, mlogCreateOptions, mviewCreateOptions,
};

fn item<T: Any + Clone>(rhs: &Rhs<'_>, position: usize) -> Option<T> {
    rhs.borrow(position)
        .and_then(|value| value.item.as_deref())
        .and_then(|value| value.downcast_ref::<T>())
        .cloned()
}

fn expression(rhs: &Rhs<'_>, position: usize) -> Option<ast::ExprNode> {
    rhs.borrow(position).and_then(|value| value.expr.clone())
}

pub(super) fn owns(rule_id: RuleId) -> bool {
    let name = rule_id.as_str();
    let relevant = name.starts_with("mview")
        || name.starts_with("mlog")
        || name.starts_with("creatematerializedview")
        || name.starts_with("altermaterializedview")
        || name.starts_with("dropmaterializedview")
        || name.starts_with("purgematerializedviewlog")
        || name.starts_with("cancelmaterializedviewjob")
        || name.starts_with("refreshmaterializedview")
        || name.starts_with("refreshwithasyncmodeopt")
        || name.starts_with("refreshcompletemode")
        || name.starts_with("altermlogpurgeclause");
    relevant
        && RULE_IDS_BY_REDUCTION
            .binary_search_by(|candidate| candidate.as_str().cmp(name))
            .is_ok_and(|index| GENERATED_MAIN_ACTION_REQUIRED[index])
}

pub(super) fn apply(
    rule_id: RuleId,
    mut rhs: Rhs<'_>,
    context: Context<'_>,
) -> Result<bool, isize> {
    let name = rule_id.as_str();
    let table = |position: usize| {
        rhs.borrow(position)
            .and_then(|value| value.item.as_deref())
            .and_then(|item| item.downcast_ref::<ast::TableName>())
            .cloned()
    };
    let value = |position: usize| {
        rhs.borrow(position)
            .and_then(|symbol| symbol.item.as_deref())
    };
    if name.starts_with("dropmaterializedviewstmt_") {
        context.output.statement = Some(Box::new(ast::DropMaterializedViewStmt {
            IfExists: rhs.len() == 6,
            ViewName: table(rhs.len()),
            ..Default::default()
        }));
    } else if name.starts_with("dropmaterializedviewlogstmt_") {
        context.output.statement = Some(Box::new(ast::DropMaterializedViewLogStmt {
            IfExists: value(5)
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or_default(),
            Table: table(7),
            ..Default::default()
        }));
    } else if name.starts_with("purgematerializedviewlogstmt_") {
        context.output.statement = Some(Box::new(ast::PurgeMaterializedViewLogStmt {
            Table: table(6),
            ..Default::default()
        }));
    } else if name.starts_with("cancelmaterializedviewjobstmt_") {
        context.output.statement = Some(Box::new(ast::CancelMaterializedViewJobStmt {
            Tp: if rhs.len() == 6 {
                ast::CancelMaterializedViewJobType::Refresh
            } else {
                ast::CancelMaterializedViewJobType::LogPurge
            },
            JobID: value(rhs.len())
                .and_then(|item| item.downcast_ref::<i64>())
                .copied()
                .unwrap_or_default(),
            ..Default::default()
        }));
    } else if name.starts_with("refreshmaterializedviewobserveopt") {
        context.output.item = Some(Box::new(if name.contains("dry_run") {
            ast::RefreshMaterializedViewObserveType::DryRun
        } else if name.contains("with_profile") {
            ast::RefreshMaterializedViewObserveType::Profile
        } else {
            ast::RefreshMaterializedViewObserveType::None
        }));
    } else if name.starts_with("refreshwithasyncmodeopt") {
        context.output.item = Some(Box::new(rhs.len() != 0));
    } else if name.starts_with("refreshcompletemode_") {
        context.output.item = Some(Box::new(if name.contains("out_of_place") {
            ast::RefreshMaterializedViewCompleteType::OutOfPlace
        } else if name.contains("delta_apply") {
            ast::RefreshMaterializedViewCompleteType::DeltaApply
        } else {
            ast::RefreshMaterializedViewCompleteType::InPlace
        }));
    } else if name.starts_with("refreshmaterializedviewstmt_") {
        let complete = name.ends_with("--8a718088acd8a67c");
        context.output.statement = Some(Box::new(ast::RefreshMaterializedViewStmt {
            ViewName: table(4),
            WithAsyncMode: value(5)
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or_default(),
            Type: if complete {
                ast::RefreshMaterializedViewType::Complete
            } else {
                ast::RefreshMaterializedViewType::Fast
            },
            CompleteType: if complete {
                value(7)
                    .and_then(|item| {
                        item.downcast_ref::<ast::RefreshMaterializedViewCompleteType>()
                    })
                    .copied()
                    .unwrap_or_default()
            } else {
                ast::RefreshMaterializedViewCompleteType::InPlace
            },
            ObserveType: value(8)
                .and_then(|item| item.downcast_ref::<ast::RefreshMaterializedViewObserveType>())
                .copied()
                .unwrap_or_default(),
            AsOf: if complete {
                None
            } else {
                value(7)
                    .and_then(|item| item.downcast_ref::<ast::AsOfClause>())
                    .cloned()
            },
            ..Default::default()
        }));
    } else if name.starts_with("mviewtableoptionlistopt") {
        context.output.item = if rhs.len() == 0 {
            Some(Box::new(mviewCreateOptions::default()))
        } else {
            rhs.borrow_mut(1).and_then(|value| value.item.take())
        };
    } else if name.starts_with("mviewtableoptionlist_") {
        if rhs.len() == 1 {
            context.output.item = rhs.borrow_mut(1).and_then(|value| value.item.take());
            return Ok(true);
        }
        let Some(mut left) = rhs
            .borrow_mut(1)
            .and_then(|symbol| symbol.item.take())
            .and_then(|item| item.downcast::<mviewCreateOptions>().ok())
            .map(|item| *item)
        else {
            return Ok(false);
        };
        let Some(right) = rhs
            .borrow_mut(2)
            .and_then(|symbol| symbol.item.take())
            .and_then(|item| item.downcast::<mviewCreateOptions>().ok())
            .map(|item| *item)
        else {
            return Ok(false);
        };
        if right.hasComment {
            if left.hasComment {
                context.lexer.AppendError(context.lexer.Errorf(
                    "Duplicate COMMENT specified in CREATE MATERIALIZED VIEW",
                    &[],
                ));
            }
            left.hasComment = true;
            left.comment = right.comment;
        }
        if right.hasShardRowIDBits {
            if left.hasShardRowIDBits {
                context.lexer.AppendError(context.lexer.Errorf(
                    "Duplicate SHARD_ROW_ID_BITS specified in CREATE MATERIALIZED VIEW",
                    &[],
                ));
            }
            left.hasShardRowIDBits = true;
        }
        if right.hasPreSplitRegion {
            if left.hasPreSplitRegion {
                context.lexer.AppendError(context.lexer.Errorf(
                    "Duplicate PRE_SPLIT_REGIONS specified in CREATE MATERIALIZED VIEW",
                    &[],
                ));
            }
            left.hasPreSplitRegion = true;
        }
        left.options.extend(right.options);
        context.output.item = Some(Box::new(left));
    } else if name.starts_with("mviewtableoption_") {
        let mut options = mviewCreateOptions::default();
        if name.contains("comment") {
            options.hasComment = true;
            options.comment = rhs
                .borrow(3)
                .map(|value| value.ident.clone())
                .unwrap_or_default();
        } else {
            let shard = name.contains("shard_row_id_bits");
            options.hasShardRowIDBits = shard;
            options.hasPreSplitRegion = !shard;
            options.options.push(ast::TableOption {
                Tp: if shard {
                    ast::TableOptionType::ShardRowID
                } else {
                    ast::TableOptionType::PreSplitRegion
                },
                UintValue: item::<u64>(&rhs, 3).unwrap_or_default(),
                ..Default::default()
            });
        }
        context.output.item = Some(Box::new(options));
    } else if name.starts_with("mviewrefreshclauseopt")
        || name.starts_with("mviewstartwithornextopt")
    {
        context.output.item = if rhs.len() == 0 {
            None
        } else {
            rhs.borrow_mut(1).and_then(|value| value.item.take())
        };
    } else if name.starts_with("mviewattributesopt") {
        context.output.ident = if rhs.len() == 0 {
            String::new()
        } else {
            rhs.borrow(3)
                .map(|value| value.ident.clone())
                .unwrap_or_default()
        };
    } else if name.starts_with("mviewrefreshclause_") {
        let mut refresh = item::<ast::MViewRefreshClause>(&rhs, 3).unwrap_or_default();
        refresh.Method = ast::MViewRefreshMethod::Fast;
        context.output.item = Some(Box::new(refresh));
    } else if name.starts_with("mviewstartwithornext_") {
        context.output.item = Some(Box::new(ast::MViewRefreshClause {
            StartWith: if rhs.len() == 5 {
                expression(&rhs, 3)
            } else {
                None
            },
            Next: expression(&rhs, rhs.len()),
            ..Default::default()
        }));
    } else if name.starts_with("creatematerializedviewstmt_") {
        let view_name = table(4);
        let options = rhs
            .borrow_mut(8)
            .and_then(|value| value.item.take())
            .and_then(|item| item.downcast::<mviewCreateOptions>().ok())
            .map(|item| *item)
            .unwrap_or_default();
        let select = rhs.borrow_mut(12).and_then(|value| value.statement.take());
        context.output.statement = Some(Box::new(ast::CreateMaterializedViewStmt {
            node_text: Default::default(),
            ViewName: view_name,
            Cols: item::<Vec<ast::CIStr>>(&rhs, 6).unwrap_or_default(),
            Comment: options.comment,
            Refresh: item::<ast::MViewRefreshClause>(&rhs, 9),
            Attributes: rhs
                .borrow(10)
                .map(|value| value.ident.clone())
                .unwrap_or_default(),
            Options: options.options,
            Select: select,
        }));
    } else if name.starts_with("altermaterializedviewaction_") {
        context.output.item = Some(Box::new(if name.contains("comment") {
            ast::AlterMaterializedViewAction {
                Tp: ast::AlterMaterializedViewActionType::Comment,
                Comment: rhs
                    .borrow(3)
                    .map(|value| value.ident.clone())
                    .unwrap_or_default(),
                ..Default::default()
            }
        } else if name.contains("attributes") {
            ast::AlterMaterializedViewAction {
                Tp: ast::AlterMaterializedViewActionType::Attributes,
                Attributes: rhs
                    .borrow(3)
                    .map(|value| value.ident.clone())
                    .unwrap_or_default(),
                ..Default::default()
            }
        } else {
            ast::AlterMaterializedViewAction {
                Tp: ast::AlterMaterializedViewActionType::Refresh,
                Refresh: Some(item::<ast::MViewRefreshClause>(&rhs, 2).unwrap_or_default()),
                ..Default::default()
            }
        }));
    } else if name.starts_with("altermaterializedviewactionlist_") {
        let mut actions = if rhs.len() == 1 {
            Vec::new()
        } else {
            item::<Vec<ast::AlterMaterializedViewAction>>(&rhs, 1).unwrap_or_default()
        };
        if let Some(action) = item::<ast::AlterMaterializedViewAction>(&rhs, rhs.len()) {
            actions.push(action);
        }
        context.output.item = Some(Box::new(actions));
    } else if name.starts_with("altermaterializedviewstmt_") {
        context.output.statement = Some(Box::new(ast::AlterMaterializedViewStmt {
            ViewName: table(4),
            Actions: item::<Vec<ast::AlterMaterializedViewAction>>(&rhs, 5).unwrap_or_default(),
            ..Default::default()
        }));
    } else if name.starts_with("mlogcreateoptionlistopt") {
        context.output.item = if rhs.len() == 0 {
            Some(Box::new(mlogCreateOptions::default()))
        } else {
            rhs.borrow_mut(1).and_then(|value| value.item.take())
        };
    } else if name.starts_with("mlogcreateoptionlist_") {
        if rhs.len() == 1 {
            context.output.item = rhs.borrow_mut(1).and_then(|value| value.item.take());
            return Ok(true);
        }
        let Some(mut left) = rhs
            .borrow_mut(1)
            .and_then(|value| value.item.take())
            .and_then(|item| item.downcast::<mlogCreateOptions>().ok())
            .map(|item| *item)
        else {
            return Ok(false);
        };
        let Some(right) = rhs
            .borrow_mut(2)
            .and_then(|value| value.item.take())
            .and_then(|item| item.downcast::<mlogCreateOptions>().ok())
            .map(|item| *item)
        else {
            return Ok(false);
        };
        if right.hasShardRowIDBits {
            if left.hasShardRowIDBits {
                context.lexer.AppendError(context.lexer.Errorf(
                    "Duplicate SHARD_ROW_ID_BITS specified in CREATE MATERIALIZED VIEW LOG",
                    &[],
                ));
            }
            left.hasShardRowIDBits = true;
        }
        if right.hasPreSplitRegion {
            if left.hasPreSplitRegion {
                context.lexer.AppendError(context.lexer.Errorf(
                    "Duplicate PRE_SPLIT_REGIONS specified in CREATE MATERIALIZED VIEW LOG",
                    &[],
                ));
            }
            left.hasPreSplitRegion = true;
        }
        left.options.extend(right.options);
        context.output.item = Some(Box::new(left));
    } else if name.starts_with("mlogcreateoption_") {
        let shard = name.contains("shard_row_id_bits");
        context.output.item = Some(Box::new(mlogCreateOptions {
            hasShardRowIDBits: shard,
            hasPreSplitRegion: !shard,
            options: vec![ast::TableOption {
                Tp: if shard {
                    ast::TableOptionType::ShardRowID
                } else {
                    ast::TableOptionType::PreSplitRegion
                },
                UintValue: item::<u64>(&rhs, 3).unwrap_or_default(),
                ..Default::default()
            }],
        }));
    } else if name.starts_with("mlogpurgeclauseopt")
        || name.starts_with("mlogaccumulationalertclauseopt")
    {
        context.output.item = if rhs.len() == 0 {
            None
        } else {
            rhs.borrow_mut(1).and_then(|value| value.item.take())
        };
    } else if name.starts_with("mlogstartwithopt") {
        context.output.expr = if rhs.len() == 0 {
            None
        } else {
            expression(&rhs, 3)
        };
    } else if name.starts_with("mlogpurgeclause_") {
        context.output.item = Some(Box::new(if rhs.len() == 2 {
            ast::MLogPurgeClause {
                Immediate: true,
                ..Default::default()
            }
        } else {
            ast::MLogPurgeClause {
                StartWith: expression(&rhs, 2),
                Next: expression(&rhs, 4),
                ..Default::default()
            }
        }));
    } else if name.starts_with("mlogaccumulationalertclause_") {
        context.output.item = Some(Box::new(ast::MLogAccumulationAlertClause {
            Rows: item::<i64>(&rhs, 3).unwrap_or_default(),
        }));
    } else if name.starts_with("creatematerializedviewlogstmt_") {
        let log_table = table(6);
        let options = rhs
            .borrow_mut(10)
            .and_then(|value| value.item.take())
            .and_then(|item| item.downcast::<mlogCreateOptions>().ok())
            .map(|item| *item)
            .unwrap_or_default();
        context.output.statement = Some(Box::new(ast::CreateMaterializedViewLogStmt {
            Table: log_table,
            Cols: item::<Vec<ast::CIStr>>(&rhs, 8).unwrap_or_default(),
            Options: options.options,
            Purge: item::<ast::MLogPurgeClause>(&rhs, 11),
            AccumulationAlert: item::<ast::MLogAccumulationAlertClause>(&rhs, 12),
            ..Default::default()
        }));
    } else if name.starts_with("altermlogpurgeclause_") {
        context.output.item = if rhs.len() == 1
            && rhs
                .borrow(1)
                .and_then(|value| value.item.as_deref())
                .is_some()
        {
            rhs.borrow_mut(1).and_then(|value| value.item.take())
        } else {
            Some(Box::new(ast::MLogPurgeClause::default()))
        };
    } else if name.starts_with("altermaterializedviewlogaction_") {
        context.output.item = Some(Box::new(if rhs.len() == 1 {
            ast::AlterMaterializedViewLogAction {
                Tp: ast::AlterMaterializedViewLogActionType::Purge,
                Purge: item::<ast::MLogPurgeClause>(&rhs, 1),
                ..Default::default()
            }
        } else {
            ast::AlterMaterializedViewLogAction {
                Tp: ast::AlterMaterializedViewLogActionType::AddColumn,
                Cols: item::<Vec<ast::CIStr>>(&rhs, 4).unwrap_or_default(),
                ..Default::default()
            }
        }));
    } else if name.starts_with("altermaterializedviewlogactionlist_") {
        let mut actions = if rhs.len() == 1 {
            Vec::new()
        } else {
            item::<Vec<ast::AlterMaterializedViewLogAction>>(&rhs, 1).unwrap_or_default()
        };
        if let Some(action) = item::<ast::AlterMaterializedViewLogAction>(&rhs, rhs.len()) {
            actions.push(action);
        }
        context.output.item = Some(Box::new(actions));
    } else if name.starts_with("altermaterializedviewlogstmt_") {
        context.output.statement = Some(Box::new(ast::AlterMaterializedViewLogStmt {
            Table: table(6),
            Actions: item::<Vec<ast::AlterMaterializedViewLogAction>>(&rhs, 7).unwrap_or_default(),
            ..Default::default()
        }));
    }
    Ok(true)
}
