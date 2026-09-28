// Copyright 2026 AsterSQL.

//! Owned, thread-safe expression snapshots used at the instance plan-cache boundary.

use crate::*;

/// Error returned when an expression cannot be represented without retaining runtime state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CacheSnapshotError(String);

impl std::fmt::Display for CacheSnapshotError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for CacheSnapshotError {}

fn unsupported(message: impl Into<String>) -> CacheSnapshotError {
    CacheSnapshotError(message.into())
}

#[derive(Clone)]
struct CachedCollation {
    coercibility: Option<Coercibility>,
    repertoire: Repertoire,
    charset: String,
    collation: String,
    explicit_charset: bool,
}

impl CachedCollation {
    fn capture(value: &dyn CollationInfo) -> Self {
        let (charset, collation) = value.CharsetAndCollation();
        Self {
            coercibility: value.HasCoercibility().then(|| value.Coercibility()),
            repertoire: value.Repertoire(),
            charset,
            collation,
            explicit_charset: value.IsExplicitCharset(),
        }
    }

    fn restore(&self, value: &mut dyn CollationInfo) {
        value.SetCharsetAndCollation(self.charset.clone(), self.collation.clone());
        if let Some(coercibility) = self.coercibility {
            value.SetCoercibility(coercibility);
        }
        value.SetRepertoire(self.repertoire);
        value.SetExplicitCharset(self.explicit_charset);
    }
}

/// Owned snapshot of a column expression. Virtual expressions are recursive snapshots.
#[derive(Clone)]
pub struct CachedColumn {
    pub ret_type: Option<types::FieldType>,
    pub id: i64,
    pub unique_id: i64,
    pub index: isize,
    pub virtual_expr: Option<Box<CachedExpression>>,
    pub orig_name: String,
    pub is_hidden: bool,
    pub is_prefix: bool,
    pub in_operand: bool,
    pub correlated_col_unique_id: i64,
    collation: CachedCollation,
}

/// Owned snapshot of a schema, including ordered and duplicate key shapes.
#[derive(Clone)]
pub struct CachedSchema {
    pub columns: Vec<CachedColumn>,
    pub pk_or_uk: Vec<Vec<CachedColumn>>,
    pub nullable_uk: Vec<Vec<CachedColumn>>,
}

/// Owned snapshot of result-field names. `None` entries retain their positions.
#[derive(Clone)]
pub struct CachedNameSlice(pub Vec<Option<CachedFieldName>>);

/// Owned field-name metadata without shared runtime pointers.
#[derive(Clone)]
pub struct CachedFieldName {
    pub orig_tbl_name: ast::CIStr,
    pub orig_col_name: ast::CIStr,
    pub db_name: ast::CIStr,
    pub tbl_name: ast::CIStr,
    pub col_name: ast::CIStr,
    pub hidden: bool,
    pub not_explicit_usable: bool,
    pub redundant: bool,
}

/// Owned snapshot of a correlated column. The runtime slot is copied into a fresh slot on restore.
#[derive(Clone)]
pub struct CachedCorrelatedColumn {
    pub column: CachedColumn,
    pub value: Option<types::Datum>,
}

/// Owned snapshot of a constant, including deferred and prepared-parameter semantics.
#[derive(Clone)]
pub struct CachedConstant {
    pub value: types::Datum,
    pub ret_type: Option<types::FieldType>,
    pub deferred_expr: Option<Box<CachedExpression>>,
    pub param_order: Option<usize>,
    pub subquery_ref_id: i64,
    collation: CachedCollation,
}

/// Owned scalar-function snapshot. Core registry identity plus recursive arguments rebuilds its signature.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CachedBuiltinId(String);

impl CachedBuiltinId {
    const PREFIX: &'static str = "builtin:v1:";

    pub(crate) fn from_name(name: &str) -> Result<Self, CacheSnapshotError> {
        let canonical_name = name.to_ascii_lowercase();
        if !is_core_cache_snapshot_builtin(&canonical_name) {
            return Err(unsupported(format!(
                "builtin {canonical_name} is not in the plan-cache snapshot registry"
            )));
        }
        Ok(Self(format!("{}{}", Self::PREFIX, canonical_name)))
    }

