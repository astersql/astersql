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

//! Storage-class attributes on canonical table and partition metadata.
use astersql_expression as expression;
use astersql_expression_exprstatic as exprstatic;
use astersql_meta_model as model;
use astersql_parser_ast as ast;
use astersql_util_chunk::Row;
use astersql_util_dbterror as dbterror;
use expression::BuildContext;
use model::{StorageClassDef, StorageClassSettings};

pub type Result<T> = std::result::Result<T, String>;
fn invalid(message: impl Into<String>) -> String {
    dbterror::ErrStorageClassInvalidSpec
        .GenWithStackByArgs(&[message.into().into()])
        .to_string()
}
fn format_error(error: impl std::fmt::Display) -> String {
    dbterror::ErrEngineAttributeInvalidFormat
        .GenWithStackByArgs(&[format!("'{error}'").into()])
        .to_string()
}
fn check_tier(tier: &str) -> Result<()> {
    if matches!(tier, "STANDARD" | "IA") {
        Ok(())
    } else {
        Err(invalid(format!("invalid storage class tier: {tier}")))
    }
}
fn normalize(def: &mut StorageClassDef) -> Result<()> {
    def.Tier = def.Tier.to_uppercase();
    for name in def.NamesIn.iter_mut().flatten() {
        *name = name.to_lowercase();
    }
    for rule in def.Transitions.iter_mut().flatten() {
        rule.Tier = rule.Tier.to_uppercase();
    }
    check_tier(&def.Tier)?;
    if let Some(rules) = &def.Transitions {
        if !rules.is_empty()
            && !(def.Tier == "STANDARD"
                && rules.len() == 1
                && rules[0].Tier == "IA"
                && rules[0].TotalSeconds() > 0)
        {
            return Err(invalid(
                "only transition from 'STANDARD' to 'IA' is allowed",
            ));
        }
    }
    let scopes = usize::from(def.NamesIn.as_ref().is_some_and(|v| !v.is_empty()))
        + usize::from(def.LessThan.is_some())
        + usize::from(def.ValuesIn.as_ref().is_some_and(|v| !v.is_empty()));
    if scopes > 1 {
        return Err(invalid(
            "can not specify 'names_in', 'less_than', or 'values_in' together",
        ));
    }
    Ok(())
}
/// Local strict decoder: Go accepts null primitive fields as their zero value,
/// matches JSON field names case-insensitively, and rejects unknown fields.
struct StrictDef(StorageClassDef);
struct StrictRule(model::StorageClassTransitRule);
impl<'de> serde::Deserialize<'de> for StrictDef {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = StrictDef;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("storage class definition")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Self::Value, M::Error> {
                let mut def = StorageClassDef::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.to_lowercase().as_str() {
                        "tier" => {
                            def.Tier = map.next_value::<Option<String>>()?.unwrap_or_default()
                        }
                        "names_in" => {
                            def.NamesIn = map
                                .next_value::<Option<Vec<Option<String>>>>()?
                                .map(|v| v.into_iter().map(Option::unwrap_or_default).collect())
                        }
                        "less_than" => def.LessThan = map.next_value()?,
                        "values_in" => {
                            def.ValuesIn = map
                                .next_value::<Option<Vec<Option<String>>>>()?
                                .map(|v| v.into_iter().map(Option::unwrap_or_default).collect())
                        }
                        "transitions" => {
                            def.Transitions = map
                                .next_value::<Option<Vec<Option<StrictRule>>>>()?
                                .map(|v| {
                                    v.into_iter()
                                        .map(|r| r.map(|r| r.0).unwrap_or_default())
                                        .collect()
                                })
                        }
                        _ => {
                            return Err(serde::de::Error::unknown_field(
                                &key,
                                &["tier", "names_in", "less_than", "values_in", "transitions"],
                            ));
                        }
                    }
                }
                Ok(StrictDef(def))
            }
        }
        deserializer.deserialize_map(Visitor)
    }
}
impl<'de> serde::Deserialize<'de> for StrictRule {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = StrictRule;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("storage class transition")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Self::Value, M::Error> {
                let mut rule = model::StorageClassTransitRule::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.to_lowercase().as_str() {
                        "tier" => {
                            rule.Tier = map.next_value::<Option<String>>()?.unwrap_or_default()
                        }
                        "after_days" => {
                            rule.AfterDays = map.next_value::<Option<u64>>()?.unwrap_or_default()
                        }
                        "after_seconds" => {
                            rule.AfterSeconds = map.next_value::<Option<u64>>()?.unwrap_or_default()
                        }
                        _ => {
                            return Err(serde::de::Error::unknown_field(
                                &key,
                                &["tier", "after_days", "after_seconds"],
                            ));
                        }
                    }
                }
                Ok(StrictRule(rule))
            }
        }
        deserializer.deserialize_map(Visitor)
    }
}
pub fn BuildStorageClassSettingsFromJSON(input: Option<&str>) -> Result<StorageClassSettings> {
    let Some(input) = input else {
        return Ok(StorageClassSettings {
            Defs: Some(vec![Some(StorageClassDef {
                Tier: "STANDARD".into(),
                ..Default::default()
            })]),
        });
    };
    let value: serde_json::Value = serde_json::from_str(input).map_err(|_| {
        invalid(format!(
            "invalid storage class def: '{}'",
            input.chars().take(192).collect::<String>()
        ))
    })?;
    // Go json.Unmarshal(null, &string) leaves the zero string, then tier validation rejects it.
    let decode_error = |_| {
        invalid(format!(
            "invalid storage class def: '{}'",
            input.chars().take(192).collect::<String>()
        ))
    };
    let mut defs = match value {
        serde_json::Value::String(tier) => vec![Some(StorageClassDef {
            Tier: tier,
            ..Default::default()
        })],
        serde_json::Value::Null => vec![Some(StorageClassDef::default())],
        serde_json::Value::Array(_) => serde_json::from_str::<Vec<Option<StrictDef>>>(input)
            .map_err(decode_error)?
            .into_iter()
            .map(|d| d.map(|d| d.0))
            .collect(),
        _ => vec![Some(
            serde_json::from_str::<StrictDef>(input)
                .map_err(decode_error)?
                .0,
        )],
    };
    for def in &mut defs {
        normalize(
            def.as_mut()
                .ok_or_else(|| invalid("storage class def must not be null"))?,
        )?;
    }
    Ok(StorageClassSettings { Defs: Some(defs) })
}
fn defs(settings: &StorageClassSettings) -> impl Iterator<Item = &StorageClassDef> {
    settings.Defs.iter().flatten().flatten()
}
pub fn BuildStorageClassForTable(
    table: &mut model::TableInfo,
    settings: Option<&StorageClassSettings>,
) -> Result<()> {
    let Some(settings) = settings else {
        return Ok(());
    };
    let default = defs(settings).find(|def| def.HasNoScopeDef());
    table.StorageClassTier = default.map_or("STANDARD", |def| &def.Tier).to_owned();
    table.StorageClassTransitions = default
        .and_then(|def| def.Transitions.clone())
        .unwrap_or_default();
    astersql_util_logutil::log::BgLogger()
        .with_fields([
            astersql_util_logutil::log::LogField::I64("tableID".into(), table.ID),
            astersql_util_logutil::log::LogField::String(
                "tier".into(),
                table.StorageClassTier.clone(),
            ),
            astersql_util_logutil::log::LogField::String(
                "transitions".into(),
                serde_json::to_string(&table.StorageClassTransitions).unwrap_or_default(),
            ),
        ])
        .info("storage class: set table storage class");
    Ok(())
}
fn conflict() -> String {
    invalid("can not specify 'ENGINE_ATTRIBUTE' and 'STORAGE_CLASS' together")
}
pub fn GetEngineAttributeFromStorageClassTableOptions(
    options: &[ast::TableOption],
) -> Result<Option<String>> {
    let mut engine = false;
    let mut sugar = false;
    let mut last = None;
    for option in options {
        match option.Tp {
            ast::TableOptionType::EngineAttribute => {
                engine = true;
                last = Some(option);
            }
            ast::TableOptionType::StorageClass => {
                sugar = true;
                last = Some(option);
            }
            _ => {}
        }
    }
    if engine && sugar {
        return Err(conflict());
    }
    for option in options {
        match option.Tp {
            ast::TableOptionType::EngineAttribute => {
                let attr = model::ParseEngineAttributeFromString(&option.StrValue)
                    .map_err(format_error)?;
                let raw = attr
                    .StorageClass
                    .as_ref()
                    .ok_or_else(|| dbterror::ErrUnsupportedEngineAttribute.to_string())?;
                BuildStorageClassSettingsFromJSON(Some(raw.get()))?;
            }
            ast::TableOptionType::StorageClass => check_tier(&option.StrValue.to_uppercase())?,
            _ => {}
        }
    }
    Ok(last.map(|option| {
        if option.Tp == ast::TableOptionType::EngineAttribute {
            option.StrValue.clone()
        } else {
            serde_json::json!({"storage_class": option.StrValue.to_uppercase()}).to_string()
        }
    }))
}
pub fn CheckStorageClassConflictInAlterTableSpecs(specs: &[ast::AlterTableSpec]) -> Result<()> {
    let mut engine = false;
    let mut sugar = false;
    for spec in specs
        .iter()
        .filter(|spec| spec.Tp == ast::AlterTableType::Option)
    {
        for opt in &spec.Options {
            engine |= opt.Tp == ast::TableOptionType::EngineAttribute;
            sugar |= opt.Tp == ast::TableOptionType::StorageClass;
        }
    }
    if engine && sugar {
        Err(conflict())
    } else {
        Ok(())
    }
}
pub fn GetSimpleTableStorageClassForShowCreate(table: &model::TableInfo) -> Result<Option<String>> {
    if table.EngineAttribute.is_empty() {
        return Ok(None);
    }
    let fields: serde_json::Value =
        serde_json::from_str(&table.EngineAttribute).map_err(|e| e.to_string())?;
    let Some(fields) = fields.as_object() else {
        return Ok(None);
    };
    if fields.len() != 1 || !fields.contains_key("storage_class") {
        return Ok(None);
    }
    let settings = BuildStorageClassSettingsFromJSON(Some(&fields["storage_class"].to_string()))?;
    let values: Vec<_> = defs(&settings).collect();
    Ok(
        if values.len() == 1
            && values[0].HasNoScopeDef()
            && values[0].Transitions.as_ref().is_none_or(Vec::is_empty)
        {
            Some(values[0].Tier.clone())
        } else {
            None
        },
    )
}
pub fn get_settings(table: &model::TableInfo) -> Result<Option<StorageClassSettings>> {
    let attr =
        model::ParseEngineAttributeFromString(&table.EngineAttribute).map_err(format_error)?;
    attr.StorageClass
        .as_ref()
        .map(|raw| BuildStorageClassSettingsFromJSON(Some(raw.get())))
        .transpose()
}
pub fn handle_create(input: &str, table: &mut model::TableInfo) -> Result<()> {
    let attr = model::ParseEngineAttributeFromString(input).map_err(format_error)?;
    table.EngineAttribute = input.to_owned();
    if let Some(raw) = attr.StorageClass.as_ref() {
        let settings = BuildStorageClassSettingsFromJSON(Some(raw.get()))?;
        BuildStorageClassForTable(table, Some(&settings))?;
    }
    Ok(())
}
fn keyword(value: &str) -> bool {
    value.eq_ignore_ascii_case("MAXVALUE") || value.eq_ignore_ascii_case("DEFAULT")
}
fn unquote(value: &str) -> &str {
    value
        .strip_prefix('\'')
        .and_then(|v| v.strip_suffix('\''))
        .unwrap_or(value)
}
fn values_equal(left: &str, right: &str) -> bool {
    left == right
        || if keyword(left) || keyword(right) {
            keyword(left) && keyword(right) && left.eq_ignore_ascii_case(right)
        } else {
            unquote(left) == unquote(right)
        }
}
pub fn range_value(ctx: &dyn BuildContext, value: &str, unsigned: bool) -> Result<String> {
    if unsigned {
        if let Ok(v) = value.parse::<u64>() {
            return Ok(v.to_string());
        }
    } else if let Ok(v) = value.parse::<i64>() {
        return Ok(v.to_string());
    }
    let expr = expression::ParseSimpleExpr(ctx, value, Vec::new()).map_err(|e| e.to_string())?;
    let (v, null) = expr
        .EvalInt(ctx.GetEvalCtx(), Row::default())
        .map_err(|e| e.to_string())?;
    if null {
        return Err(invalid(format!("invalid RANGE partition value: {value}")));
    }
    Ok(if unsigned {
        (v as u64).to_string()
    } else {
        v.to_string()
    })
}
fn is_unsigned(ctx: &dyn BuildContext, table: &model::TableInfo) -> bool {
    let Some(partition) = &table.Partition else {
        return false;
    };
    if partition.Expr.is_empty() {
        return false;
    }
    expression::ParseSimpleExpr(
        ctx,
        &partition.Expr,
        vec![expression::WithTableInfo("", table)],
    )
    .is_ok_and(|expr| model::mysql::HasUnsignedFlag(expr.GetType(ctx.GetEvalCtx()).GetFlag()))
}
pub fn compare_range(
    table: &model::TableInfo,
    left: &str,
    right: &str,
) -> Result<std::cmp::Ordering> {
    use std::cmp::Ordering;
    if left.eq_ignore_ascii_case("MAXVALUE") {
        return Ok(if right.eq_ignore_ascii_case("MAXVALUE") {
            Ordering::Equal
        } else {
            Ordering::Greater
        });
    }
    if right.eq_ignore_ascii_case("MAXVALUE") {
        return Ok(Ordering::Less);
    }
    let ctx = exprstatic::NewExprContext(Vec::new());
    if let Some(partition) = &table.Partition {
        if let Some(name) = partition.Columns.first() {
            let col = table
                .Columns
                .iter()
                .find(|c| c.Name.L == name.L)
                .ok_or_else(|| {
                    invalid("'less_than' can not find RANGE COLUMNS partition column")
                })?;
            let right = if right.starts_with('\'') && right.ends_with('\'') {
                right.to_owned()
            } else {
                format!("'{}'", right.replace('\'', "''"))
            };
            let l = expression::ParseSimpleExpr(
                &ctx,
                left,
                vec![
                    expression::WithTableInfo("", table),
                    expression::WithCastExprTo(&col.FieldType),
                ],
            )
            .map_err(|_| invalid(format!("invalid 'less_than' value: {right}")))?;
            let r = expression::ParseSimpleExpr(
                &ctx,
                &right,
                vec![
                    expression::WithTableInfo("", table),
                    expression::WithCastExprTo(&col.FieldType),
                ],
            )
            .map_err(|_| invalid(format!("invalid 'less_than' value: {right}")))?;
            for (name, ordering) in [("eq", Ordering::Equal), ("gt", Ordering::Greater)] {
                let mut e = expression::NewFunctionBase(
                    &ctx,
                    name,
                    model::types::NewFieldType(model::mysql::TypeLonglong),
                    vec![l.clone(), r.clone()],
                )
                .map_err(|_| invalid(format!("invalid 'less_than' value: {right}")))?;
                if let Some(scalar) = e.as_any_mut().downcast_mut::<expression::ScalarFunction>() {
                    scalar.SetCharsetAndCollation((
                        col.GetCharset().to_owned(),
                        col.GetCollate().to_owned(),
                    ));
                }
                let (value, _) = e
                    .EvalInt(ctx.GetEvalCtx(), Row::default())
                    .map_err(|_| invalid(format!("invalid 'less_than' value: {right}")))?;
                if value > 0 {
                    return Ok(ordering);
                }
            }
            return Ok(Ordering::Less);
        }
    }
    let unsigned = is_unsigned(&ctx, table);
    let l = range_value(&ctx, left, unsigned)
        .map_err(|_| invalid(format!("invalid RANGE partition value: {left}")))?;
    let r = range_value(&ctx, right, unsigned)
        .map_err(|_| invalid(format!("invalid 'less_than' value: {right}")))?;
    if unsigned {
        Ok(l.parse::<u64>().unwrap().cmp(&r.parse::<u64>().unwrap()))
    } else {
        Ok(l.parse::<i64>().unwrap().cmp(&r.parse::<i64>().unwrap()))
    }
}
pub fn BuildStorageClassForPartitions(
    partitions: &mut [model::PartitionDefinition],
    table: &model::TableInfo,
    settings: Option<&StorageClassSettings>,
) -> Result<()> {
    let Some(settings) = settings else {
        return Ok(());
    };
    if let Some(partition) = &table.Partition {
        for def in defs(settings) {
            if !def.HasNoScopeDef()
                && [
                    model::ast::model::PartitionTypeHash,
                    model::ast::model::PartitionTypeKey,
                ]
                .contains(&partition.Type)
            {
                return Err(invalid(
                    "partition-scoped storage_class does not support HASH or KEY partitions",
                ));
            }
            if def.LessThan.is_some() {
                if partition.Type != model::ast::model::PartitionTypeRange {
                    return Err(invalid("'less_than' only supports RANGE partitions"));
                }
                if partitions.iter().any(|p| p.LessThan.len() != 1) {
                    return Err(invalid(
                        "'less_than' only supports single-column RANGE partitions",
                    ));
                }
            }
            if def.ValuesIn.as_ref().is_some_and(|v| !v.is_empty()) {
                if partition.Type != model::ast::model::PartitionTypeList {
                    return Err(invalid("'values_in' only supports LIST partitions"));
                }
                if partitions
                    .iter()
                    .flat_map(|p| &p.InValues)
                    .any(|v| v.len() != 1)
                {
                    return Err(invalid(
                        "'values_in' only supports single-column LIST partitions",
                    ));
                }
            }
        }
    }
    let default = defs(settings).find(|d| d.HasNoScopeDef());
    for partition in partitions {
        let mut matched = None;
        for def in defs(settings).filter(|d| !d.HasNoScopeDef()) {
            let names = def
                .NamesIn
                .as_ref()
                .is_some_and(|names| names.contains(&partition.Name.L));
            let less =
                if let (Some(value), [boundary]) = (&def.LessThan, partition.LessThan.as_slice()) {
                    compare_range(table, boundary, value)? != std::cmp::Ordering::Greater
                } else {
                    false
                };
            let values = def.ValuesIn.as_ref().is_some_and(|values| {
                partition.InValues.iter().any(|row| {
                    row.len() == 1 && values.iter().any(|value| values_equal(&row[0], value))
                })
            });
            if names || less || values {
                matched = Some(def);
                break;
            }
        }
        let chosen = matched.or(default);
        partition.StorageClassTier = chosen.map_or("STANDARD", |d| &d.Tier).to_owned();
        partition.StorageClassTransitions = chosen
            .and_then(|d| d.Transitions.clone())
            .unwrap_or_default();
        astersql_util_logutil::log::BgLogger()
            .with_fields([
                astersql_util_logutil::log::LogField::I64("tableID".into(), table.ID),
                astersql_util_logutil::log::LogField::String(
                    "partitionName".into(),
                    partition.Name.L.clone(),
                ),
                astersql_util_logutil::log::LogField::String(
                    "tier".into(),
                    partition.StorageClassTier.clone(),
                ),
                astersql_util_logutil::log::LogField::String(
                    "transitions".into(),
                    serde_json::to_string(&partition.StorageClassTransitions).unwrap_or_default(),
                ),
            ])
            .info("storage class: set partition storage class");
    }
    Ok(())
}
pub fn rebuild_partitions(table: &mut model::TableInfo) -> Result<()> {
    let settings = get_settings(table)?;
    let Some(partition) = &table.Partition else {
        return Ok(());
    };
    if partition.Type == model::ast::model::PartitionTypeNone {
        return Ok(());
    }
    let mut definitions = partition.Definitions.clone();
    BuildStorageClassForPartitions(&mut definitions, table, settings.as_ref())?;
    table.Partition.as_mut().unwrap().Definitions = definitions;
    Ok(())
}
/// New ADD/REORGANIZE definitions must come from the final normalized metadata.
pub fn update_checked_definitions(
    table: &mut model::TableInfo,
    part_info: &mut model::PartitionInfo,
    offset: usize,
) -> Result<()> {
    rebuild_partitions(table)?;
    let definitions = &table
        .Partition
        .as_ref()
        .ok_or("missing partition metadata")?
        .Definitions;
    let end = offset
        .checked_add(part_info.Definitions.len())
        .ok_or("partition range overflow")?;
    part_info.Definitions = definitions
        .get(offset..end)
        .ok_or_else(|| {
            format!(
                "invalid partition definition range [{offset}, {end}) for {} final definitions",
                definitions.len()
            )
        })?
        .to_vec();
    Ok(())
}

