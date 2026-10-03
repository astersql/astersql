// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc. Licensed under Apache-2.0.

//! CREATE TABLE restoration needed exclusively by Dumpling's schema projection.
//! The AST owns every selection and dependency decision. No source-SQL deletion
//! or fallback to an unmodified statement is used to generate projected output.

use super::*;
use ast::{ColumnOptionType as CO, ConstraintType as CT, TableOptionType as TO};

fn name(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}
fn string(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
}
fn special(feature: &str, body: &str) -> String {
    if feature.is_empty() {
        format!("/*T! {body} */")
    } else {
        format!("/*T![{feature}] {body} */")
    }
}
fn column_name(column: &ast::ColumnName) -> String {
    [&column.Schema.O, &column.Table.O, &column.Name.O]
        .into_iter()
        .filter(|part| !part.is_empty())
        .map(|part| name(part))
        .collect::<Vec<_>>()
        .join(".")
}
fn table_name(table: &ast::TableName) -> String {
    if table.Schema.O.is_empty() {
        name(&table.Name.O)
    } else {
        format!("{}.{}", name(&table.Schema.O), name(&table.Name.O))
    }
}
fn literal(input: &ast::ExprNode) -> Option<String> {
    match &input.Kind {
        ast::ExprKind::Value(value) => Some(match &value.Datum {
            ast::ValueDatum::String(value_text) => {
                let charset = value.Type.GetCharset();
                let prefix = if charset.is_empty() {
                    String::new()
                } else {
                    format!("_{}", charset.to_uppercase())
                };
                format!("{prefix}{}", string(value_text))
            }
            ast::ValueDatum::Bytes(value) => string(&String::from_utf8_lossy(value)),
            ast::ValueDatum::BitLiteral(value) => format!(
                "b'{}'",
                value
                    .iter()
                    .map(|byte| format!("{byte:08b}"))
                    .collect::<String>()
            ),
            ast::ValueDatum::HexLiteral(value) => format!(
                "x'{}'",
                value
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            ),
            _ => value.text(),
        }),
        ast::ExprKind::IntroducedValue { Value, Charset, .. } => {
            Some(format!("_{}{}", Charset.to_uppercase(), string(Value)))
        }
        _ => None,
    }
}

