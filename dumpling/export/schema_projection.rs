// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc. Licensed under Apache-2.0.

use crate::{Result, errors_new, escapeString};
use ast::Node;
use schema_parser::{Parser, ast};
use std::collections::{HashMap, HashSet};

pub(crate) struct ProjectedTableSchema {
    pub create_table: Box<ast::CreateTableStmt>,
    pub retained_columns: HashSet<String>,
}

pub(crate) type ProjectedTableSchemas = HashMap<(String, String), ProjectedTableSchema>;

pub(crate) fn parse_table_schema(parser: &mut Parser, sql: &str) -> Result<ProjectedTableSchema> {
    let stmt = parser.ParseOneStmt(sql, "", "").map_err(|err| {
        errors_new(format!(
            "failed to parse CREATE TABLE for column projection: {err}"
        ))
    })?;
    let create_table = stmt
        .into_any()
        .downcast::<ast::CreateTableStmt>()
        .map_err(|_| {
            errors_new("expected CREATE TABLE for column projection, got another statement")
        })?;
    let retained_columns = create_table
        .Cols
        .iter()
        .map(|column| column.Name.Name.L.clone())
        .collect();
    Ok(ProjectedTableSchema {
        create_table,
        retained_columns,
    })
}

// Use the parser's column visitor, so function names, quoted strings and qualified
// column references follow the same AST dependency semantics as Go's DDL visitor.
fn expression_columns(expr: Option<&ast::ExprNode>) -> HashSet<String> {
    struct Columns(HashSet<String>);
    impl ast::Visitor for Columns {
        fn enter(&mut self, _: &dyn ast::Node) -> bool {
            false
        }
        fn leave(&mut self, _: &dyn ast::Node) -> bool {
            true
        }
        fn enter_column_name(&mut self, column: &ast::ColumnName) -> bool {
            self.0.insert(column.Name.L.clone());
            false
        }
    }
    let mut visitor = Columns(HashSet::new());
    if let Some(expr) = expr {
        expr.accept(&mut visitor);
    }
    visitor.0
}

fn uses_only_retained_columns(expr: Option<&ast::ExprNode>, retained: &HashSet<String>) -> bool {
    expression_columns(expr).is_subset(retained)
}

fn keep_constraint(constraint: &ast::Constraint, retained: &HashSet<String>) -> bool {
    constraint.Keys.iter().all(|key| {
        key.Column
            .as_ref()
            .is_none_or(|column| retained.contains(&column.Name.L))
            && uses_only_retained_columns(key.Expr.as_ref(), retained)
    }) && uses_only_retained_columns(constraint.Expr.as_ref(), retained)
        && uses_only_retained_columns(
            constraint
                .Option
                .as_ref()
                .and_then(|option| option.Condition.as_ref()),
            retained,
        )
}

fn index_constraint(constraint: &ast::Constraint) -> bool {
    matches!(
        constraint.Tp,
        ast::ConstraintType::PrimaryKey | ast::ConstraintType::Index | ast::ConstraintType::Unique
    )
}

fn index_covers_columns(keys: &[ast::IndexPartSpecification], columns: &[String]) -> bool {
    keys.len() >= columns.len()
        && keys.iter().zip(columns).all(|(key, column)| {
            key.Length <= 0
                && key
                    .Column
                    .as_ref()
                    .is_some_and(|name| &name.Name.L == column)
        })
}

fn has_parent_index(table: &ast::CreateTableStmt, columns: &[String]) -> bool {
    table.Cols.iter().any(|column| {
        columns.len() == 1
            && column.Name.Name.L == columns[0]
            && column.Options.iter().any(|option| {
                matches!(
                    option.Tp,
                    ast::ColumnOptionType::PrimaryKey | ast::ColumnOptionType::UniqueKey
                )
            })
    }) || table.Constraints.iter().any(|constraint| {
        index_constraint(constraint) && index_covers_columns(&constraint.Keys, columns)
    })
}