/// Normalize the canonical definitions before storage-class scopes consume them.
pub fn normalize_partition_definitions(
    ctx: &dyn BuildContext,
    table: &mut model::TableInfo,
) -> Result<()> {
    let Some(partition) = &table.Partition else {
        return Ok(());
    };
    let unsigned = is_unsigned(ctx, table);
    let columns = !partition.Columns.is_empty();
    let ty = partition.Type;
    let mut definitions = partition.Definitions.clone();
    if !columns
        && [
            model::ast::model::PartitionTypeRange,
            model::ast::model::PartitionTypeList,
        ]
        .contains(&ty)
    {
        for definition in &mut definitions {
            for value in definition
                .LessThan
                .iter_mut()
                .chain(definition.InValues.iter_mut().flatten())
            {
                if keyword(value) || value.eq_ignore_ascii_case("NULL") {
                    continue;
                }
                *value = range_value(ctx, value, unsigned)?;
            }
        }
    }
    table.Partition.as_mut().unwrap().Definitions = definitions;
    Ok(())
}
pub fn CheckAndUpdateAddedPartitionDefinitions(
    ctx: &dyn BuildContext,
    table: &model::TableInfo,
    added: &mut model::PartitionInfo,
    offset: usize,
) -> Result<()> {
    let mut final_table = table.clone();
    let mut final_info = added.clone();
    let mut definitions = table
        .Partition
        .as_ref()
        .ok_or("missing original partition metadata")?
        .Definitions
        .clone();
    definitions.extend(added.Definitions.clone());
    final_info.Definitions = definitions;
    final_table.Partition = Some(final_info);
    normalize_partition_definitions(ctx, &mut final_table)?;
    check_final_definitions(&final_table)?;
    update_checked_definitions(&mut final_table, added, offset)
}
pub fn check_final_definitions(table: &model::TableInfo) -> Result<()> {
    let Some(partition) = &table.Partition else {
        return Ok(());
    };
    let mut names = std::collections::HashSet::new();
    for definition in &partition.Definitions {
        if !names.insert(&definition.Name.L) {
            return Err(format!("duplicate partition name '{}'", definition.Name.O));
        }
    }
    if partition.Type == model::ast::model::PartitionTypeRange && partition.Columns.len() <= 1 {
        for adjacent in partition.Definitions.windows(2) {
            if let ([left], [right]) = (
                adjacent[0].LessThan.as_slice(),
                adjacent[1].LessThan.as_slice(),
            ) {
                if compare_range(table, left, right)? != std::cmp::Ordering::Less {
                    return Err(
                        "VALUES LESS THAN value must be strictly increasing for each partition"
                            .into(),
                    );
                }
            }
        }
    }
    Ok(())
}

/// Check metadata using the same static expression context as Go partition helpers.
pub fn normalize_checked_partitions(table: &mut model::TableInfo) -> Result<()> {
    normalize_partition_definitions(&exprstatic::NewExprContext(Vec::new()), table)
}
