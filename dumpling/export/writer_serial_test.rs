// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Go `writer_serial_test.go` 的 Rust 对等测试。

use crate::main_test::{app_logger, default_config_for_test};
use crate::util_for_test::{bytes_cell, mockMetaIR, mockPoisonWriter, mockTableIR, null_cell};
use crate::*;

struct MemBufWriter(Vec<u8>);
impl MemBufWriter {
    fn new() -> Self {
        Self(vec![])
    }
    fn string(&self) -> String {
        String::from_utf8(self.0.clone()).unwrap()
    }
}
impl ObjectWriter for MemBufWriter {
    fn Write(&mut self, data: &[u8]) -> Result<usize> {
        self.0.extend_from_slice(data);
        Ok(data.len())
    }
    fn Close(&mut self) -> Result<()> {
        Ok(())
    }
}

// Match sqlmock.RowError at index 3: exactly three successfully decoded rows.
fn fail_on_fourth_row(ir: &mut dyn TableDataIR) -> impl TableDataIR {
    struct LastRowError {
        inner: Box<dyn SQLRowIter>,
        advanced: usize,
    }
    impl SQLRowIter for LastRowError {
        fn Decode(&mut self, row: &mut dyn RowReceiver) -> Result<()> {
            self.inner.Decode(row)
        }
        fn Next(&mut self) {
            self.advanced += 1;
            if self.advanced < 3 {
                self.inner.Next();
            }
        }
        fn Error(&self) -> Option<Error> {
            (self.advanced >= 3).then(|| errors_new("mock row error"))
        }
        fn HasNext(&self) -> bool {
            self.advanced < 3 && self.inner.HasNext()
        }
        fn Close(&mut self) -> Result<()> {
            self.inner.Close()
        }
    }
    struct ErrorIR(Option<Box<dyn SQLRowIter>>);
    impl TableDataIR for ErrorIR {
        fn Start(&mut self, _: &tcontext::Context, _: &Conn) -> Result<()> {
            Ok(())
        }
        fn Rows(&mut self) -> Box<dyn SQLRowIter> {
            self.0.take().unwrap()
        }
        fn Close(&mut self) -> Result<()> {
            Ok(())
        }
        fn RawRows(&mut self) -> Option<&mut Rows> {
            None
        }
    }
    ErrorIR(Some(Box::new(LastRowError {
        inner: ir.Rows(),
        advanced: 0,
    })))
}

const TYPES: &[&str] = &["INT", "SET", "VARCHAR", "VARCHAR", "TEXT"];
fn data() -> Vec<Vec<Option<Vec<u8>>>> {
    vec![
        vec![
            bytes_cell("1"),
            bytes_cell("male"),
            bytes_cell("bob@mail.com"),
            bytes_cell("020-1234"),
            null_cell(),
        ],
        vec![
            bytes_cell("2"),
            bytes_cell("female"),
            bytes_cell("sarah@mail.com"),
            bytes_cell("020-1253"),
            bytes_cell("healthy"),
        ],
        vec![
            bytes_cell("3"),
            bytes_cell("male"),
            bytes_cell("john@mail.com"),
            bytes_cell("020-1256"),
            bytes_cell("healthy"),
        ],
        vec![
            bytes_cell("4"),
            bytes_cell("female"),
            bytes_cell("sarah@mail.com"),
            bytes_cell("020-1235"),
            bytes_cell("healthy"),
        ],
    ]
}
fn csv_conf() -> Config {
    let mut c = default_config_for_test();
    c.NoHeader = true;
    c.CsvNullValue = "\\N".into();
    c.CsvDelimiter = "\"".into();
    c.CsvSeparator = ",".into();
    c.CsvLineTerminator = "\r\n".into();
    c
}
fn gauges(m: &metrics, rows: f64, size: usize) {
    assert_eq!(ReadGauge(Some(&m.finishedRowsGauge)), rows);
    assert_eq!(ReadGauge(Some(&m.finishedSizeGauge)), size as f64);
}

#[test]
fn test_write_meta() {
    let t = tcontext::Background().WithLogger(app_logger());
    let ddl = "CREATE TABLE `t1` (\n  `a` int(11) DEFAULT NULL\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;\n";
    let mut meta = mockMetaIR::new("t1", ddl, &["/*!40103 SET TIME_ZONE='+00:00' */;"]);
    let mut w = MemBufWriter::new();
    WriteMeta(&t, &mut meta, &mut w).unwrap();
    assert_eq!(
        w.string(),
        format!("/*!40103 SET TIME_ZONE='+00:00' */;\n{ddl}")
    );
}

#[test]
fn test_write_insert() {
    let t = tcontext::Background().WithLogger(app_logger());
    let c = default_config_for_test();
    let meta = mockTableIR::new("test", "employee", vec![], &[], TYPES);
    let mut ir = mockTableIR::new("test", "employee", data(), &[], TYPES);
    let mut w = MemBufWriter::new();
    let m = newMetrics(c.PromFactory.as_ref(), &c.Labels);
    WriteInsertSQL(&t, &c, &meta, &mut ir, &mut w, Some(&m)).unwrap();
    let exp = "INSERT INTO `employee` VALUES\n(1,'male','bob@mail.com','020-1234',NULL),\n(2,'female','sarah@mail.com','020-1253','healthy'),\n(3,'male','john@mail.com','020-1256','healthy'),\n(4,'female','sarah@mail.com','020-1235','healthy');\n";
    assert_eq!(w.string(), exp);
    gauges(&m, 4.0, exp.len());
}