fn has_clustered_primary_key(table: &ast::CreateTableStmt, name: &str) -> bool {
    for column in &table.Cols {
        if column.Name.Name.L == name {
            if let Some(option) = column
                .Options
                .iter()
                .find(|option| option.Tp == ast::ColumnOptionType::PrimaryKey)
            {
                return option.PrimaryKeyTp != ast::PrimaryKeyType::NonClustered;
            }
        }
    }
    for constraint in &table.Constraints {
        if constraint.Tp == ast::ConstraintType::PrimaryKey
            && constraint.Keys.iter().any(|key| {
                key.Column
                    .as_ref()
                    .is_some_and(|column| column.Name.L == name)
            })
        {
            return constraint
                .Option
                .as_ref()
                .is_none_or(|option| option.PrimaryKeyTp != ast::PrimaryKeyType::NonClustered);
        }
    }
    false
}

pub(crate) fn build_projected_table_schema(
    parser: &mut Parser,
    sql: &str,
    selected: &[String],
) -> Result<ProjectedTableSchema> {
    project_table_schema(parse_table_schema(parser, sql)?, selected)
}

pub(crate) fn project_table_schema(
    mut schema: ProjectedTableSchema,
    selected: &[String],
) -> Result<ProjectedTableSchema> {
    let table = &mut schema.create_table;
    let mut partition_columns = HashSet::new();
    if let Some(partition) = &table.Partition {
        for method in std::iter::once(&partition.PartitionMethod).chain(partition.Sub.iter()) {
            partition_columns.extend(
                method
                    .ColumnNames
                    .iter()
                    .map(|column| column.Name.L.clone()),
            );
            partition_columns.extend(expression_columns(method.Expr.as_ref()));
            if method.Tp == ast::PartitionType::Key && method.ColumnNames.is_empty() {
                return Err(errors_new(
                    "PARTITION BY KEY() is not supported with column filtering",
                ));
            }
        }
    }
    let mut retained: HashSet<String> = selected
        .iter()
        .map(|column| column.to_lowercase())
        .collect();
    for column in &table.Cols {
        if column.Options.iter().any(|option| {
            option.Tp == ast::ColumnOptionType::Generated
                && uses_only_retained_columns(option.Expr.as_ref(), &retained)
        }) {
            retained.insert(column.Name.Name.L.clone());
        }
    }
    let mut columns = Vec::with_capacity(table.Cols.len());
    for mut column in std::mem::take(&mut table.Cols) {
        if !retained.contains(&column.Name.Name.L) {
            continue;
        }
        let mut options = Vec::with_capacity(column.Options.len());
        for option in std::mem::take(&mut column.Options) {
            if option.Tp == ast::ColumnOptionType::Check
                && !uses_only_retained_columns(option.Expr.as_ref(), &retained)
            {
                continue;
            }
            if matches!(
                option.Tp,
                ast::ColumnOptionType::DefaultValue | ast::ColumnOptionType::OnUpdate
            ) && !uses_only_retained_columns(option.Expr.as_ref(), &retained)
            {
                return Err(errors_new(format!(
                    "column `{}` expression references a removed column",
                    column.Name.Name.O
                )));
            }
            options.push(option);
        }
        column.Options = options;
        columns.push(column);
    }
    table.Cols = columns;
    table
        .Constraints
        .retain(|constraint| keep_constraint(constraint, &retained));
    for column in &table.Cols {
        if column
            .Options
            .iter()
            .any(|option| option.Tp == ast::ColumnOptionType::AutoRandom)
            && !has_clustered_primary_key(table, &column.Name.Name.L)
        {
            return Err(errors_new(
                "auto_random is only supported on the tables with clustered primary key",
            ));
        }
    }
    for column in &table.Cols {
        if column
            .Options
            .iter()
            .any(|option| option.Tp == ast::ColumnOptionType::AutoIncrement)
            && !has_parent_index(table, &[column.Name.Name.L.clone()])
        {
            return Err(errors_new(format!(
                "auto_increment column `{}` must be defined as a key",
                column.Name.Name.O
            )));
        }
    }
    if !partition_columns.is_subset(&retained) {
        return Err(errors_new(
            "partition definition references a removed column",
        ));
    }
    for option in &table.Options {
        if option.Tp == ast::TableOptionType::TTL {
            if let Some(column) = &option.ColumnName {
                if !retained.contains(&column.Name.L) {
                    return Err(errors_new(format!(
                        "TTL definition references removed column `{}`",
                        column.Name.O
                    )));
                }
            }
        }
    }
    schema.retained_columns = retained;
    Ok(schema)
}

