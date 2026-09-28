// Copyright 2026 AsterSQL.

use astersql_dumpling_export::{self as export, CompressType};

use crate::config_flags::{DefineFlags, ParseFromFlags};
use crate::stubs::FlagSet;

fn parse_compress(value: &str) -> Result<CompressType, String> {
    let mut flags = FlagSet::new();
    DefineFlags(&mut flags);
    flags.Parse(&["--compress".into(), value.into()])?;
    let mut conf = export::DefaultConfig();
    ParseFromFlags(&mut conf, &flags)?;
    Ok(conf.CompressType)
}

#[test]
fn compress_flag_matches_go_accepted_values() {
    assert_eq!(parse_compress("zst").unwrap(), CompressType::Zstd);
    assert!(parse_compress("none").is_err());
    assert!(parse_compress("uncompressed").is_err());
    assert!(parse_compress("GZIP").is_err());
}