// Reuse the existing expression renderer for expression structure. Its generic
// ValueExpr rendering classifies strings by their text, so typed literals are
// rendered separately. The adapter replaces whole literal AST nodes with unique
// literal tokens, then expands only those complete quoted tokens. Tokens cannot
// collide with source text or each other; column and function ASTs are untouched.
fn expression(input: &ast::ExprNode) -> Result<String> {
    struct Literals {
        source: String,
        replacements: Vec<(String, String)>,
    }
    impl ast::ExprNodeVisitor for Literals {
        fn Enter(&mut self, input: &ast::ExprNode) -> (ast::ExprNode, bool) {
            if let Some(sql) = literal(input) {
                let mut token = format!(
                    "__astersql_projection_literal_{}__",
                    self.replacements.len()
                );
                while self.source.contains(&token) {
                    token.push('_');
                }
                self.replacements
                    .push((format!("_UTF8MB4'{}'", token), sql));
                (
                    ast::ExprNode::StringValue(token, "utf8mb4", "utf8mb4_bin"),
                    true,
                )
            } else {
                (input.clone(), false)
            }
        }
        fn Leave(&mut self, input: &ast::ExprNode) -> (ast::ExprNode, bool) {
            (input.clone(), true)
        }
    }
    struct Source(String);
    impl ast::Visitor for Source {
        fn enter(&mut self, node: &dyn ast::Node) -> bool {
            self.0.push_str(&node.Text());
            if let Some(expr) = node.as_any().downcast_ref::<ast::ExprNode>() {
                if let Some(value) = literal(expr) {
                    self.0.push_str(&value);
                }
            }
            false
        }
        fn leave(&mut self, _: &dyn ast::Node) -> bool {
            true
        }
    }
    let mut source = Source(String::new());
    input.accept(&mut source);
    let mut literals = Literals {
        source: source.0,
        replacements: vec![],
    };
    let (adapted, _) = input.Accept(&mut literals);
    let mut rendered = ast::sql_restore::restore_expr(&adapted).map_err(errors_new)?;
    // Expand simultaneously so one original literal cannot introduce a token
    // that would be expanded by a later substitution.
    if !literals.replacements.is_empty() {
        let pattern = literals
            .replacements
            .iter()
            .map(|(token, _)| regex::escape(token))
            .collect::<Vec<_>>()
            .join("|");
        let regex = regex::Regex::new(&pattern).expect("escaped literal tokens");
        rendered = regex
            .replace_all(&rendered, |captures: &regex::Captures<'_>| {
                literals
                    .replacements
                    .iter()
                    .find(|(token, _)| token == &captures[0])
                    .unwrap()
                    .1
                    .clone()
            })
            .into_owned();
    }
    Ok(rendered)
}
fn optional_expression(expr: Option<&ast::ExprNode>) -> Result<String> {
    expression(
        expr.ok_or_else(|| {
            errors_new("missing expression while restoring projected CREATE TABLE")
        })?,
    )
}
fn expressions(values: &[ast::ExprNode]) -> Result<String> {
    Ok(values
        .iter()
        .map(expression)
        .collect::<Result<Vec<_>>>()?
        .join(","))
}
fn index_part(key: &ast::IndexPartSpecification) -> Result<String> {
    let mut sql = if let Some(expr) = &key.Expr {
        format!("({})", expression(expr)?)
    } else {
        let mut value = column_name(
            key.Column
                .as_ref()
                .ok_or_else(|| errors_new("missing index column"))?,
        );
        if key.Length > 0 {
            value.push_str(&format!("({})", key.Length));
        }
        value
    };
    if key.Desc {
        sql.push_str(" DESC");
    }
    Ok(sql)
}
fn reference(reference: &ast::ReferenceDef) -> Result<String> {
    let keys = reference
        .IndexPartSpecifications
        .iter()
        .map(index_part)
        .collect::<Result<Vec<_>>>()?
        .join(", ");
    let mut sql = format!("REFERENCES {}({keys})", table_name(&reference.Table));
    match reference.Match {
        ast::MatchType::None => {}
        ast::MatchType::Full => sql.push_str(" MATCH FULL"),
        ast::MatchType::Partial => sql.push_str(" MATCH PARTIAL"),
        ast::MatchType::Simple => sql.push_str(" MATCH SIMPLE"),
    }
    let action = |value| match value {
        ast::ReferOptionType::None => "",
        ast::ReferOptionType::Restrict => "RESTRICT",
        ast::ReferOptionType::Cascade => "CASCADE",
        ast::ReferOptionType::SetNull => "SET NULL",
        ast::ReferOptionType::NoAction => "NO ACTION",
        ast::ReferOptionType::SetDefault => "SET DEFAULT",
    };
    for (kind, value) in [
        ("DELETE", reference.OnDelete.ReferOpt),
        ("UPDATE", reference.OnUpdate.ReferOpt),
    ] {
        if value != ast::ReferOptionType::None {
            sql.push_str(&format!(" ON {kind} {}", action(value)));
        }
    }
    Ok(sql)
}
fn column_option(option: &ast::ColumnOption) -> Result<String> {
    Ok(match option.Tp {
        CO::None => String::new(),
        CO::PrimaryKey => {
            let mut sql = "PRIMARY KEY".to_string();
            let clustered = match option.PrimaryKeyTp {
                ast::PrimaryKeyType::Default => "",
                ast::PrimaryKeyType::Clustered => "CLUSTERED",
                ast::PrimaryKeyType::NonClustered => "NONCLUSTERED",
            };
            if !clustered.is_empty() {
                sql.push_str(&format!(" {}", special("clustered_index", clustered)));
            }
            if option.StrValue == "Global" {
                sql.push_str(" GLOBAL");
            }
            sql
        }
        CO::NotNull => "NOT NULL".into(),
        CO::Null => "NULL".into(),
        CO::AutoIncrement => "AUTO_INCREMENT".into(),
        CO::UniqueKey => if option.StrValue == "Global" {
            "UNIQUE KEY GLOBAL"
        } else {
            "UNIQUE KEY"
        }
        .into(),
        CO::DefaultValue => {
            let expr = option
                .Expr
                .as_ref()
                .ok_or_else(|| errors_new("missing default expression"))?;
            let value = expression(expr)?;
            let wrap = matches!(&expr.Kind, ast::ExprKind::Column(_))
                || matches!(&expr.Kind, ast::ExprKind::Function { FnName, .. } if FnName.L != "current_timestamp");
            format!(
                "DEFAULT {}",
                if wrap { format!("({value})") } else { value }
            )
        }
        CO::OnUpdate => format!("ON UPDATE {}", optional_expression(option.Expr.as_ref())?),
        CO::Fulltext => {
            return Err(errors_new(
                "TiDB Parser ignore the `ColumnOptionFulltext` type now",
            ));
        }
        CO::Comment => {
            let expr = option
                .Expr
                .as_ref()
                .ok_or_else(|| errors_new("missing column comment"))?;
            let value = match &expr.Kind {
                ast::ExprKind::Value(v) if matches!(v.Datum, ast::ValueDatum::String(_)) => {
                    string(v.as_str())
                }
                _ => expression(expr)?,
            };
            format!("COMMENT {value}")
        }
        CO::Generated => format!(
            "GENERATED ALWAYS AS({}) {}",
            optional_expression(option.Expr.as_ref())?,
            if option.Stored { "STORED" } else { "VIRTUAL" }
        ),
        CO::Reference => reference(
            option
                .Refer
                .as_ref()
                .ok_or_else(|| errors_new("missing inline foreign key reference"))?,
        )?,
        CO::Collate => {
            if option.StrValue.is_empty() {
                return Err(errors_new("Empty ColumnOption COLLATE"));
            }
            format!("COLLATE {}", option.StrValue)
        }
        CO::Check => format!(
            "{}CHECK({}) {}",
            if option.ConstraintName.is_empty() {
                String::new()
            } else {
                format!("CONSTRAINT {} ", name(&option.ConstraintName))
            },
            optional_expression(option.Expr.as_ref())?,
            if option.Enforced {
                "ENFORCED"
            } else {
                "NOT ENFORCED"
            }
        ),
        CO::ColumnFormat => format!("COLUMN_FORMAT {}", option.StrValue.to_uppercase()),
        CO::Storage => format!("STORAGE {}", option.StrValue.to_uppercase()),
        CO::AutoRandom => {
            let bits = &option.AutoRandOpt;
            let args = if bits.ShardBits == -1 {
                String::new()
            } else if bits.RangeBits == -1 {
                format!("({})", bits.ShardBits)
            } else {
                format!("({}, {})", bits.ShardBits, bits.RangeBits)
            };
            special("auto_rand", &format!("AUTO_RANDOM{args}"))
        }
        CO::SecondaryEngineAttribute => {
            format!("SECONDARY_ENGINE_ATTRIBUTE = {}", string(&option.StrValue))
        }
        CO::MariaDBRowStart => "GENERATED ALWAYS AS ROW START".into(),
        CO::MariaDBRowEnd => "GENERATED ALWAYS AS ROW END".into(),
    })
}
fn split_option(option: &ast::SplitOption) -> Result<String> {
    if option.ValueLists.is_empty() {
        Ok(format!(
            "BETWEEN ({}) AND ({}) REGIONS {}",
            expressions(&option.Lower)?,
            expressions(&option.Upper)?,
            option.Num
        ))
    } else {
        Ok(format!(
            "BY {}",
            option
                .ValueLists
                .iter()
                .map(|values| Ok(format!("({})", expressions(values)?)))
                .collect::<Result<Vec<_>>>()?
                .join(",")
        ))
    }
}
fn index_option(option: &ast::IndexOption) -> Result<String> {
    let mut base = option.clone();
    base.Condition = None;
    base.SplitOpt = None;
    base.PrimaryKeyTp = ast::PrimaryKeyType::Default;
    base.Comment.clear();
    base.SecondaryEngineAttr.clear();
    let mut sql = base.restore_with_special_comments(true);
    let clustered = match option.PrimaryKeyTp {
        ast::PrimaryKeyType::Default => "",
        ast::PrimaryKeyType::Clustered => "CLUSTERED",
        ast::PrimaryKeyType::NonClustered => "NONCLUSTERED",
    };
    if !clustered.is_empty() {
        sql = format!(
            "{}{}{}",
            special("clustered_index", clustered),
            if sql.is_empty() { "" } else { " " },
            sql
        );
    }
    for (key, value) in [
        ("COMMENT", &option.Comment),
        ("SECONDARY_ENGINE_ATTRIBUTE =", &option.SecondaryEngineAttr),
    ] {
        if !value.is_empty() {
            if !sql.is_empty() {
                sql.push(' ');
            }
            sql.push_str(&format!("{key} {}", string(value)));
        }
    }
    if let Some(split) = &option.SplitOpt {
        let value = if split.Num != 0 && split.Lower.is_empty() {
            split.Num.to_string()
        } else {
            format!("({})", split_option(split)?)
        };
        if !sql.is_empty() {
            sql.push(' ');
        }
        sql.push_str(&special(
            "pre_split",
            &format!("PRE_SPLIT_REGIONS = {value}"),
        ));
    }
    if let Some(condition) = &option.Condition {
        if !sql.is_empty() {
            sql.push(' ');
        }
        sql.push_str(&format!("WHERE {}", expression(condition)?));
    }
    Ok(sql)
}
fn constraint(constraint: &ast::Constraint) -> Result<String> {
    if constraint.Tp == CT::None {
        return Ok(String::new());
    }
    if constraint.Tp == CT::Check {
        return Ok(format!(
            "{}CHECK({}) {}",
            if constraint.Name.is_empty() {
                String::new()
            } else {
                format!("CONSTRAINT {} ", name(&constraint.Name))
            },
            optional_expression(constraint.Expr.as_ref())?,
            if constraint.Enforced {
                "ENFORCED"
            } else {
                "NOT ENFORCED"
            }
        ));
    }
    let mut sql = match constraint.Tp {
        CT::PrimaryKey => "PRIMARY KEY".into(),
        CT::Index => "INDEX".into(),
        CT::Unique => "UNIQUE KEY".into(),
        CT::Fulltext => "FULLTEXT".into(),
        CT::Vector => "VECTOR INDEX".into(),
        CT::Columnar => "COLUMNAR INDEX".into(),
        CT::ForeignKey => format!(
            "CONSTRAINT {}FOREIGN KEY ",
            if constraint.Name.is_empty() {
                String::new()
            } else {
                format!("{} ", name(&constraint.Name))
            }
        ),
        CT::None | CT::Check => unreachable!(),
    };
    if constraint.IfNotExists {
        sql.push_str(&special("", " IF NOT EXISTS"));
    }
    if constraint.Tp != CT::ForeignKey && (!constraint.Name.is_empty() || constraint.IsEmptyIndex) {
        sql.push_str(&format!(" {}", name(&constraint.Name)));
    }
    sql.push_str(&format!(
        "({})",
        constraint
            .Keys
            .iter()
            .map(index_part)
            .collect::<Result<Vec<_>>>()?
            .join(", ")
    ));
    if let Some(refer) = &constraint.Refer {
        sql.push_str(&format!(" {}", reference(refer)?));
    }
    if let Some(option) = &constraint.Option {
        if !option.is_empty() || option.AddColumnarReplicaOnDemand > 0 {
            sql.push_str(&format!(" {}", index_option(option)?));
        }
    }
    Ok(sql)
}
fn time_unit(unit: ast::TimeUnitType) -> &'static str {
    use ast::TimeUnitType::*;
    match unit {
        Invalid => "",
        Microsecond => "MICROSECOND",
        Second => "SECOND",
        Minute => "MINUTE",
        Hour => "HOUR",
        Day => "DAY",
        Week => "WEEK",
        Month => "MONTH",
        Quarter => "QUARTER",
        Year => "YEAR",
        SecondMicrosecond => "SECOND_MICROSECOND",
        MinuteMicrosecond => "MINUTE_MICROSECOND",
        MinuteSecond => "MINUTE_SECOND",
        HourMicrosecond => "HOUR_MICROSECOND",
        HourSecond => "HOUR_SECOND",
        HourMinute => "HOUR_MINUTE",
        DayMicrosecond => "DAY_MICROSECOND",
        DaySecond => "DAY_SECOND",
        DayMinute => "DAY_MINUTE",
        DayHour => "DAY_HOUR",
        YearMonth => "YEAR_MONTH",
    }
}
fn table_option(option: &ast::TableOption) -> Result<String> {
    let number = || option.UintValue.to_string();
    let default_number = || {
        if option.Default {
            "DEFAULT".to_string()
        } else {
            number()
        }
    };
    Ok(match option.Tp {
        TO::Engine => format!(
            "ENGINE = {}",
            if option.StrValue.is_empty() {
                "''"
            } else {
                &option.StrValue
            }
        ),
        TO::Charset => format!(
            "{}CHARACTER SET {}{}",
            if option.UintValue == 1 {
                "CONVERT TO "
            } else {
                "DEFAULT "
            },
            if option.UintValue == 0 { "= " } else { "" },
            if option.Default {
                "DEFAULT".into()
            } else {
                option.StrValue.to_uppercase()
            }
        ),
        TO::Collate => format!("DEFAULT COLLATE = {}", option.StrValue.to_uppercase()),
        TO::AutoIncrement => format!(
            "{}AUTO_INCREMENT = {}",
            if option.BoolValue {
                format!("{} ", special("force_inc", "FORCE"))
            } else {
                String::new()
            },
            number()
        ),
        TO::AutoIdCache => special("auto_id_cache", &format!("AUTO_ID_CACHE = {}", number())),
        TO::AutoRandomBase => format!(
            "{}{}",
            if option.BoolValue {
                format!("{} ", special("force_inc", "FORCE"))
            } else {
                String::new()
            },
            special(
                "auto_rand_base",
                &format!("AUTO_RANDOM_BASE = {}", number())
            )
        ),
        TO::Comment
        | TO::Compression
        | TO::Connection
        | TO::Password
        | TO::EngineAttribute
        | TO::StorageClass
        | TO::DataDirectory
        | TO::IndexDirectory
        | TO::SecondaryEngine
        | TO::SecondaryEngineAttribute
        | TO::Encryption => {
            let keyword = match option.Tp {
                TO::Comment => "COMMENT",
                TO::Compression => "COMPRESSION",
                TO::Connection => "CONNECTION",
                TO::Password => "PASSWORD",
                TO::EngineAttribute => "ENGINE_ATTRIBUTE",
                TO::StorageClass => "STORAGE_CLASS",
                TO::DataDirectory => "DATA DIRECTORY",
                TO::IndexDirectory => "INDEX DIRECTORY",
                TO::SecondaryEngine => "SECONDARY_ENGINE",
                TO::SecondaryEngineAttribute => "SECONDARY_ENGINE_ATTRIBUTE",
                TO::Encryption => "ENCRYPTION",
                _ => unreachable!(),
            };
            format!("{keyword} = {}", string(&option.StrValue))
        }
        TO::AvgRowLength
        | TO::CheckSum
        | TO::KeyBlockSize
        | TO::MaxRows
        | TO::MinRows
        | TO::DelayKeyWrite
        | TO::Nodegroup
        | TO::TableCheckSum
        | TO::PageChecksum
        | TO::PageCompressed
        | TO::PageCompressionLevel
        | TO::Transactional
        | TO::Sequence => {
            let keyword = match option.Tp {
                TO::AvgRowLength => "AVG_ROW_LENGTH",
                TO::CheckSum => "CHECKSUM",
                TO::KeyBlockSize => "KEY_BLOCK_SIZE",
                TO::MaxRows => "MAX_ROWS",
                TO::MinRows => "MIN_ROWS",
                TO::DelayKeyWrite => "DELAY_KEY_WRITE",
                TO::Nodegroup => "NODEGROUP",
                TO::TableCheckSum => "TABLE_CHECKSUM",
                TO::PageChecksum => "PAGE_CHECKSUM",
                TO::PageCompressed => "PAGE_COMPRESSED",
                TO::PageCompressionLevel => "PAGE_COMPRESSION_LEVEL",
                TO::Transactional => "TRANSACTIONAL",
                TO::Sequence => "SEQUENCE",
                _ => unreachable!(),
            };
            format!("{keyword} = {}", number())
        }
        TO::RowFormat => {
            let formats = [
                "DEFAULT",
                "DYNAMIC",
                "FIXED",
                "COMPRESSED",
                "REDUNDANT",
                "COMPACT",
                "TOKUDB_DEFAULT",
                "TOKUDB_FAST",
                "TOKUDB_SMALL",
                "TOKUDB_ZLIB",
                "TOKUDB_QUICKLZ",
                "TOKUDB_LZMA",
                "TOKUDB_SNAPPY",
                "TOKUDB_UNCOMPRESSED",
                "TOKUDB_ZSTD",
            ];
            let format = option
                .UintValue
                .checked_sub(1)
                .and_then(|index| formats.get(index as usize))
                .ok_or_else(|| {
                    errors_new(format!(
                        "invalid TableOption: TableOptionRowFormat: {}",
                        option.UintValue
                    ))
                })?;
            format!("ROW_FORMAT = {format}")
        }
        TO::StatsPersistent => {
            "STATS_PERSISTENT = DEFAULT /* TableOptionStatsPersistent is not supported */ ".into()
        }
        TO::PackKeys => "PACK_KEYS = DEFAULT /* TableOptionPackKeys is not supported */ ".into(),
        TO::StatsAutoRecalc | TO::StatsSamplePages | TO::StatsBuckets | TO::StatsTopN => {
            let keyword = match option.Tp {
                TO::StatsAutoRecalc => "STATS_AUTO_RECALC",
                TO::StatsSamplePages => "STATS_SAMPLE_PAGES",
                TO::StatsBuckets => "STATS_BUCKETS",
                TO::StatsTopN => "STATS_TOPN",
                _ => unreachable!(),
            };
            format!("{keyword} = {}", default_number())
        }
        TO::ShardRowID => special("", &format!("SHARD_ROW_ID_BITS = {}", number())),
        TO::PreSplitRegion => special("", &format!("PRE_SPLIT_REGIONS = {}", number())),
        TO::Tablespace => format!("TABLESPACE = {}", name(&option.StrValue)),
        TO::StorageMedia => format!("STORAGE {}", option.StrValue.to_uppercase()),
        TO::SecondaryEngineNull => "SECONDARY_ENGINE = NULL".into(),
        TO::InsertMethod => format!("INSERT_METHOD = {}", option.StrValue.to_uppercase()),
        TO::Union => format!(
            "UNION = ({})",
            option
                .TableNames
                .iter()
                .map(table_name)
                .collect::<Vec<_>>()
                .join(",")
        ),
        TO::StatsSampleRate => format!(
            "STATS_SAMPLE_RATE = {}",
            if option.Default {
                "DEFAULT".into()
            } else {
                optional_expression(option.Value.as_ref())?
            }
        ),
        TO::StatsColsChoice | TO::StatsColList => format!(
            "{} = {}",
            if option.Tp == TO::StatsColsChoice {
                "STATS_COL_CHOICE"
            } else {
                "STATS_COL_LIST"
            },
            if option.Default {
                "DEFAULT".into()
            } else {
                string(&option.StrValue)
            }
        ),
        TO::TTL => special(
            "ttl",
            &format!(
                "TTL = {} + INTERVAL {} {}",
                column_name(
                    option
                        .ColumnName
                        .as_ref()
                        .ok_or_else(|| errors_new("missing TTL column"))?
                ),
                optional_expression(option.Value.as_ref())?,
                time_unit(
                    option
                        .TimeUnitValue
                        .ok_or_else(|| errors_new("missing TTL time unit"))?
                )
            ),
        ),
        TO::TTLEnable => special(
            "ttl",
            &format!(
                "TTL_ENABLE = {}",
                string(if option.BoolValue { "ON" } else { "OFF" })
            ),
        ),
        TO::TTLJobInterval => special(
            "ttl",
            &format!("TTL_JOB_INTERVAL = {}", string(&option.StrValue)),
        ),
        TO::AutoextendSize => format!("AUTOEXTEND_SIZE = {}", option.StrValue),
        TO::IetfQuotes => format!("IETF_QUOTES = {}", option.StrValue),
        TO::Affinity => special(
            "affinity",
            &format!("AFFINITY = {}", string(&option.StrValue)),
        ),
        TO::StartTransaction => "START TRANSACTION".into(),
        TO::Policy => special(
            "placement",
            &format!("PLACEMENT POLICY = {}", name(&option.StrValue)),
        ),
        // Go TableOption.Restore rejects unsupported placement-option variants;
        // preserve that error rather than silently dropping an AST option.
        _ => return Err(errors_new(format!("invalid TableOption: {:?}", option.Tp))),
    })
}
fn partition_method(method: &ast::PartitionMethod) -> Result<String> {
    let keyword = match method.Tp {
        ast::PartitionType::None => "",
        ast::PartitionType::Key => "KEY",
        ast::PartitionType::Hash => "HASH",
        ast::PartitionType::Range => "RANGE",
        ast::PartitionType::List => "LIST",
        ast::PartitionType::SystemTime => "SYSTEM_TIME",
    };
    let mut sql = format!("{}{keyword}", if method.Linear { "LINEAR " } else { "" });
    if method.KeyAlgorithm.Type != 0 {
        sql.push_str(&format!(" ALGORITHM = {}", method.KeyAlgorithm.Type));
    }
    if method.Tp == ast::PartitionType::SystemTime {
        if let Some(expr) = &method.Expr {
            if method.Unit != ast::TimeUnitType::Invalid {
                sql.push_str(&format!(
                    " INTERVAL {} {}",
                    expression(expr)?,
                    time_unit(method.Unit)
                ));
            }
        }
        if method.Limit > 0 {
            sql.push_str(&format!(" LIMIT {}", method.Limit));
        }
    } else if let Some(expr) = &method.Expr {
        sql.push_str(&format!(" ({})", expression(expr)?));
    } else {
        if matches!(
            method.Tp,
            ast::PartitionType::Range | ast::PartitionType::List
        ) {
            sql.push_str(" COLUMNS");
        }
        sql.push_str(&format!(
            " ({})",
            method
                .ColumnNames
                .iter()
                .map(column_name)
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    if let Some(interval) = &method.Interval {
        sql.push_str(&format!(
            " INTERVAL ({}{})",
            optional_expression(interval.IntervalExpr.Expr.as_ref())?,
            if interval.IntervalExpr.TimeUnit == ast::TimeUnitType::Invalid {
                String::new()
            } else {
                format!(" {}", time_unit(interval.IntervalExpr.TimeUnit))
            }
        ));
        for (keyword, expr) in [
            ("FIRST", interval.FirstRangeEnd.as_ref()),
            ("LAST", interval.LastRangeEnd.as_ref()),
        ] {
            if let Some(expr) = expr {
                sql.push_str(&format!(
                    " {keyword} PARTITION LESS THAN ({})",
                    expression(expr)?
                ));
            }
        }
        if interval.NullPart {
            sql.push_str(" NULL PARTITION");
        }
        if interval.MaxValPart {
            sql.push_str(" MAXVALUE PARTITION");
        }
    }
    Ok(sql)
}
fn partition_definition(partition: &ast::PartitionDefinition) -> Result<String> {
    let mut sql = format!("PARTITION {}", name(&partition.Name.O));
    match &partition.Clause {
        ast::PartitionDefinitionClause::None => {}
        ast::PartitionDefinitionClause::LessThan(values) => {
            sql.push_str(&format!(" VALUES LESS THAN ({})", expressions(values)?))
        }
        ast::PartitionDefinitionClause::In(rows) => {
            if rows.is_empty()
                || rows.len() == 1
                    && rows[0].len() == 1
                    && matches!(
                        rows[0][0].Kind,
                        ast::ExprKind::DefaultValue | ast::ExprKind::NamedDefault(_)
                    )
            {
                sql.push_str(" DEFAULT");
            } else {
                sql.push_str(&format!(
                    " VALUES IN ({})",
                    rows.iter()
                        .map(|row| {
                            let values = expressions(row)?;
                            Ok(if row.len() == 1 {
                                values
                            } else {
                                format!("({values})")
                            })
                        })
                        .collect::<Result<Vec<_>>>()?
                        .join(", ")
                ));
            }
        }
        ast::PartitionDefinitionClause::History { Current } => {
            sql.push_str(if *Current { " CURRENT" } else { " HISTORY" })
        }
    }
    for option in &partition.Options {
        sql.push_str(&format!(" {}", table_option(option)?));
    }
    if !partition.Sub.is_empty() {
        sql.push_str(" (");
        sql.push_str(
            &partition
                .Sub
                .iter()
                .map(|sub| {
                    let mut value = format!("SUBPARTITION {}", name(&sub.Name.O));
                    for option in &sub.Options {
                        value.push_str(&format!(" {}", table_option(option)?));
                    }
                    Ok(value)
                })
                .collect::<Result<Vec<_>>>()?
                .join(","),
        );
        sql.push(')');
    }
    Ok(sql)
}
fn partition_options(partition: &ast::PartitionOptions) -> Result<String> {
    let mut sql = format!(
        "PARTITION BY {}",
        partition_method(&partition.PartitionMethod)?
    );
    if partition.PartitionMethod.Num > 0 && partition.Definitions.is_empty() {
        sql.push_str(&format!(" PARTITIONS {}", partition.PartitionMethod.Num));
    }
    if let Some(sub) = &partition.Sub {
        sql.push_str(&format!(" SUBPARTITION BY {}", partition_method(sub)?));
        if sub.Num > 0 {
            sql.push_str(&format!(" SUBPARTITIONS {}", sub.Num));
        }
    }
    if !partition.Definitions.is_empty() {
        sql.push_str(&format!(
            " ({})",
            partition
                .Definitions
                .iter()
                .map(partition_definition)
                .collect::<Result<Vec<_>>>()?
                .join(",")
        ));
    }
    if !partition.UpdateIndexes.is_empty() {
        sql.push_str(&format!(
            " UPDATE INDEXES ({})",
            partition
                .UpdateIndexes
                .iter()
                .map(|index| format!(
                    "{} {}",
                    name(&index.Name),
                    if index.Option.as_ref().is_some_and(|option| option.Global) {
                        "GLOBAL"
                    } else {
                        "LOCAL"
                    }
                ))
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    Ok(sql)
}

pub(super) fn restore(table: &ast::CreateTableStmt) -> Result<String> {
    let mut sql = match table.TemporaryKeyword {
        ast::TemporaryKeyword::None => "CREATE TABLE ",
        ast::TemporaryKeyword::Local => "CREATE TEMPORARY TABLE ",
        ast::TemporaryKeyword::Global => "CREATE GLOBAL TEMPORARY TABLE ",
    }
    .to_string();
    if table.IfNotExists {
        sql.push_str("IF NOT EXISTS ");
    }
    sql.push_str(&table_name(&table.Table));
    if let Some(reference) = &table.ReferTable {
        sql.push_str(&format!(" LIKE {}", table_name(reference)));
    }
    let mut definitions = Vec::with_capacity(table.Cols.len() + table.Constraints.len());
    for column in &table.Cols {
        let mut bytes = Vec::new();
        column
            .Tp
            .Restore(&mut schema_format::NewRestoreCtx(
                schema_format::DefaultRestoreFlags
                    | schema_format::RestoreTiDBSpecialComment
                    | schema_format::RestoreStringEscapeBackslash,
                &mut bytes,
            ))
            .map_err(|err| errors_new(err.to_string()))?;
        let field_type = String::from_utf8(bytes).map_err(|err| errors_new(err.to_string()))?;
        let mut value = format!("{} {field_type}", column_name(&column.Name));
        for option in &column.Options {
            value.push(' ');
            value.push_str(&column_option(option)?);
        }
        definitions.push(value);
    }
    for value in &table.Constraints {
        definitions.push(constraint(value)?);
    }
    if !definitions.is_empty() {
        sql.push_str(&format!(" ({})", definitions.join(",")));
    }
    for option in &table.Options {
        sql.push_str(&format!(" {}", table_option(option)?));
    }
    if let Some(partition) = &table.Partition {
        sql.push_str(&format!(" {}", partition_options(partition)?));
    }
    for split in &table.SplitIndex {
        sql.push_str(" SPLIT ");
        if !split.TableLevel {
            if split.PrimaryKey {
                sql.push_str("PRIMARY KEY ");
            } else {
                sql.push_str(&format!("INDEX {} ", name(&split.IndexName.O)));
            }
        }
        sql.push_str(&split_option(&split.SplitOpt)?);
    }
    if let Some(select) = &table.Select {
        sql.push_str(match table.OnDuplicate {
            ast::OnDuplicateKeyHandlingType::Error => " AS ",
            ast::OnDuplicateKeyHandlingType::Ignore => " IGNORE AS ",
            ast::OnDuplicateKeyHandlingType::Replace => " REPLACE AS ",
        });
        sql.push_str(&ast::sql_restore::restore_node(select.as_ref()).map_err(errors_new)?);
    }
    if table.TemporaryKeyword == ast::TemporaryKeyword::Global {
        sql.push_str(if table.OnCommitDelete {
            " ON COMMIT DELETE ROWS"
        } else {
            " ON COMMIT PRESERVE ROWS"
        });
    }
    Ok(sql)
}