    pub(crate) fn registered_name(&self) -> Result<&str, CacheSnapshotError> {
        let Some(name) = self.0.strip_prefix(Self::PREFIX) else {
            return Err(unsupported(format!(
                "unknown builtin snapshot id {}",
                self.0
            )));
        };
        if name.is_empty() || !is_core_cache_snapshot_builtin(name) {
            return Err(unsupported(format!(
                "unknown builtin snapshot id {}",
                self.0
            )));
        }
        Ok(name)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[cfg(test)]
    pub(crate) fn from_raw_for_test(value: &str) -> Self {
        Self(value.to_owned())
    }
}

#[derive(Clone)]
pub struct CachedScalarFunction {
    pub builtin_id: CachedBuiltinId,
    pub ret_type: types::FieldType,
    pub arguments: Vec<CachedExpression>,
    pub grouping_metadata: Option<(i64, Vec<Vec<u64>>)>,
    collation: CachedCollation,
}

/// Closed expression snapshot whitelist used by the plan cache.
#[derive(Clone)]
pub enum CachedExpression {
    Column(CachedColumn),
    CorrelatedColumn(CachedCorrelatedColumn),
    Constant(CachedConstant),
    ScalarFunction(CachedScalarFunction),
}

impl CachedExpression {
    pub fn try_from_expression(expression: &dyn Expression) -> Result<Self, CacheSnapshotError> {
        if let Some(column) = expression.as_any().downcast_ref::<Column>() {
            return Ok(Self::Column(CachedColumn::capture(column)?));
        }
        if let Some(column) = expression.as_any().downcast_ref::<CorrelatedColumn>() {
            let value = column
                .data
                .as_ref()
                .map(|value| {
                    value
                        .read()
                        .map(|value| value.clone())
                        .map_err(|_| unsupported("correlated datum lock is poisoned"))
                })
                .transpose()?;
            return Ok(Self::CorrelatedColumn(CachedCorrelatedColumn {
                column: CachedColumn::capture(&column.column)?,
                value,
            }));
        }
        if let Some(constant) = expression.as_any().downcast_ref::<Constant>() {
            return Ok(Self::Constant(CachedConstant {
                value: constant.Value.clone(),
                ret_type: constant.RetType.clone(),
                deferred_expr: constant
                    .DeferredExpr
                    .as_deref()
                    .map(Self::try_from_expression)
                    .transpose()?
                    .map(Box::new),
                param_order: constant.ParamMarker.as_ref().map(ParamMarker::order),
                subquery_ref_id: constant.SubqueryRefID,
                collation: CachedCollation::capture(constant),
            }));
        }
        if let Some(function) = expression.as_any().downcast_ref::<ScalarFunction>() {
            if function.Function.isExtensionFunction() {
                return Err(unsupported(format!(
                    "extension builtin {} is not cache-snapshot supported",
                    function.FuncName.L
                )));
            }
            if !is_core_cache_snapshot_builtin(&function.FuncName.L) {
                return Err(unsupported(format!(
                    "builtin {} is not in the plan-cache snapshot whitelist",
                    function.FuncName.L
                )));
            }
            if function.Function.groupingMetaInitialized() == Some(false) {
                return Err(unsupported("GROUPING metadata has not been initialized"));
            }
            let ret_type = function
                .RetType
                .clone()
                .ok_or_else(|| unsupported("scalar function has no return type"))?;
            return Ok(Self::ScalarFunction(CachedScalarFunction {
                builtin_id: CachedBuiltinId::from_name(&function.FuncName.L)?,
                ret_type,
                arguments: function
                    .GetArgs()
                    .iter()
                    .map(|argument| Self::try_from_expression(argument.as_ref()))
                    .collect::<Result<_, _>>()?,
                grouping_metadata: function.Function.groupingModeAndMarks(),
                collation: CachedCollation::capture(function),
            }));
        }
        Err(unsupported(format!(
            "expression type {} is not plan-cache snapshot supported",
            std::any::type_name_of_val(expression)
        )))
    }

    pub fn restore(
        &self,
        ctx: &dyn BuildContext,
    ) -> Result<Box<dyn Expression>, CacheSnapshotError> {
        match self {
            Self::Column(column) => Ok(Box::new(column.restore(ctx)?)),
            Self::CorrelatedColumn(column) => Ok(Box::new(CorrelatedColumn {
                column: column.column.restore(ctx)?,
                data: column.value.clone().map(NewCorrelatedDatum),
            })),
            Self::Constant(constant) => {
                let ret_type = constant
                    .ret_type
                    .clone()
                    .ok_or_else(|| unsupported("constant has no return type"))?;
                let mut restored = Constant::with_type(constant.value.clone(), ret_type);
                restored.DeferredExpr = constant
                    .deferred_expr
                    .as_deref()
                    .map(|expression| expression.restore(ctx))
                    .transpose()?;
                restored.ParamMarker = constant.param_order.map(ParamMarker::new);
                restored.SubqueryRefID = constant.subquery_ref_id;
                constant.collation.restore(&mut restored);
                Ok(Box::new(restored))
            }
            Self::ScalarFunction(function) => {
                let arguments = function
                    .arguments
                    .iter()
                    .map(|argument| argument.restore(ctx))
                    .collect::<Result<Vec<_>, _>>()?;
                let name = function.builtin_id.registered_name()?;
                let restored = rebuild_core_cache_snapshot_builtin(
                    ctx,
                    name,
                    function.ret_type.clone(),
                    arguments,
                )
                .map_err(|error| unsupported(error.to_string()))?;
                let scalar = restored
                    .as_any()
                    .downcast_ref::<ScalarFunction>()
                    .ok_or_else(|| {
                        unsupported(format!(
                            "builtin {} did not rebuild a scalar function",
                            function.builtin_id.as_str()
                        ))
                    })?;
                let mut scalar = scalar.clone_scalar();
                if let Some((mode, marks)) = &function.grouping_metadata {
                    scalar
                        .Function
                        .restoreGroupingModeAndMarks(*mode, marks.clone())
                        .map_err(|error| unsupported(error.to_string()))?;
                }
                function.collation.restore(&mut scalar);
                Ok(Box::new(scalar))
            }
        }
    }
}

impl CachedColumn {
    pub fn try_from_column(column: &Column) -> Result<Self, CacheSnapshotError> {
        Self::capture(column)
    }