#[test]
fn test_write_insert_returns_error() {
    let t = tcontext::Background().WithLogger(app_logger());
    let c = default_config_for_test();
    let meta = mockTableIR::new("test", "employee", vec![], &[], TYPES);
    let mut ir = mockTableIR::new("test", "employee", data(), &[], TYPES);
    let mut ir = fail_on_fourth_row(&mut ir);
    let mut w = MemBufWriter::new();
    let m = newMetrics(c.PromFactory.as_ref(), &c.Labels);
    assert_eq!(
        WriteInsertSQL(&t, &c, &meta, &mut ir, &mut w, Some(&m))
            .unwrap_err()
            .to_string(),
        "mock row error"
    );
    let expected = "INSERT INTO `employee` VALUES\n(1,'male','bob@mail.com','020-1234',NULL),\n(2,'female','sarah@mail.com','020-1253','healthy'),\n(3,'male','john@mail.com','020-1256','healthy');\n";
    assert_eq!(w.string(), expected);
    gauges(&m, 0.0, 0);
}

#[test]
fn test_write_insert_in_csv() {
    let t = tcontext::Background().WithLogger(app_logger());
    let c = csv_conf();
    let meta = mockTableIR::new("test", "employee", vec![], &[], TYPES);
    let mut ir = mockTableIR::new("test", "employee", data(), &[], TYPES);
    let mut w = MemBufWriter::new();
    let m = newMetrics(c.PromFactory.as_ref(), &c.Labels);
    WriteInsertInCsv(&t, &c, &meta, &mut ir, &mut w, Some(&m)).unwrap();
    let exp = "1,\"male\",\"bob@mail.com\",\"020-1234\",\\N\r\n2,\"female\",\"sarah@mail.com\",\"020-1253\",\"healthy\"\r\n3,\"male\",\"john@mail.com\",\"020-1256\",\"healthy\"\r\n4,\"female\",\"sarah@mail.com\",\"020-1235\",\"healthy\"\r\n";
    assert_eq!(w.string(), exp);
    gauges(&m, 4.0, exp.len());
}

#[test]
fn test_write_insert_in_csv_returns_error() {
    let t = tcontext::Background().WithLogger(app_logger());
    let c = csv_conf();
    let meta = mockTableIR::new("test", "employee", vec![], &[], TYPES);
    let mut ir = mockTableIR::new("test", "employee", data(), &[], TYPES);
    let mut ir = fail_on_fourth_row(&mut ir);
    let mut w = MemBufWriter::new();
    let m = newMetrics(c.PromFactory.as_ref(), &c.Labels);
    assert_eq!(
        WriteInsertInCsv(&t, &c, &meta, &mut ir, &mut w, Some(&m))
            .unwrap_err()
            .to_string(),
        "mock row error"
    );
    let expected = "1,\"male\",\"bob@mail.com\",\"020-1234\",\\N\r\n2,\"female\",\"sarah@mail.com\",\"020-1253\",\"healthy\"\r\n3,\"male\",\"john@mail.com\",\"020-1256\",\"healthy\"\r\n";
    assert_eq!(w.string(), expected);
    gauges(&m, 0.0, 0);
}

#[test]
fn test_write_insert_in_csv_with_dialect() {
    let t = tcontext::Background().WithLogger(app_logger());
    let d = vec![
        vec![bytes_cell("1"), bytes_cell("blob1")],
        vec![bytes_cell("2"), bytes_cell("blob2")],
    ];
    for (dialect, vals) in [
        (CSVDialect::CSVDialectDefault, ["blob1", "blob2"]),
        (CSVDialect::CSVDialectRedshift, ["626c6f6231", "626c6f6232"]),
        (CSVDialect::CSVDialectBigQuery, ["YmxvYjE=", "YmxvYjI="]),
    ] {
        let mut c = csv_conf();
        c.CsvOutputDialect = dialect;
        let meta = mockTableIR::new("test", "employee", vec![], &[], &["INT", "BLOB"]);
        let mut ir = mockTableIR::new("test", "employee", d.clone(), &[], &["INT", "BLOB"]);
        let mut w = MemBufWriter::new();
        let m = newMetrics(c.PromFactory.as_ref(), &c.Labels);
        WriteInsertInCsv(&t, &c, &meta, &mut ir, &mut w, Some(&m)).unwrap();
        let exp = format!("1,\"{}\"\r\n2,\"{}\"\r\n", vals[0], vals[1]);
        assert_eq!(w.string(), exp);
        gauges(&m, 2.0, exp.len());
    }
}

#[test]
fn test_sql_data_types() {
    let t = tcontext::Background().WithLogger(app_logger());
    for (ty, val, result) in [
        ("CHAR", "char1", "'char1'"),
        ("INT", "12345", "12345"),
        ("BINARY", "1234", "x'31323334'"),
    ] {
        let c = default_config_for_test();
        let meta = mockTableIR::new("test", "t", vec![], &[], &[ty]);
        let mut ir = mockTableIR::new("test", "t", vec![vec![bytes_cell(val)]], &[], &[ty]);
        let mut w = MemBufWriter::new();
        let m = newMetrics(c.PromFactory.as_ref(), &c.Labels);
        WriteInsertSQL(&t, &c, &meta, &mut ir, &mut w, Some(&m)).unwrap();
        let exp = format!("INSERT INTO `t` VALUES\n({result});\n");
        assert_eq!(w.string(), exp);
        gauges(&m, 1.0, exp.len());
    }
}

#[test]
fn test_write() {
    let t = tcontext::Background().WithLogger(app_logger());
    let mut w = mockPoisonWriter::new();
    for s in ["test", "loooooooooooooooooooong"] {
        write(&t, &mut w, s).unwrap();
        assert_eq!(w.buf, s);
    }
    assert_eq!(
        write(&t, &mut w, "poison").unwrap_err().to_string(),
        "poison_error"
    );
    write(&t, &mut w, "test").unwrap();
}
