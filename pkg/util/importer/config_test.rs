// Copyright 2026 AsterSQL.

use crate::config::{Config, DbConfig};

#[test]
fn config_string_matches_go_fmt_shape_and_nil_contract() {
    let config = Config {
        table_sql: "create table t (id int)".to_owned(),
        index_sql: "create index i on t (id)".to_owned(),
        log_level: "info".to_owned(),
        db_config: DbConfig {
            user: "root".to_owned(),
            password: "secret".to_owned(),
            host: "127.0.0.1".to_owned(),
            port: 3306,
            schema: "test".to_owned(),
        },
        worker_count: 2,
        job_count: 10,
        batch: 5,
    };

    let expected = "Config({TableSQL:create table t (id int) IndexSQL:create index i on t (id) LogLevel:info DBCfg:{Host:127.0.0.1 User:root Password:secret Schema:test Snapshot: Port:3306} WorkerCount:2 JobCount:10 Batch:5})";
    assert_eq!(Config::string(Some(&config)), expected);
    assert_eq!(config.to_string(), expected);
    assert_eq!(Config::string(None), "<nil>");
}
