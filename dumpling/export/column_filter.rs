// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc. Licensed under Apache-2.0.

use astersql_util_table_filter as column_filter_lib;

#[derive(Clone, Debug, Default)]
pub struct columnFilterConfig {
    pub Filters: Vec<Arc<columnFilterRule>>,
}

#[derive(Debug)]
pub struct columnFilterRule {
    table_filter: Box<dyn column_filter_lib::Filter>,
    column_rules: column_filter_lib::ColumnFilterRules,
}

impl columnFilterConfig {
    pub fn applyToColumns(
        &self,
        database: &str,
        table: &str,
        source: &[String],
    ) -> Result<(Vec<String>, Vec<usize>)> {
        let matched: Vec<_> = self
            .Filters
            .iter()
            .rev()
            .filter(|r| r.table_filter.MatchTable(database, table))
            .collect();
        if matched.is_empty() {
            return Ok((source.to_vec(), (0..source.len()).collect()));
        }
        let mut columns = Vec::new();
        let mut indexes = Vec::new();
        for (i, column) in source.iter().enumerate() {
            if matched
                .iter()
                .find_map(|r| r.column_rules.match_rule(column))
                .unwrap_or(false)
            {
                columns.push(column.clone());
                indexes.push(i);
            }
        }
        if columns.is_empty() {
            return Err(errors_new(format!(
                "column filter selects no writable columns from table `{}`.`{}`",
                escapeString(database),
                escapeString(table)
            )));
        }
        Ok((columns, indexes))
    }
}

fn columnFilterFromToml(
    value: toml::Value,
    case_sensitive: bool,
    option: &str,
) -> Result<columnFilterConfig> {
    let root = value
        .as_table()
        .ok_or_else(|| errors_new(format!("failed to parse --{option}: expected TOML table")))?;
    let key = "filters";
    let mut unknown = Vec::new();
    for k in root.keys().filter(|k| k.as_str() != key) {
        unknown.push(k.clone());
    }
    let entries: Vec<&toml::Value> = match root.get(key) {
        None => vec![],
        Some(v) => v
            .as_array()
            .ok_or_else(|| {
                errors_new(format!(
                    "failed to parse --{option}: filters must be an array"
                ))
            })?
            .iter()
            .collect(),
    };
    // Decode every rule before reporting unknown keys or compiling matchers.
    let mut decoded = Vec::with_capacity(entries.len());
    for entry in &entries {
        let rule = entry.as_table().ok_or_else(|| {
            errors_new(format!("failed to parse --{option}: expected filter table"))
        })?;
        let strings = |name: &str| -> Result<Vec<String>> {
            match rule.get(name) {
                None => Ok(vec![]),
                Some(v) => v
                    .as_array()
                    .ok_or_else(|| {
                        errors_new(format!(
                            "failed to parse --{option}: {name} must be an array"
                        ))
                    })?
                    .iter()
                    .map(|v| {
                        v.as_str().map(str::to_owned).ok_or_else(|| {
                            errors_new(format!(
                                "failed to parse --{option}: {name} must contain strings"
                            ))
                        })
                    })
                    .collect(),
            }
        };
        decoded.push((strings("matcher")?, strings("columns")?));
    }
    for entry in &entries {
        let rule = entry.as_table().ok_or_else(|| {
            errors_new(format!("failed to parse --{option}: expected filter table"))
        })?;
        for k in rule
            .keys()
            .filter(|k| k.as_str() != "matcher" && k.as_str() != "columns")
        {
            unknown.push(format!("{key}.{k}"));
        }
    }
    if !unknown.is_empty() {
        return Err(errors_new(format!(
            "--{option} contains unknown TOML keys: {}",
            unknown.join(", ")
        )));
    }
    if entries.is_empty() {
        return Err(errors_new(format!(
            "--{option} requires at least one column filter"
        )));
    }
    let mut config = columnFilterConfig::default();
    for (i, (matcher, columns)) in decoded.into_iter().enumerate() {
        if matcher.is_empty() {
            return Err(errors_new(format!(
                "--{option} filter {i} requires at least one matcher"
            )));
        }
        if columns.is_empty() {
            return Err(errors_new(format!(
                "--{option} filter {i} requires at least one column rule"
            )));
        }
        let mut table_filter = column_filter_lib::Parse(matcher).map_err(|e| {
            errors_new(format!(
                "failed to parse --{option} filter {i} matcher: {e}"
            ))
        })?;
        if !case_sensitive {
            table_filter = column_filter_lib::CaseInsensitive(table_filter);
        }
        let column_rules = column_filter_lib::ParseColumnFilterRules(columns).map_err(|e| {
            errors_new(format!(
                "failed to parse --{option} filter {i} columns: {e}"
            ))
        })?;
        config.Filters.push(Arc::new(columnFilterRule {
            table_filter,
            column_rules,
        }));
    }
    Ok(config)
}

