// Copyright 2026 AsterSQL.

//! Partition routing for the canonical catalog model used by SQL execution.

use std::cmp::Ordering;
use std::collections::HashMap;

use model_dependency as model;

/// A borrowed routing state shared by SQL row, index and scan paths. The
/// caller supplies the table's new-collation mode at construction.
pub struct CanonicalPartitionedTable<'a> {
    table: &'a model::TableInfo,
    use_new_collation: bool,
}

impl<'a> CanonicalPartitionedTable<'a> {
    pub fn new(table: &'a model::TableInfo, use_new_collation: bool) -> Self {
        Self {
            table,
            use_new_collation,
        }
    }

    pub fn locate(
        &self,
        row: &HashMap<String, Option<String>>,
        expression_value: impl Fn(&str, &HashMap<String, Option<String>>) -> Option<i64>,
    ) -> i64 {
        let Some(partition) = self.table.GetPartitionInfo() else {
            return self.table.ID;
        };
        if partition.Definitions.is_empty() {
            return self.table.ID;
        }
        let expression = if partition.Expr.is_empty() {
            partition
                .Columns
                .first()
                .map(|column| column.L.clone())
                .unwrap_or_default()
        } else {
            partition.Expr.replace('`', "").to_lowercase()
        };
        let value = expression_value(&expression, row).unwrap_or_default();
        let definition = match partition.Type {
            model::ast::model::PartitionTypeHash => {
                let position = value.rem_euclid(partition.Definitions.len() as i64) as usize;
                partition.Definitions.get(position)
            }
            model::ast::model::PartitionTypeKey => {
                let mut hasher = crc32fast::Hasher::new();
                for name in &partition.Columns {
                    match row.get(&name.L).and_then(Option::as_deref) {
                        None => hasher.update(&[0]),
                        Some(value) => {
                            let collate = self
                                .table
                                .Columns
                                .iter()
                                .find(|column| column.Name.L == name.L)
                                .map(|column| column.FieldType.GetCollate())
                                .unwrap_or("binary");
                            hasher.update(
                                &collate_dependency::GetCollatorWithCollate(
                                    self.use_new_collation,
                                    collate,
                                )
                                .Key(value),
                            );
                        }
                    }
                }
                partition
                    .Definitions
                    .get(hasher.finalize() as usize % partition.Definitions.len())
            }
            model::ast::model::PartitionTypeRange => {
                partition.Definitions.iter().find(|definition| {
                    if partition.Columns.is_empty() {
                        definition.LessThan.first().is_none_or(|upper| {
                            upper.eq_ignore_ascii_case("maxvalue")
                                || value < upper.parse::<i64>().unwrap_or(i64::MAX)
                        })
                    } else {
                        self.range_columns_row_is_below(
                            row,
                            &partition.Columns,
                            &definition.LessThan,
                        )
                    }
                })
            }
            model::ast::model::PartitionTypeList => {
                partition.Definitions.iter().find(|definition| {
                    definition.InValues.iter().any(|values| {
                        if partition.Columns.is_empty() {
                            values.len() == 1
                                && self.list_value_matches(
                                    expression_value(&expression, row)
                                        .map(|value| value.to_string())
                                        .as_deref(),
                                    &values[0],
                                    None,
                                )
                        } else {
                            values.len() == partition.Columns.len()
                                && partition.Columns.iter().zip(values).all(
                                    |(column, configured)| {
                                        self.list_value_matches(
                                            row.get(&column.L).and_then(Option::as_deref),
                                            configured,
                                            self.table
                                                .Columns
                                                .iter()
                                                .find(|info| info.Name.L == column.L)
                                                .map(|info| info.FieldType.GetCollate()),
                                        )
                                    },
                                )
                        }
                    })
                })
            }
            _ => None,
        };
        definition.map_or(self.table.ID, |definition| definition.ID)
    }

    pub fn list_value_matches(
        &self,
        actual: Option<&str>,
        configured: &str,
        collation: Option<&str>,
    ) -> bool {
        let configured = configured.trim().trim_matches(['\'', '"']);
        if configured.eq_ignore_ascii_case("null") {
            actual.is_none()
        } else {
            actual.is_some_and(|actual| {
                collation.map_or_else(
                    || actual == configured,
                    |collation| {
                        let collator = collate_dependency::GetCollatorWithCollate(
                            self.use_new_collation,
                            collation,
                        );
                        collator.Key(actual) == collator.Key(configured)
                    },
                )
            })
        }
    }

    pub fn range_columns_row_is_below(
        &self,
        row: &HashMap<String, Option<String>>,
        columns: &[model::ast::CIStr],
        upper_bound: &[String],
    ) -> bool {
        for (column, upper) in columns.iter().zip(upper_bound) {
            if upper.eq_ignore_ascii_case("maxvalue") {
                return true;
            }
            let actual = row.get(&column.L).and_then(Option::as_deref);
            let upper = upper.trim().trim_matches(['\'', '"']);
            let ordering = match actual {
                None => Ordering::Less,
                Some(actual) => match (actual.parse::<i128>(), upper.parse::<i128>()) {
                    (Ok(actual), Ok(upper)) => actual.cmp(&upper),
                    _ => self
                        .table
                        .Columns
                        .iter()
                        .find(|info| info.Name.L == column.L)
                        .map_or_else(
                            || actual.cmp(upper),
                            |info| {
                                let collator = collate_dependency::GetCollatorWithCollate(
                                    self.use_new_collation,
                                    info.FieldType.GetCollate(),
                                );
                                collator.Key(actual).cmp(&collator.Key(upper))
                            },
                        ),
                },
            };
            if ordering != Ordering::Equal {
                return ordering == Ordering::Less;
            }
        }
        false
    }
}
