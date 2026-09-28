// Copyright 2026 AsterSQL.

use super::super::RULE_IDS_BY_REDUCTION;
use super::*;
use crate::parsergen_grammar::Grammar;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

const DDL_NONTERMINALS: &[&str] = &[
    "AlgorithmClause",
    "AllOrPartitionNameList",
    "AlterDatabaseStmt",
    "AlterPolicyStmt",
    "AlterRangeStmt",
    "AlterTableSpec",
    "AlterTableSpecList",
    "AlterTableSpecListOpt",
    "AlterTableSpecSingleOpt",
    "AlterTableStmt",
    "AttributesOpt",
    "AutoRandomOpt",
    "BitValueType",
    "BlobType",
    "BooleanType",
    "CancelDistributionJobStmt",
    "ColumnDef",
    "ColumnFormat",
    "ColumnKeywordOpt",
    "ColumnList",
    "ColumnOption",
    "ColumnOptionList",
    "ColumnOptionListOpt",
    "ColumnPosition",
    "Constraint",
    "ConstraintColumnarIndex",
    "ConstraintElem",
    "ConstraintKeywordOpt",
    "ConstraintVectorIndex",
    "ConstraintWithColumnarIndex",
    "CreateDatabaseStmt",
    "CreateIndexStmt",
    "CreatePolicyStmt",
    "CreateTableOptionListOpt",
    "CreateTableSelectOpt",
    "CreateTableStmt",
    "CreateViewSelectOpt",
    "CreateViewStmt",
    "DatabaseOption",
    "DatabaseOptionList",
    "DatabaseOptionListOpt",
    "DateAndTimeType",
    "DefaultKwdOpt",
    "DefaultValueExpr",
    "DirectPlacementOption",
    "DistributeTableStmt",
    "DropDatabaseStmt",
    "DropIndexStmt",
    "DropPolicyStmt",
    "DropTableStmt",
    "DropViewStmt",
    "DuplicateOpt",
    "EnforcedOrNot",
    "EnforcedOrNotOpt",
    "EnforcedOrNotOrNotNullOpt",
    "FieldLen",
    "FieldOpt",
    "FieldOpts",
    "FirstAndLastPartOpt",
    "FixedPointType",
    "FlashbackDatabaseStmt",
    "FlashbackTableStmt",
    "FlashbackToNewName",
    "FlashbackToTimestampStmt",
    "FloatOpt",
    "FloatingPointType",
    "ForceOpt",
    "GeneratedAlways",
    "GlobalOrLocal",
    "GlobalOrLocalOpt",
    "IndexInvisible",
    "IndexKeyTypeOpt",
    "IndexLockAndAlgorithmOpt",
    "IndexName",
    "IndexNameAndTypeOpt",
    "IndexOption",
    "IndexOptionList",
    "IndexPartSpecification",
    "IndexPartSpecificationList",
    "IndexPartSpecificationListOpt",
    "IndexType",
    "IndexTypeName",
    "IndexTypeOpt",
    "IntegerType",
    "IntervalExpr",
    "KeyOrIndex",
    "KeyOrIndexOpt",
    "LikeTableWithOrWithoutParen",
    "LinearOpt",
    "LocationLabelList",
    "LockClause",
    "Match",
    "MatchOpt",
    "MaxValPartOpt",
    "NullPartOpt",
    "NumericType",
    "OnCommitOpt",
    "OnDelete",
    "OnDeleteUpdateOpt",
    "OnUpdate",
    "OptBinMod",
    "OptBinary",
    "OptCharset",
    "OptCharsetWithOptBinary",
    "OptCollate",
    "OptFieldLen",
    "OptTemporary",
    "OptVectorElementType",
    "OrReplace",
    "PartDefOption",
    "PartDefOptionList",
    "PartDefValuesOpt",
    "PartitionDefinition",
    "PartitionDefinitionList",
    "PartitionDefinitionListOpt",
    "PartitionIntervalOpt",
    "PartitionKeyAlgorithmOpt",
    "PartitionMethod",
    "PartitionNameList",
    "PartitionNumOpt",
    "PartitionOpt",
    "PlacementOptionList",
    "PlacementPolicyOption",
    "Precision",
    "PrimaryOpt",
    "RecoverTableStmt",
    "ReferDef",
    "ReferOpt",
    "RenameTableStmt",
    "ReorganizePartitionRuleOpt",
    "RowFormat",
    "SplitIndexList",
    "SplitIndexListOpt",
    "SplitIndexOption",
    "SplitOption",
    "SplitOptionBetween",
    "SplitRegionStmt",
    "SplitSyntaxOption",
    "StatsOptionsOpt",
    "StorageMedia",
    "StringList",
    "StringType",
    "SubPartDefinition",
    "SubPartDefinitionList",
    "SubPartDefinitionListOpt",
    "SubPartitionMethod",
    "SubPartitionNumOpt",
    "SubPartitionOpt",
    "Symbol",
    "TableElementList",
    "TableElementListOpt",
    "TableOption",
    "TableOptionList",
    "TableToTable",
    "TableToTableList",
    "TextString",
    "TextStringList",
    "TextType",
    "TruncateTableStmt",
    "UpdateIndexElem",
    "UpdateIndexesList",
    "UpdateIndexesOpt",
    "ViewAlgorithm",
    "ViewCheckOption",
    "ViewDefiner",
    "ViewFieldList",
    "ViewSQLSecurity",
    "VirtualOrStored",
    "WithClustered",
    "WithValidation",
    "WithValidationOpt",
    "Writeable",
];

fn main_grammar() -> Grammar {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("grammar")
        .join("main.astergram");
    let source = fs::read_to_string(path).expect("read main Rust grammar");
    Grammar::parse(&source).expect("parse main Rust grammar")
}

#[test]
fn ddl_rule_coverage() {
    let grammar = main_grammar();
    let ddl_lhs = DDL_NONTERMINALS.iter().copied().collect::<HashSet<_>>();
    let expected = grammar
        .productions
        .iter()
        .filter(|production| {
            production.requires_action && ddl_lhs.contains(production.lhs.as_str())
        })
        .map(|production| production.rule_id.as_str())
        .collect::<HashSet<_>>();

    assert_eq!(expected.len(), 590, "DDL RuleId inventory changed");

    let owned = RULE_IDS_BY_REDUCTION
        .iter()
        .copied()
        .filter(|rule_id| ddl::owns(*rule_id))
        .map(RuleId::as_str)
        .collect::<HashSet<_>>();

    assert_eq!(owned, expected, "DDL semantic ownership changed");
}

#[test]
fn ddl_module_has_no_numeric_fallback() {
    let source = include_str!("ddl.rs");
    assert!(!source.contains("legacy_rule_number"));
    assert!(!source.contains("apply_numeric"));
    assert!(!source.contains("rhs_index"));
}