pub fn parseColumnFilterArgs(args: &[String], case_sensitive: bool) -> Result<columnFilterConfig> {
    let mut entries = Vec::new();
    for (i, arg) in args.iter().enumerate() {
        let value = format!("filter = {arg}")
            .parse::<toml::Value>()
            .map_err(|e| errors_new(format!("failed to parse --column-filter {i}: {e}")))?;
        let root = value.as_table().unwrap();
        let mut unknown: Vec<_> = root
            .keys()
            .filter(|k| k.as_str() != "filter")
            .cloned()
            .collect();
        let rule = root.get("filter").unwrap();
        let table = rule.as_table().ok_or_else(|| {
            errors_new(format!(
                "failed to parse --column-filter {i}: expected filter table"
            ))
        })?;
        for key in table
            .keys()
            .filter(|k| k.as_str() != "matcher" && k.as_str() != "columns")
        {
            unknown.push(format!("filter.{key}"));
        }
        // TOML decoding checks field types before compiling any matcher, as in Go.
        for name in ["matcher", "columns"] {
            if let Some(value) = table.get(name) {
                let array = value.as_array().ok_or_else(|| {
                    errors_new(format!(
                        "failed to parse --column-filter {i}: {name} must be an array"
                    ))
                })?;
                if array.iter().any(|v| v.as_str().is_none()) {
                    return Err(errors_new(format!(
                        "failed to parse --column-filter {i}: {name} must contain strings"
                    )));
                }
            }
        }
        if !unknown.is_empty() {
            return Err(errors_new(format!(
                "--column-filter contains unknown TOML keys: {}",
                unknown.join(", ")
            )));
        }
        entries.push(rule.clone());
    }
    let mut root = toml::map::Map::new();
    root.insert("filters".into(), toml::Value::Array(entries));
    columnFilterFromToml(toml::Value::Table(root), case_sensitive, "column-filter")
}

pub fn parseColumnFilterConfig(path: &str, case_sensitive: bool) -> Result<columnFilterConfig> {
    let content = std::fs::read(path)
        .map_err(|e| errors_new(format!("failed to read --column-filter-file {path}: {e}")))?;
    let content = std::str::from_utf8(&content)
        .map_err(|e| errors_new(format!("failed to parse --column-filter-file {path}: {e}")))?;
    let value = content
        .parse::<toml::Value>()
        .map_err(|e| errors_new(format!("failed to parse --column-filter-file {path}: {e}")))?;
    columnFilterFromToml(value, case_sensitive, "column-filter-file")
}

pub fn validateColumnFilterOptions(conf: &Config, option: &str) -> Result<()> {
    if !conf.SQL.is_empty() {
        return Err(errors_new(format!(
            "can't specify both --sql and --{option} at the same time"
        )));
    }
    // The no-schemas restriction was removed by dfc06738174f7e15c383a76a536568a4005d2adc.
    Ok(())
}

impl Config {
    pub fn parseColumnFilterOptions(
        &mut self,
        args: &[String],
        path: &str,
        case_sensitive: bool,
    ) -> Result<()> {
        if !args.is_empty() && !path.trim().is_empty() {
            return Err(errors_new(
                "can't specify both --column-filter and --column-filter-file at the same time",
            ));
        }
        if !args.is_empty() {
            validateColumnFilterOptions(self, "column-filter")?;
            self.columnFilter = parseColumnFilterArgs(args, case_sensitive)?;
        } else if !path.trim().is_empty() {
            validateColumnFilterOptions(self, "column-filter-file")?;
            self.columnFilter = parseColumnFilterConfig(path, case_sensitive)?;
        }
        Ok(())
    }
}
