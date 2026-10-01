// Copyright 2026 AsterSQL.

//! Bridges the shared InfoSchema validator to storage commit-time checks.
use astersql_domain::schema_checker::{RelatedSchemaChange, SchemaCheckResult, SchemaValidator};
use astersql_infoschema_isvalidator as validator;
use std::sync::Arc;

pub(super) struct SharedValidator(pub Arc<validator::Validator>);
impl SchemaValidator for SharedValidator {
    fn check(&self, ts: u64, version: i64, tables: &[i64], delta: bool) -> SchemaCheckResult {
        let (change, result) = self.0.check(ts, version, Some(tables), delta);
        match result {
            validator::Result::ResultSucc => SchemaCheckResult::Success,
            validator::Result::ResultUnknown => SchemaCheckResult::Unknown,
            validator::Result::ResultFail => SchemaCheckResult::Fail(change.map(|change| {
                RelatedSchemaChange {
                    physical_table_ids: change.phy_tbl_ids,
                    action_types: change
                        .action_types
                        .into_iter()
                        .map(|action| action.to_string())
                        .collect(),
                }
            })),
        }
    }
}

pub(super) fn checker(
    validator: Arc<validator::Validator>,
    version: i64,
    tables: Vec<i64>,
    delta: bool,
) -> astersql_kv::TransactionSchemaChecker {
    let checker = astersql_domain::schema_checker::SchemaChecker::new(
        Arc::new(SharedValidator(validator)),
        version,
        tables,
        delta,
    );
    astersql_kv::TransactionSchemaChecker(Arc::new(move |ts| {
        checker.check(ts).map_err(|error| match error {
            astersql_domain::schema_checker::SchemaCheckError::InfoSchemaChanged(_) => {
                astersql_domain::domain::ERR_INFO_SCHEMA_CHANGED.FastGenByArgs(&[])
            }
            astersql_domain::schema_checker::SchemaCheckError::InfoSchemaExpired => {
                astersql_domain::domain::ERR_INFO_SCHEMA_EXPIRED.FastGenByArgs(&[])
            }
        })
    }))
}
