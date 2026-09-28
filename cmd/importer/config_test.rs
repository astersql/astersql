// Copyright 2026 AsterSQL.

use crate::config::{Config, DBConfig, NewConfig};

#[test]
fn integer_flags_accept_go_base_zero_and_native_width() {
    let mut cfg = NewConfig();
    cfg.Parse(&[
        "-c=0x10".into(),
        "-n".into(),
        "010".into(),
        "-b=-0x2".into(),
        "-P=0b11".into(),
    ])
    .unwrap();

    assert_eq!(cfg.SysCfg.WorkerCount, 16);
    assert_eq!(cfg.SysCfg.JobCount, 8);
    assert_eq!(cfg.SysCfg.Batch, -2);
    assert_eq!(cfg.DBCfg.Port, 3);

    let mut cfg = NewConfig();
    cfg.Parse(&["-c=0x_10".into(), "-n=1_024".into()]).unwrap();
    assert_eq!(cfg.SysCfg.WorkerCount, 16);
    assert_eq!(cfg.SysCfg.JobCount, 1024);

    let mut cfg = NewConfig();
    let err = cfg.Parse(&["-c=08".into()]).unwrap_err();
    assert!(err.msg.ends_with("parse error"));

    let mut cfg = NewConfig();
    let result = cfg.Parse(&["-c=0x80000000".into()]);
    if isize::BITS == 64 {
        result.unwrap();
        assert_eq!(cfg.SysCfg.WorkerCount as i128, 2_147_483_648);
    } else {
        assert!(result.is_err());
    }
}

#[test]
fn toml_decode_supports_go_toml_syntax_and_cli_precedence() {
    let path = std::env::temp_dir().join(format!(
        "astersql-importer-config-parity-{}.toml",
        std::process::id()
    ));
    std::fs::write(
        &path,
        concat!(
            "[db]\n",
            "host = \"db\\u002Dhost\" # inline comment\n",
            "port = 0x0cea\n",
            "[ddl]\n",
            "table-sql = \"create\\ntable\"\n",
            "[sys]\n",
            "worker-count = 0x80000000\n",
            "job-count = 0o10\n",
            "batch = 1_000\n",
        ),
    )
    .unwrap();

    let mut cfg = NewConfig();
    let result = cfg.Parse(&[
        "-config".into(),
        path.to_string_lossy().into_owned(),
        "-h=cli-host".into(),
        "-c=0x20".into(),
    ]);
    std::fs::remove_file(path).unwrap();
    result.unwrap();

    assert_eq!(cfg.DBCfg.Host, "cli-host");
    assert_eq!(cfg.DBCfg.Port, 3306);
    assert_eq!(cfg.DDLCfg.TableSQL, "create\ntable");
    assert_eq!(cfg.SysCfg.WorkerCount, 32);
    assert_eq!(cfg.SysCfg.JobCount, 8);
    assert_eq!(cfg.SysCfg.Batch, 1000);
}

#[test]
fn flag_syntax_and_help_match_go_flag_set() {
    let mut cfg = NewConfig();
    let err = cfg
        .Parse(&["---h".into(), "other-host".into()])
        .unwrap_err();
    assert_eq!(err.msg, "bad flag syntax: ---h");
    assert_eq!(cfg.DBCfg.Host, "127.0.0.1");

    let mut cfg = NewConfig();
    let err = cfg.Parse(&["-help=anything".into()]).unwrap_err();
    assert!(err.is_help, "Go treats an undefined help flag as ErrHelp");
}

#[test]
fn flag_errors_preserve_go_error_shape() {
    let mut cfg = NewConfig();
    let err = cfg.Parse(&["-c".into()]).unwrap_err();
    assert_eq!(err.msg, "flag needs an argument: -c");

    let mut cfg = NewConfig();
    let err = cfg.Parse(&["-".into()]).unwrap_err();
    assert_eq!(err.msg, "'-' is an invalid flag");

    let mut cfg = NewConfig();
    let err = cfg.Parse(&["-c=not-an-int".into()]).unwrap_err();
    assert_eq!(
        err.msg,
        "invalid value \"not-an-int\" for flag -c: parse error"
    );
}

#[test]
fn db_config_string_matches_go_fmt_output() {
    let cfg = DBConfig::default();
    assert_eq!(
        DBConfig::String(Some(&cfg)),
        "DBConfig({Host:127.0.0.1 User:root Password: Name:test Port:3306})"
    );
    assert_eq!(DBConfig::String(None), "<nil>");
}

#[test]
fn config_string_matches_go_fmt_shape() {
    let cfg = NewConfig();
    let rendered = Config::String(Some(&cfg));

    assert!(rendered.starts_with("Config({FlagSet:0x"));
    assert!(rendered.contains(
        " DBCfg:{Host:127.0.0.1 User:root Password: Name:test Port:3306} \
         DDLCfg:{TableSQL: IndexSQL:} StatsCfg:{Path:} \
         SysCfg:{LogLevel:info WorkerCount:2 JobCount:10000 Batch:1000} configFile:"
    ));
    assert!(rendered.ends_with("})"));
    assert!(!rendered.contains("help_requested"));
    assert_eq!(Config::String(None), "<nil>");
}