pub(crate) fn lookup_schema<'a>(
    schemas: &'a ProjectedTableSchemas,
    database: &str,
    table: &str,
) -> Result<Option<&'a ProjectedTableSchema>> {
    if let Some(schema) = schemas.get(&(database.to_owned(), table.to_owned())) {
        return Ok(Some(schema));
    }
    // Use Unicode simple folding rather than lowercasing: final sigma and long-s
    // share Go EqualFold classes, while sharp-s does not expand to two letters.
    let folded = |name: &str| {
        regex::RegexBuilder::new(&format!("\\A{}\\z", regex::escape(name)))
            .case_insensitive(true)
            .build()
            .expect("escaped table name")
    };
    let database_pattern = folded(database);
    let table_pattern = folded(table);
    let mut matched = None;
    for ((db, name), schema) in schemas {
        if database_pattern.is_match(db) && table_pattern.is_match(name) {
            if matched.is_some() {
                return Err(errors_new(format!(
                    "foreign key reference `{}`.`{}` is ambiguous under case-insensitive matching",
                    escapeString(database),
                    escapeString(table)
                )));
            }
            matched = Some(schema);
        }
    }
    Ok(matched)
}

fn validate_foreign_key_parent(
    reference: Option<&ast::ReferenceDef>,
    child_database: &str,
    schemas: &ProjectedTableSchemas,
) -> Result<()> {
    let Some(reference) = reference else {
        return Ok(());
    };
    let database = if reference.Table.Schema.O.is_empty() {
        child_database
    } else {
        &reference.Table.Schema.O
    };
    let table = &reference.Table.Name.O;
    let Some(parent) = lookup_schema(schemas, database, table)? else {
        return Ok(());
    };
    let mut columns = Vec::with_capacity(reference.IndexPartSpecifications.len());
    for key in &reference.IndexPartSpecifications {
        // SHOW CREATE TABLE emits column index parts for foreign key references.
        let column = key
            .Column
            .as_ref()
            .ok_or_else(|| errors_new("foreign key reference is not a column index part"))?;
        if !parent.retained_columns.contains(&column.Name.L) {
            return Err(errors_new(format!(
                "foreign key references removed column `{}`.`{}`.`{}`",
                escapeString(database),
                escapeString(table),
                escapeString(&column.Name.O)
            )));
        }
        columns.push(column.Name.L.clone());
    }
    if !has_parent_index(&parent.create_table, &columns) {
        return Err(errors_new(format!(
            "foreign key referenced columns are not indexed in table `{}`.`{}`",
            escapeString(database),
            escapeString(table)
        )));
    }
    Ok(())
}

pub(crate) fn validate_foreign_key_parents(
    child_database: &str,
    child: &ProjectedTableSchema,
    schemas: &ProjectedTableSchemas,
) -> Result<()> {
    for column in &child.create_table.Cols {
        for option in &column.Options {
            if option.Tp == ast::ColumnOptionType::Reference {
                validate_foreign_key_parent(option.Refer.as_ref(), child_database, schemas)?;
            }
        }
    }
    for constraint in &child.create_table.Constraints {
        validate_foreign_key_parent(constraint.Refer.as_ref(), child_database, schemas)?;
    }
    Ok(())
}

#[path = "schema_projection_restore.rs"]
mod restore;

pub(crate) fn restore_projected_schema(table: &ast::CreateTableStmt) -> Result<String> {
    restore::restore(table)
        .map_err(|err| errors_new(format!("failed to restore projected CREATE TABLE: {err}")))
}

pub(crate) fn new_schema_parser(params: &HashMap<String, String>) -> Result<Box<Parser>> {
    let mut parser = schema_parser::New();
    if let Some(value) = params.get("sql_mode") {
        let sql_mode =
            schema_mysql::r#const::GetSQLMode(&schema_mysql::r#const::FormatSQLModeStr(value))
                .map_err(|err| errors_new(format!("failed to parse session sql_mode: {err}")))?;
        parser.SetSQLMode(sql_mode);
    }
    Ok(parser)
}