    fn capture(column: &Column) -> Result<Self, CacheSnapshotError> {
        Ok(Self {
            ret_type: column.RetType.clone(),
            id: column.ID,
            unique_id: column.UniqueID,
            index: column.Index,
            virtual_expr: column
                .VirtualExpr
                .as_deref()
                .map(CachedExpression::try_from_expression)
                .transpose()?
                .map(Box::new),
            orig_name: column.OrigName.clone(),
            is_hidden: column.IsHidden,
            is_prefix: column.IsPrefix,
            in_operand: column.InOperand,
            correlated_col_unique_id: column.CorrelatedColUniqueID,
            collation: CachedCollation::capture(column),
        })
    }

    pub fn restore_column(&self, ctx: &dyn BuildContext) -> Result<Column, CacheSnapshotError> {
        self.restore(ctx)
    }

    fn restore(&self, ctx: &dyn BuildContext) -> Result<Column, CacheSnapshotError> {
        let mut column = Column::default();
        column.RetType = self.ret_type.clone();
        column.ID = self.id;
        column.UniqueID = self.unique_id;
        column.Index = self.index;
        column.VirtualExpr = self
            .virtual_expr
            .as_deref()
            .map(|expression| expression.restore(ctx))
            .transpose()?;
        column.OrigName = self.orig_name.clone();
        column.IsHidden = self.is_hidden;
        column.IsPrefix = self.is_prefix;
        column.InOperand = self.in_operand;
        column.CorrelatedColUniqueID = self.correlated_col_unique_id;
        self.collation.restore(&mut column);
        Ok(column)
    }
}

impl CachedSchema {
    pub fn try_from_schema(schema: &Schema) -> Result<Self, CacheSnapshotError> {
        fn capture_keys(keys: &[KeyInfo]) -> Result<Vec<Vec<CachedColumn>>, CacheSnapshotError> {
            keys.iter()
                .map(|key| key.iter().map(CachedColumn::capture).collect())
                .collect()
        }

        Ok(Self {
            columns: schema
                .Columns
                .iter()
                .map(CachedColumn::capture)
                .collect::<Result<_, _>>()?,
            pk_or_uk: capture_keys(&schema.PKOrUK)?,
            nullable_uk: capture_keys(&schema.NullableUK)?,
        })
    }

    pub fn restore(&self, ctx: &dyn BuildContext) -> Result<Schema, CacheSnapshotError> {
        fn restore_keys(
            keys: &[Vec<CachedColumn>],
            ctx: &dyn BuildContext,
        ) -> Result<Vec<KeyInfo>, CacheSnapshotError> {
            keys.iter()
                .map(|key| key.iter().map(|column| column.restore(ctx)).collect())
                .collect()
        }

        let mut schema = NewSchema(
            self.columns
                .iter()
                .map(|column| column.restore(ctx))
                .collect::<Result<_, _>>()?,
        );
        schema.SetKeys(restore_keys(&self.pk_or_uk, ctx)?);
        schema.SetUniqueKeys(restore_keys(&self.nullable_uk, ctx)?);
        Ok(schema)
    }
}

impl CachedNameSlice {
    pub fn from_name_slice(names: &types::NameSlice) -> Self {
        Self(
            names
                .0
                .iter()
                .map(|name| name.as_deref().map(CachedFieldName::capture))
                .collect(),
        )
    }

    pub fn restore(&self) -> types::NameSlice {
        types::NameSlice(
            self.0
                .iter()
                .map(|name| {
                    name.as_ref()
                        .map(|name| std::sync::Arc::new(name.restore()))
                })
                .collect(),
        )
    }
}

impl CachedFieldName {
    fn capture(name: &types::FieldName) -> Self {
        Self {
            orig_tbl_name: name.OrigTblName.clone(),
            orig_col_name: name.OrigColName.clone(),
            db_name: name.DBName.clone(),
            tbl_name: name.TblName.clone(),
            col_name: name.ColName.clone(),
            hidden: name.Hidden,
            not_explicit_usable: name.NotExplicitUsable,
            redundant: name.Redundant,
        }
    }

    fn restore(&self) -> types::FieldName {
        types::FieldName {
            OrigTblName: self.orig_tbl_name.clone(),
            OrigColName: self.orig_col_name.clone(),
            DBName: self.db_name.clone(),
            TblName: self.tbl_name.clone(),
            ColName: self.col_name.clone(),
            Hidden: self.hidden,
            NotExplicitUsable: self.not_explicit_usable,
            Redundant: self.redundant,
        }
    }
}
