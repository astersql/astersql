// Copyright 2026 AsterSQL.
//! Real external libpq client regression. Install libpq 18 and Python 3;
//! PG_LIBPQ_LIBRARY selects the system library. No protocol code is reimplemented.
use crate::runtime::{BootstrapAuthMode, CanonicalConnectionDomain, ConcreteSessionDriver};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

#[test]
fn postgres_client_protocol_versions() {
    pg_introspection_clients();
}

#[test]
fn pg_introspection_clients() {
    run_clients(true);
}

#[test]
fn pg_introspection_clients_mysql_only() {
    run_clients(false);
}

#[test]
fn pg_introspection_clients_stored_view_source() {
    for (stored, expected) in [
        (
            "CREATE VIEW `as` AS SELECT 'AS WITH LOCAL CHECK OPTION' AS label",
            "SELECT 'AS WITH LOCAL CHECK OPTION' AS label",
        ),
        (
            "CREATE VIEW v AS SELECT id FROM t WITH LOCAL CHECK OPTION;",
            "SELECT id FROM t",
        ),
        (
            "CREATE VIEW v AS SELECT id FROM t WITH CASCADED CHECK OPTION",
            "SELECT id FROM t",
        ),
        (
            "CREATE VIEW v AS /* AS ignored */ SELECT 1",
            "/* AS ignored */ SELECT 1",
        ),
        ("SELECT 1", "SELECT 1"),
    ] {
        assert_eq!(
            crate::pg_name::stored_view_select(stored).unwrap(),
            expected
        );
    }
    assert!(crate::pg_name::stored_view_select("DROP TABLE t").is_err());
    assert!(crate::pg_name::stored_view_select("SELECT 1; SELECT 2").is_err());
}

struct CloseServer<'a>(&'a crate::server::Server);

impl Drop for CloseServer<'_> {
    fn drop(&mut self) {
        self.0.close();
    }
}

#[test]
fn pg_core_libpq_insert_readback() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain, Some(0));
    let _close = CloseServer(&server);
    let address = server.postgres_listener_addr().unwrap();
    let database = format!("pg_core_{}_{}", std::process::id(), address.port());
    native
        .execute(&format!("CREATE DATABASE {database}"))
        .unwrap();
    // Capture spawn/client failures until after teardown; the server guard also
    // closes listeners and their sessions if setup or cleanup panics.
    let result = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/pg_core_libpq.py"
        ))
        .arg(address.port().to_string())
        .arg(&database)
        .env_remove("PG_JDBC_LIBRARIES")
        .env_remove("PG_JAVA")
        .env_remove("JAVA_HOME")
        .env_remove("CLASSPATH")
        .output();
    server.close();
    native
        .execute(&format!("DROP DATABASE {database}"))
        .unwrap();
    assert!(TcpStream::connect(address).is_err(), "PG listener leaked");
    let output = result.expect("Python 3 is required for the independent libpq client");
    println!("{}", String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success(),
        "independent libpq workflow failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn pg_core_libpq_integer_differential() {
    use std::io::BufRead;
    use std::process::Stdio;

    assert!(
        std::env::var_os("PG18_REFERENCE_DSN").is_some(),
        "PG18_REFERENCE_DSN is required (no skip)"
    );
    let run = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let evidence = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/pg-core-evidence")
        .join(&run);
    std::fs::create_dir_all(evidence.parent().unwrap()).unwrap();
    std::fs::create_dir(&evidence).unwrap();
    println!("PG_CORE_EVIDENCE_DIR={}", evidence.display());
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&repository)
        .output()
        .expect("git HEAD is required for provenance");
    assert!(head.status.success());
    let build = serde_json::json!({
        "head": String::from_utf8(head.stdout).unwrap().trim(),
        "path": std::env::current_exe().unwrap(),
        "profile": "cargo-test-default-features",
        "storage": "CreateAnalyzeSession test storage",
        "authentication": "InsecureRootOnly"
    });
    std::fs::write(evidence.join("rust-build.json"), build.to_string()).unwrap();
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain, Some(0));
    let _close = CloseServer(&server);
    let address = server.postgres_listener_addr().unwrap();
    let database = format!("pg_diff_{}_{}", std::process::id(), address.port());
    native
        .execute(&format!("CREATE DATABASE {database}"))
        .unwrap();
    let mysql_address = server.listener_addr().unwrap();
    // Declared before the child guard: on unwind, Python finishes its cleanup first.
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let cleanup = || {
        server.close();
        let dropped = native.execute(&format!("DROP DATABASE {database}")).is_ok();
        let record = serde_json::json!({"database_dropped": dropped,
            "postgres_port_closed": TcpStream::connect(address).is_err(),
            "mysql_port_closed": TcpStream::connect(mysql_address).is_err()});
        let _ = std::fs::write(evidence.join("rust-cleanup.json"), record.to_string());
    };
    let cleanup_guard = Cleanup(cleanup);
    let mut mysql = TcpStream::connect(mysql_address).unwrap();
    mysql
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    mysql_handshake(&mut mysql);
    mysql_isolation(&mut mysql);
    let checkpoint = |mysql: &mut TcpStream, phase: &str| {
        let sql = "SELECT id, CAST('-17' AS SIGNED) FROM public.client_isolation";
        let actual = mysql_query(mysql, sql);
        let record = serde_json::json!({"run": run, "case_id": phase,
            "side": "mysql", "attempt": 1, "input": {"command": sql},
            "observation": {"rows": actual}, "outcome": "bypass_health"});
        use std::fs::OpenOptions;
        let mut archive = OpenOptions::new()
            .create(true)
            .append(true)
            .open(evidence.join("mysql.jsonl"))
            .unwrap();
        writeln!(archive, "{record}").unwrap();
        archive.sync_all().unwrap();
        assert_eq!(
            actual,
            vec![vec![Some(b"31".to_vec()), Some(b"-17".to_vec())]]
        );
    };
    checkpoint(&mut mysql, "before");
    struct ChildGuard(std::process::Child, std::path::PathBuf);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            // EOF releases the Python checkpoint even if a MySQL assertion panics,
            // allowing its finally block to clean the reference endpoint as well.
            drop(self.0.stdin.take());
            if let Ok(status) = self.0.wait() {
                let _ = std::fs::write(
                    &self.1,
                    serde_json::json!({
                    "success": status.success(), "code": status.code(), "status": status.to_string()})
                    .to_string(),
                );
            }
        }
    }
    let mut child = ChildGuard(
        Command::new("python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/testdata/pg_core_libpq.py"
            ))
            .args([
                address.port().to_string(),
                database.clone(),
                "integer-differential".into(),
            ])
            .env("PG_CORE_EVIDENCE_DIR", &evidence)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("Python 3 is required"),
        evidence.join("child-exit.json"),
    );
    let stdout = child.0.stdout.take().unwrap();
    let mut middle = false;
    for line in std::io::BufReader::new(stdout).lines() {
        let line = line.unwrap();
        println!("{line}");
        if line == "PG_DIFFERENTIAL_MID" {
            checkpoint(&mut mysql, "during");
            middle = true;
            child
                .0
                .stdin
                .as_mut()
                .unwrap()
                .write_all(b"continue\n")
                .unwrap();
        }
    }
    let status = child.0.wait().unwrap();
    checkpoint(&mut mysql, "after");
    mysql_query(&mut mysql, "DROP DATABASE public");
    mysql_packet(&mut mysql, 0, &[0x01]);
    std::fs::write(
        evidence.join("child-exit.json"),
        serde_json::json!({"success": status.success(), "code": status.code(), "status": status.to_string()}).to_string(),
    )
    .unwrap();
    drop(cleanup_guard);
    let cleanup: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(evidence.join("rust-cleanup.json")).unwrap())
            .unwrap();
    assert_eq!(cleanup["database_dropped"], true);
    assert_eq!(cleanup["postgres_port_closed"], true);
    assert_eq!(cleanup["mysql_port_closed"], true);
    let summary: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(evidence.join("summary.json")).unwrap())
            .unwrap();
    assert_eq!(summary["run"], run);
    if std::env::var("PG_CORE_INJECT_CLEANUP_FAILURE").as_deref() == Ok("reference") {
        assert!(
            !status.success(),
            "cleanup failure must fail the Python client"
        );
        assert_eq!(summary["passed"], false);
        assert_eq!(summary["cleanup_ok"], false);
        assert_eq!(summary["unique_effective"], 0);
        assert!(summary["failed"].as_u64().unwrap() >= 1);
    } else {
        assert!(status.success(), "integer differential client failed");
        assert_eq!(summary["passed"], true);
        assert_eq!(summary["discovered"], 104);
        assert_eq!(summary["executed"], 104);
        assert_eq!(summary["unique_effective"], 101);
        assert_eq!(summary["self_checks"], 3);
        assert!(summary["assertions"].as_u64().unwrap() > 0);
        for field in ["failed", "exceptions", "timeouts", "skipped", "ignored"] {
            assert_eq!(summary[field], 0, "unexpected {field}");
        }
    }
    let mysql_records: Vec<serde_json::Value> =
        std::fs::read_to_string(evidence.join("mysql.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    assert_eq!(mysql_records.len(), 3);
    for (record, phase) in mysql_records.iter().zip(["before", "during", "after"]) {
        assert_eq!(record["run"], run);
        assert_eq!(record["case_id"], phase);
        assert_eq!(record["outcome"], "bypass_health");
    }
    assert!(middle, "missing middle MySQL checkpoint");
    let matrix_status = Command::new("python3")
        .args(["-c", "import sys; from pg_core_evidence import finalize_capabilities; finalize_capabilities(sys.argv[1])"])
        .arg(&evidence)
        .current_dir(repository.join("pkg/server/testdata"))
        .status()
        .unwrap();
    assert!(matrix_status.success(), "capability finalization failed");
    let matrix: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(evidence.join("capabilities.json")).unwrap())
            .unwrap();
    assert_eq!(matrix["run"], run);
    assert_eq!(
        matrix["capabilities"]["mysql.bypass_health"]["status"],
        "unverified"
    );
    let eligible: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(evidence.join("provenance-gate.json")).unwrap(),
    )
    .unwrap();
    let support = eligible["eligible"] == true && summary["passed"] == true;
    for (key, capability) in matrix["capabilities"].as_object().unwrap() {
        if key != "mysql.bypass_health" {
            assert_eq!(
                capability["status"],
                if support { "supported" } else { "unverified" },
                "{key}"
            );
        }
    }

    println!(
        "MySQL SELECT/CAST before, during and after PG; prepared regression and listener cleanup passed"
    );
}

pub(super) fn start_client_server(
    domain: Arc<astersql_domain::Domain>,
    postgres_port: Option<u16>,
) -> Arc<crate::server::Server> {
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    struct Driver;
    impl crate::server::ServerDriver for Driver {
        fn name(&self) -> &str {
            "tidb"
        }
    }
    let server = crate::server::Server::new_test(
        crate::server::ServerConfig {
            host: "127.0.0.1".into(),
            port: 0,
            postgres_port,
            status: crate::server::StatusConfig {
                report_status: false,
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(Driver),
    );
    server
        .set_connection_runtime(
            driver,
            Arc::new(CanonicalConnectionDomain::new(domain.clone())),
        )
        .unwrap();
    server
        .run(Arc::new(crate::runtime::CanonicalServerDomain::new(domain)))
        .unwrap();
    server
}

fn run_clients(pg_enabled: bool) {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let ui_port = std::env::var("PG_DATAGRIP_UI_PORT").ok().map(|port| {
        port.parse::<u16>()
            .expect("PG_DATAGRIP_UI_PORT must be a port")
    });
    let database = if ui_port.is_some() {
        "pg_datagrip_ui".into()
    } else {
        format!("pg_clients_{}_{}", std::process::id(), u8::from(pg_enabled))
    };
    native
        .execute(&format!("CREATE DATABASE {database}"))
        .unwrap();
    for sql in [
        format!("CREATE TABLE {database}.client_view_base (id INT)"),
        format!(
            "CREATE VIEW `{database}`.`client_view_live` AS SELECT id FROM `{database}`.`client_view_base`"
        ),
        format!("CREATE TABLE {database}.client_isolation (id INT)"),
        format!("INSERT INTO {database}.client_isolation VALUES (17)"),
    ] {
        native.execute(&sql).unwrap();
    }
    if ui_port.is_some() {
        native.execute(&format!("CREATE TABLE {database}.dg_ui_structure (id INT PRIMARY KEY, note VARCHAR(30) NOT NULL DEFAULT 'ready', UNIQUE KEY note_unique (note, id))")).unwrap();
    }
    let view_source = format!("SELECT id FROM `{database}`.`client_view_base`");
    let server = start_client_server(domain, pg_enabled.then_some(ui_port.unwrap_or(0)));
    let _close = CloseServer(&server);
    let address = server.postgres_listener_addr();
    let mut mysql = TcpStream::connect(server.listener_addr().unwrap()).unwrap();
    mysql
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    mysql_handshake(&mut mysql);
    mysql_isolation(&mut mysql);
    let Some(address) = address else {
        assert!(!pg_enabled);
        mysql_query(&mut mysql, "DROP DATABASE public");
        mysql_packet(&mut mysql, 0, &[0x01]);
        native
            .execute(&format!("DROP DATABASE {database}"))
            .unwrap();
        return;
    };
    if ui_port.is_some() {
        let control = std::path::PathBuf::from(
            std::env::var("PG_DATAGRIP_UI_CONTROL")
                .expect("PG_DATAGRIP_UI_CONTROL must name a temporary control file"),
        );
        assert!(!control.exists(), "UI control file must be fresh");
        println!(
            "DATAGRIP_UI_READY pid={} port={} database={} table=dg_ui_structure",
            std::process::id(),
            address.port(),
            database
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(1200);
        while !control.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
        }
        assert!(control.exists(), "DataGrip UI session timed out");
        std::fs::remove_file(control).unwrap();
    }
    // The real libpq and JDBC clients use disjoint temporary object names and
    // only share catalog reads, so run the independent client workflows while
    // the same server fixture is alive instead of paying their latency serially.
    let (result, jdbc_result) = std::thread::scope(|scope| {
        let libpq = scope.spawn(|| {
            Command::new("python3")
                .arg("-c")
                .arg(LIBPQ_WORKFLOW)
                .arg(address.port().to_string())
                .arg(crate::pg_catalog::DATABASES_SQL)
                .args([
                    DATAGRIP_VIEW_SOURCES_SQL,
                    DATAGRIP_FUNCTION_SOURCES_SQL,
                    DATAGRIP_RELATIONS_SQL,
                ])
                .args([database.as_str(), view_source.as_str()])
                .output()
        });
        let jdbc_result = run_jdbc(address.port(), &database, &view_source);
        (
            libpq.join().expect("libpq workflow thread panicked"),
            jdbc_result,
        )
    });
    assert_eq!(
        mysql_query(&mut mysql, "SELECT id FROM public.client_isolation"),
        vec![vec![Some(b"31".to_vec())]]
    );
    mysql_query(&mut mysql, "DROP DATABASE public");
    // The authenticated MySQL connection stays usable after the PG workload.
    mysql_packet(&mut mysql, 0, &[0x0e]); // COM_PING
    assert_eq!(mysql_read(&mut mysql).first(), Some(&0));
    mysql_packet(&mut mysql, 0, &[0x01]); // COM_QUIT
    server.close();
    native
        .execute(&format!("DROP DATABASE {database}"))
        .unwrap();
    let output = result.expect("Python 3 is required to call the external libpq client");
    println!("{}", String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success(),
        "external libpq workflow failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    jdbc_result.unwrap();
}

fn run_jdbc(port: u16, database: &str, view_source: &str) -> Result<(), String> {
    let home = std::env::var("HOME").map_err(|e| e.to_string())?;
    let libraries = std::env::var_os("PG_JDBC_LIBRARIES")
        .map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .unwrap_or_else(|| ["42.7.13", "42.7.3"].iter().map(|version| {
            std::path::PathBuf::from(&home).join(format!("Library/Application Support/JetBrains/DataGrip2025.1/jdbc-drivers/PostgreSQL/{version}/org/postgresql/postgresql/{version}/postgresql-{version}.jar"))
        }).collect());
    let java = std::env::var("PG_JAVA").unwrap_or_else(|_| {
        "/Applications/DataGrip.app/Contents/jbr/Contents/Home/bin/java".into()
    });
    let directory =
        std::env::temp_dir().join(format!("pg-introspection-{}-{port}", std::process::id()));
    std::fs::create_dir(&directory).map_err(|e| e.to_string())?;
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(directory.clone());
    let source = directory.join("PgIntrospection.java");
    std::fs::write(&source, JDBC_WORKFLOW).map_err(|e| e.to_string())?;
    if libraries.is_empty() {
        return Err("PG_JDBC_LIBRARIES must include a real installed JDBC driver".into());
    }
    for library in &libraries {
        if !library.is_file() {
            return Err(format!(
                "missing installed PostgreSQL JDBC driver: {}",
                library.display()
            ));
        }
    }
    let outputs = std::thread::scope(|scope| {
        let workers = libraries
            .iter()
            .enumerate()
            .map(|(index, library)| {
                let java = &java;
                let source = &source;
                scope.spawn(move || {
                    Command::new(java)
                        .arg("--class-path")
                        .arg(library)
                        .arg(source)
                        .arg(port.to_string())
                        .args([
                            DATAGRIP_VIEW_SOURCES_SQL,
                            DATAGRIP_FUNCTION_SOURCES_SQL,
                            DATAGRIP_RELATIONS_SQL,
                        ])
                        .args([database, view_source])
                        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/pg_datagrip"))
                        .arg(index.to_string())
                        .output()
                })
            })
            .collect::<Vec<_>>();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("JDBC workflow thread panicked"))
            .collect::<Vec<_>>()
    });
    let mut failures = Vec::new();
    for output in outputs {
        let output = output.map_err(|e| format!("start installed Java runtime: {e}"))?;
        println!("{}", String::from_utf8_lossy(&output.stdout));
        if !output.status.success() {
            failures.push(format!(
                "JDBC workflow failed: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}

const JDBC_WORKFLOW: &str = r#"
import java.sql.*;
import java.nio.file.*;
import org.postgresql.util.PGobject;
class PgIntrospection {
    static void check(boolean ok, String message) { if (!ok) throw new AssertionError(message); }
    static String viewSource;
    static void verify(ResultSet rows, int count, boolean view) throws Exception {
        check(rows.getMetaData().getColumnCount() == count, "Describe column count");
        if (view) {
            check(rows.next(), "real view missing");
            check(rows.getString(1).equals("v"), "view kind");
            check(rows.getLong(2) > 0, "view oid");
            check(rows.getString(3).equals(viewSource), "persisted SELECT source: " + rows.getString(3));
        }
        check(!rows.next(), "unexpected introspection rows");
    }
    public static void main(String[] args) throws Exception {
        viewSource = args[5];
        String parseTable = "jdbc_parse_ddl_" + args[7];
        String liveTableName = "jdbc_client_live_" + args[7];
        String options = "sslmode=disable&gssEncMode=disable&prepareThreshold=1&connectTimeout=5&socketTimeout=10";
        try (Connection c = DriverManager.getConnection("jdbc:postgresql://127.0.0.1:"+args[0]+"/"+args[4]+"?"+options, "root", ""); Statement s = c.createStatement()) {
            // Preserve the installed driver defaults for binary parameters/results.
            System.out.println("installed PostgreSQL JDBC " + c.getMetaData().getDriverVersion() + "; " + options);
            long namespace;
            try (ResultSet r = s.executeQuery("select oid from pg_namespace where nspname='public'")) { check(r.next(), "public missing"); namespace = r.getLong(1); }
            for (int i=1; i<=3; i++) {
                try (ResultSet r = s.executeQuery(args[i].replace("?", Long.toString(namespace)))) { verify(r, i==1?3:i==2?5:3, i==1); }
                try (PreparedStatement p = c.prepareStatement(args[i])) {
                    PGobject oid = new PGobject(); oid.setType("oid"); oid.setValue(Long.toString(namespace)); p.setObject(1, oid);
                    check(p.getMetaData().getColumnCount() == (i==2?5:3), "Statement Describe");
                    for (int repeat=0; repeat<2; repeat++) { try (ResultSet r=p.executeQuery()) { verify(r, i==2?5:3, i==1); } }
                    p.setLong(1, namespace);
                    for (int repeat=0; repeat<2; repeat++) { try (ResultSet r=p.executeQuery()) { verify(r, i==2?5:3, i==1); } }
                    p.setNull(1, Types.OTHER, "oid"); try (ResultSet r=p.executeQuery()) { verify(r, i==2?5:3, false); }
                    oid.setValue("4294967295"); p.setObject(1, oid); try (ResultSet r=p.executeQuery()) { verify(r, i==2?5:3, false); }
                }
            }
            try (PreparedStatement p=c.prepareStatement("SELECT relname FROM pg_class WHERE relname='"+parseTable+"'")) {
                check(p.getMetaData().getColumnCount()==1, "Parse-before-DDL Describe");
                try(ResultSet r=p.executeQuery()) { check(!r.next(), "before DDL rows"); }
                s.execute("CREATE TABLE public."+parseTable+" (id INT)");
                try { try(ResultSet r=p.executeQuery()) { check(r.next() && r.getString(1).equals(parseTable) && !r.next(), "live rows after DDL"); } }
                finally { s.execute("DROP TABLE public."+parseTable); }
                try(ResultSet r=p.executeQuery()) { check(!r.next(), "live rows after DROP"); }
            }
            s.execute("CREATE TABLE public."+liveTableName+" (id INT PRIMARY KEY, note VARCHAR(30))");
            try {
                try (PreparedStatement p=c.prepareStatement("SELECT c.oid, c.relname='"+liveTableName+"' AS matched, i.indkey, i.indclass, i.indoption FROM pg_class c JOIN pg_index i ON i.indrelid=c.oid WHERE c.relname='"+liveTableName+"'")) {
                    for (int repeat=0;repeat<3;repeat++) { try (ResultSet r=p.executeQuery()) {
                        check(r.next() && r.getLong(1)>0 && r.getBoolean(2), "default binary catalog identity");
                        check(r.getString(3).equals("1"), "int2vector contents");
                        check(r.getString(4)!=null && r.getArray(5)!=null, "catalog vector/array contents");
                        check(!r.next(), "single primary index");
                    } }
                }
                long liveTable, liveIndex;
                try (ResultSet r=s.executeQuery("SELECT c.oid, i.indexrelid FROM pg_class c JOIN pg_index i ON i.indrelid=c.oid WHERE c.relname='"+liveTableName+"'")) {
                    check(r.next(), "live structure identity"); liveTable=r.getLong(1); liveIndex=r.getLong(2);
                }
                boolean fullFailed=false;
                try (var files=Files.list(Path.of(args[6]))) {
                    var queries=files.filter(f -> f.toString().endsWith(".sql")).sorted().toList();
                    check(queries.size()==27, "frozen DataGrip query count: "+queries.size());
                    for (Path file : queries) {
                        try {
                            String sql=Files.readString(file);
                            for (int mode=0;mode<2;mode++) {
                                try (PreparedStatement p=c.prepareStatement(mode==0?sql.replace("?", Long.toString(namespace)):sql)) {
                                    if (mode==1) for (int parameter=1;parameter<=p.getParameterMetaData().getParameterCount();parameter++) p.setLong(parameter, namespace);
                                    for (int repeat=0;repeat<2;repeat++) { try (ResultSet r=p.executeQuery()) {
                                        int count=r.getMetaData().getColumnCount(); check(count>0, "empty Describe: "+file);
                                        boolean found=false;
                                        while (r.next()) {
                                            for (int col=1;col<=count;col++) r.getObject(col);
                                            String name=file.getFileName().toString();
                                            if (name.equals("1869280142.sql") && liveTableName.equals(r.getString("table_name"))) { check(r.getLong("table_id")>0, "real table OID"); found=true; }
                                            if (name.equals("1869280153.sql") && "PRIMARY".equals(r.getString("index_name"))) { check(r.getBoolean("is_primary") && r.getLong("index_id")>0, "real primary index"); found=true; }
                                            if (name.equals("1869280154.sql") && r.getLong("table_id")==liveTable && "p".equals(r.getString("con_kind"))) { check(r.getArray("con_columns")!=null && r.getLong("index_id")==liveIndex, "primary constraint columns/identity"); found=true; }
                                            if (name.equals("RetrieveColumns.sql") && r.getLong("table_id")==liveTable && "note".equals(r.getString("column_name"))) { check(r.getInt("column_position")==2 && "character varying(30)".equals(r.getString("type_spec")), "real column structure"); found=true; }
                                            if (name.equals("RetrieveIndexColumns.sql") && r.getLong("index_id")==liveIndex) { check(r.getInt("column_position")==1 && r.getBoolean("in_key"), "real index column"); found=true; }
                                        }
                                        String name=file.getFileName().toString();
                                        if (name.equals("1869280142.sql") || name.equals("1869280153.sql") || name.equals("1869280154.sql") || name.equals("RetrieveColumns.sql") || name.equals("RetrieveIndexColumns.sql")) check(found, "real structure missing: "+file);
                                    } }
                                }
                            }
                            System.out.println("full DataGrip SQL passed: "+file.getFileName());
                        } catch (SQLException | AssertionError e) {
                            fullFailed=true; System.out.println("full DataGrip SQL FAILED: "+file.getFileName()+": "+e);
                        }
                    }
                }
                try (PreparedStatement p=c.prepareStatement("INSERT INTO public."+liveTableName+" VALUES (?, ?)")) { p.setInt(1, 7); p.setNull(2, Types.VARCHAR); check(p.executeUpdate()==1, "prepared insert count"); }
                try (PreparedStatement p=c.prepareStatement("SELECT note FROM public."+liveTableName+" WHERE id=?")) { p.setInt(1,7); try(ResultSet r=p.executeQuery()) { check(r.next() && r.getString(1)==null && r.wasNull() && !r.next(), "prepared NULL row"); } }
                try (PreparedStatement p=c.prepareStatement("UPDATE public."+liveTableName+" SET note=? WHERE id=?")) { p.setString(1,"updated"); p.setInt(2,7); check(p.executeUpdate()==1, "prepared update count"); }
                try (PreparedStatement p=c.prepareStatement("SELECT note FROM public."+liveTableName+" WHERE id=?")) { p.setInt(1,7); try(ResultSet r=p.executeQuery()) { check(r.next() && r.getString(1).equals("updated") && !r.next(), "prepared update row"); } }
                try (PreparedStatement p=c.prepareStatement("DELETE FROM public."+liveTableName+" WHERE id=?")) { p.setInt(1,7); check(p.executeUpdate()==1, "prepared delete count"); }
                try (ResultSet r=s.executeQuery("SELECT id FROM public."+liveTableName)) { check(!r.next(), "delete row"); }
                try { s.executeQuery("SELECT oid FROM pg_catalog.pg_missing"); throw new AssertionError("missing catalog accepted"); } catch (SQLException e) { check("42P01".equals(e.getSQLState()), e.toString()); }
                try (ResultSet r=s.executeQuery("SELECT 1")) { check(r.next() && r.getInt(1)==1, "error recovery"); }
                check(!fullFailed, "full DataGrip SQL acceptance failed; see per-query evidence");
            } finally { s.execute("DROP TABLE public."+liveTableName); }
            System.out.println("JDBC original introspection Statement/PreparedStatement, oid/NULL/boundary, Describe/Parse-before-DDL, public CRUD, SQLSTATE 42P01 and recovery passed");
        }
    }
}
"#;

pub(super) fn mysql_read(socket: &mut TcpStream) -> Vec<u8> {
    let mut header = [0; 4];
    socket.read_exact(&mut header).unwrap();
    let length =
        usize::from(header[0]) | (usize::from(header[1]) << 8) | (usize::from(header[2]) << 16);
    assert!(length < 1 << 20);
    let mut body = vec![0; length];
    socket.read_exact(&mut body).unwrap();
    body
}
pub(super) fn mysql_packet(socket: &mut TcpStream, sequence: u8, body: &[u8]) {
    let length = body.len();
    socket
        .write_all(&[
            length as u8,
            (length >> 8) as u8,
            (length >> 16) as u8,
            sequence,
        ])
        .unwrap();
    socket.write_all(body).unwrap();
}
pub(super) fn mysql_handshake(socket: &mut TcpStream) {
    assert_eq!(mysql_read(socket).first(), Some(&10));
    let capabilities: u32 = (1 << 9) | (1 << 15) | (1 << 19);
    let mut response = capabilities.to_le_bytes().to_vec();
    response.extend_from_slice(&(64_u32 << 20).to_le_bytes());
    response.push(45);
    response.extend_from_slice(&[0; 23]);
    response.extend_from_slice(b"root\0\0mysql_native_password\0");
    mysql_packet(socket, 1, &response);
    assert_eq!(mysql_read(socket).first(), Some(&0));
}

// Use the same real MySQL connection while the independent PG listener is live.
// Native `public` remains an ordinary database, not the PG current-db mapping.
fn mysql_isolation(socket: &mut TcpStream) {
    for sql in [
        "CREATE DATABASE public",
        "CREATE TABLE public.client_isolation (id INT)",
        "INSERT INTO public.client_isolation VALUES (31)",
        "USE public",
    ] {
        assert!(mysql_query(socket, sql).is_empty(), "{sql}");
    }
    assert_eq!(
        mysql_query(socket, "SELECT id FROM client_isolation"),
        vec![vec![Some(b"31".to_vec())]]
    );
    mysql_query(socket, "BEGIN");
    mysql_query(socket, "UPDATE client_isolation SET id=32");
    mysql_query(socket, "ROLLBACK");
    assert_eq!(
        mysql_query(socket, "SELECT id FROM public.client_isolation"),
        vec![vec![Some(b"31".to_vec())]]
    );
    mysql_query(socket, "USE test");
    assert_eq!(
        mysql_query(socket, "SELECT id FROM public.client_isolation"),
        vec![vec![Some(b"31".to_vec())]]
    );
    mysql_packet(socket, 0, b"\x02public"); // COM_INIT_DB
    assert_eq!(mysql_read(socket)[0], 0);
    mysql_query(socket, "BEGIN");
    mysql_query(socket, "UPDATE client_isolation SET id=33");
    mysql_query(socket, "COMMIT");
    assert_eq!(
        mysql_query(socket, "SELECT id FROM client_isolation"),
        vec![vec![Some(b"33".to_vec())]]
    );
    mysql_query(socket, "UPDATE client_isolation SET id=31");
    let mut request = vec![0x16]; // COM_STMT_PREPARE
    request.extend_from_slice(b"SELECT id FROM public.client_isolation WHERE id=?");
    mysql_packet(socket, 0, &request);
    let prepared = mysql_read(socket);
    assert_eq!(prepared[0], 0, "{prepared:?}");
    let id = &prepared[1..5];
    let columns = u16::from_le_bytes(prepared[5..7].try_into().unwrap());
    let parameters = u16::from_le_bytes(prepared[7..9].try_into().unwrap());
    assert_eq!((columns, parameters), (1, 1));
    for count in [parameters, columns] {
        for _ in 0..count {
            mysql_read(socket);
        }
        assert_eq!(mysql_read(socket)[0], 0xfe);
    }
    let mut execute = vec![0x17]; // COM_STMT_EXECUTE with a real integer parameter
    execute.extend_from_slice(id);
    execute.extend_from_slice(&[0, 1, 0, 0, 0, 0, 1, 3, 0]);
    execute.extend_from_slice(&31_i32.to_le_bytes());
    mysql_packet(socket, 0, &execute);
    assert_eq!(mysql_read(socket), vec![1]);
    mysql_read(socket); // column definition
    assert_eq!(mysql_read(socket)[0], 0xfe);
    let row = mysql_read(socket);
    assert_eq!(&row[..2], &[0, 0]); // binary row, no NULL columns
    assert_eq!(i32::from_le_bytes(row[2..6].try_into().unwrap()), 31);
    assert_eq!(mysql_read(socket)[0], 0xfe);
    let mut close = vec![0x19];
    close.extend_from_slice(id);
    mysql_packet(socket, 0, &close);
    println!(
        "MySQL native public database, USE, rollback, real relation rows and prepared integer parameter passed"
    );
}

pub(super) fn mysql_query(socket: &mut TcpStream, sql: &str) -> Vec<Vec<Option<Vec<u8>>>> {
    let mut packet = vec![3];
    packet.extend_from_slice(sql.as_bytes());
    mysql_packet(socket, 0, &packet);
    let head = mysql_read(socket);
    assert_ne!(head[0], 0xff, "{sql}: {}", String::from_utf8_lossy(&head));
    if head[0] == 0 {
        return Vec::new();
    }
    let count = usize::from(head[0]);
    assert!(count < 251);
    for _ in 0..count {
        mysql_read(socket);
    }
    assert_eq!(mysql_read(socket)[0], 0xfe);
    let mut rows = Vec::new();
    loop {
        let row = mysql_read(socket);
        if row[0] == 0xfe && row.len() < 9 {
            break;
        }
        assert_ne!(row[0], 0xff, "{sql}: {row:?}");
        let mut offset = 0;
        let values = (0..count)
            .map(|_| {
                let length = row[offset];
                offset += 1;
                if length == 251 {
                    None
                } else {
                    assert!(length < 251);
                    let value = row[offset..offset + usize::from(length)].to_vec();
                    offset += usize::from(length);
                    Some(value)
                }
            })
            .collect();
        assert_eq!(offset, row.len());
        rows.push(values);
    }
    rows
}

// DataGrip 2025.1.3 database log, 2026-10-03 08:45:58, session 1533977248.
// Keep JDBC display SQL verbatim; the client workflow converts the sole ?
// to a literal for Query or $1 for Parse, without changing inactive comments.
// Statement 1869279758.
const DATAGRIP_VIEW_SOURCES_SQL: &str = r#"select
       T.relkind as view_kind,
       T.oid as view_id,
       pg_catalog.pg_get_viewdef(T.oid, true) as source_text
from pg_catalog.pg_class T
  join pg_catalog.pg_namespace N on T.relnamespace = N.oid
where N.oid = ?::oid
  and T.relkind in ('m','v')
  --  and T.relname in ( :[*f_names] )
  --  and (pg_catalog.age(T.xmin) <= #SRCTXAGE or exists(
  --  select A.attrelid from pg_catalog.pg_attribute A where A.attrelid = T.oid and pg_catalog.age(A.xmin) <= #SRCTXAGE))
"#;

// Statement 1869279759.
const DATAGRIP_FUNCTION_SOURCES_SQL: &str = r#"with system_languages as ( select oid as lang
                           from pg_catalog.pg_language
                           where lanname in ('c','internal') )
select oid as id,
       pg_catalog.pg_get_function_arguments(oid) as arguments_def,
       pg_catalog.pg_get_function_result(oid) as result_def,
       pg_catalog.pg_get_function_sqlbody(oid) /* null */ as sqlbody_def,
       prosrc as source_text
from pg_catalog.pg_proc
where pronamespace = ?::oid
  --  and pg_proc.proname in ( :[*f_names] )
  --  and pg_catalog.age(xmin) <= #SRCTXAGE
  and not (prokind = 'a') /* proisagg */
  and prolang not in (select lang from system_languages)
  and prosrc is not null
"#;

// Statement 1869279760.
const DATAGRIP_RELATIONS_SQL: &str = r#"select D.objid as dependent_id,
       D.refobjid as owner_id,
       D.refobjsubid as owner_subobject_id
from pg_depend D
  join pg_class C_SEQ on D.objid    = C_SEQ.oid and D.classid    = 'pg_class'::regclass::oid
  join pg_class C_TAB on D.refobjid = C_TAB.oid and D.refclassid = 'pg_class'::regclass::oid
where C_SEQ.relkind = 'S'
  and C_TAB.relkind = 'r'
  and D.refobjsubid <> 0
  and (D.deptype = 'a' or D.deptype = 'i')
  and C_TAB.relnamespace = ?::oid
order by owner_id
"#;

const LIBPQ_WORKFLOW: &str = r#"
import ctypes as c, ctypes.util, os, sys
library = os.environ.get('PG_LIBPQ_LIBRARY')
if not library:
    library = '/opt/homebrew/opt/libpq/lib/libpq.dylib' if sys.platform == 'darwin' else ctypes.util.find_library('pq')
assert library, 'install PostgreSQL libpq 18 or set PG_LIBPQ_LIBRARY'
pq = c.CDLL(library)
def api(name, result, *args):
    fn = getattr(pq, name)
    fn.restype, fn.argtypes = result, args
    return fn
ptr, text, integer = c.c_void_p, c.c_char_p, c.c_int
version = api('PQlibVersion', integer)
assert version() >= 180000, 'protocol 3.2 requires libpq 18'
print('external PostgreSQL libpq version:', version(), flush=True)
connect = api('PQconnectdb', ptr, text)
status = api('PQstatus', integer, ptr)
error = api('PQerrorMessage', text, ptr)
finish = api('PQfinish', None, ptr)
protocol = api('PQfullProtocolVersion', integer, ptr)
parameter_status = api('PQparameterStatus', text, ptr, text)
server_version = api('PQserverVersion', integer, ptr)
execute = api('PQexec', ptr, ptr, text)
params = api('PQexecParams', ptr, ptr, text, integer, c.POINTER(c.c_uint), c.POINTER(text), c.POINTER(integer), c.POINTER(integer), integer)
close_prepared = api('PQclosePrepared', ptr, ptr, text)
prepare = api('PQprepare', ptr, ptr, text, text, integer, c.POINTER(c.c_uint))
describe = api('PQdescribePrepared', ptr, ptr, text)
prepared_execute = api('PQexecPrepared', ptr, ptr, text, integer, c.POINTER(text), c.POINTER(integer), c.POINTER(integer), integer)
result_status = api('PQresultStatus', integer, ptr)
result_error = api('PQresultErrorMessage', text, ptr)
rows = api('PQntuples', integer, ptr)
columns = api('PQnfields', integer, ptr)
value = api('PQgetvalue', text, ptr, integer, integer)
is_null = api('PQgetisnull', integer, ptr, integer, integer)
field_name = api('PQfname', text, ptr, integer)
field_type = api('PQftype', c.c_uint, ptr, integer)
error_field = api('PQresultErrorField', text, ptr, integer)
clear = api('PQclear', None, ptr)
txn = api('PQtransactionStatus', integer, ptr)
cancel_create = api('PQcancelCreate', ptr, ptr)
cancel_blocking = api('PQcancelBlocking', integer, ptr)
cancel_finish = api('PQcancelFinish', None, ptr)
base_conninfo = f'host=127.0.0.1 port={sys.argv[1]} user=root dbname={sys.argv[6]} sslmode=disable gssencmode=disable connect_timeout=5'
for options, expected_protocol in [('', 30000), (' min_protocol_version=3.2 max_protocol_version=3.2', 30002)]:
    conninfo = base_conninfo + options
    conn = connect(conninfo.encode())
    assert conn
    try:
        assert status(conn) == 0, error(conn).decode()
        assert protocol(conn) == expected_protocol, protocol(conn)
        assert parameter_status(conn, b'server_version') == b'18.0 (AsterSQL)'
        assert server_version(conn) == 180000, server_version(conn)
        def query(sql, expected=None, parameter=None, metadata=None, extended=False, sqlstate=None, parameter_oid=23, null_parameter=False, binary_parameter=False):
            if extended:
                result = params(conn, sql.encode(), 0, None, None, None, None, 0)
            elif parameter is None and not null_parameter:
                result = execute(conn, sql.encode())
            else:
                oids = (c.c_uint * 1)(parameter_oid)
                payload = None if null_parameter else (int(parameter).to_bytes(4, 'big') if binary_parameter else str(parameter).encode())
                values = (text * 1)(payload)
                lengths = (integer * 1)(0 if payload is None else len(payload))
                formats = (integer * 1)(int(binary_parameter))
                result = params(conn, sql.encode(), 1, oids, values, lengths, formats, 0)
            assert result
            try:
                if sqlstate is not None:
                    assert result_status(result) == 7, result_status(result)
                    assert error_field(result, ord('C')) == sqlstate.encode(), result_error(result).decode()
                    return
                assert result_status(result) in (1, 2), result_error(result).decode()
                if metadata is not None:
                    actual_metadata = [(field_name(result, col).decode(), field_type(result, col)) for col in range(columns(result))]
                    assert actual_metadata == metadata, (sql, actual_metadata, metadata)
                actual = [[None if is_null(result, r, col) else value(result, r, col).decode() for col in range(columns(result))] for r in range(rows(result))]
                if expected is not None:
                    assert actual == expected, (sql, actual, expected)
                return actual
            finally:
                clear(result)
        for invalid_info in [conninfo.replace('user=root', 'user=intruder')]:
            invalid = connect(invalid_info.encode())
            try:
                assert status(invalid) != 0, 'invalid identity must be rejected'
            finally:
                finish(invalid)
        query('SELECT 1', [['1']])
        query('SELECT id FROM public.client_isolation', [['17']])
        # Source SQL remains frozen. Functions have no native stored-program rows;
        # native independent sequences have no auto/internal column ownership.
        # Use the real current namespace ID; no catalog rows are mocked.
        namespace_id = int(query("select oid from pg_catalog.pg_namespace where nspname = 'public'")[0][0])
        for label, displayed, simple_state, parse_state in zip(
                ['RetrieveViewSources', 'RetrieveFunctionSources', 'RetrieveRelations'],
                sys.argv[3:6], [None, None, None], [None, None, None]):
            assert displayed.count('?') == 1, label
            source_metadata = {
                'RetrieveViewSources': [('view_kind', 25), ('view_id', 26), ('source_text', 25)],
                'RetrieveFunctionSources': [('id', 26), ('arguments_def', 25), ('result_def', 25), ('sqlbody_def', 25), ('source_text', 25)],
                'RetrieveRelations': [('dependent_id', 26), ('owner_id', 26), ('owner_subobject_id', 23)],
            }.get(label)
            source_rows = None
            if label == 'RetrieveViewSources':
                view_oid = query("select oid from pg_class where relname = 'client_view_live'")[0][0]
                source_rows = [['v', view_oid, sys.argv[7]]]
            query(displayed.replace('?', str(namespace_id)), sqlstate=simple_state,
                  expected=source_rows if source_rows is not None else [] if source_metadata else None, metadata=source_metadata)
            query('SELECT 1', [['1']])
            query(displayed.replace('?', '$1'), parameter=namespace_id,
                  parameter_oid=26, sqlstate=parse_state,
                  expected=source_rows if source_rows is not None else [] if source_metadata else None, metadata=source_metadata)
            query('SELECT 1', [['1']])
            query(displayed.replace('?', '$1'), parameter_oid=26, null_parameter=True, expected=[], metadata=source_metadata)
            query(displayed.replace('?', '$1'), parameter=4294967295, parameter_oid=26, expected=[], metadata=source_metadata)
            query(displayed.replace('?', '$1'), parameter=namespace_id, parameter_oid=26, binary_parameter=True,
                  expected=source_rows if source_rows is not None else [], metadata=source_metadata)
            query(displayed.replace('?', '$1'), parameter_oid=26, null_parameter=True, binary_parameter=True, expected=[], metadata=source_metadata)
            query(displayed.replace('?', '$1'), parameter=4294967295, parameter_oid=26, binary_parameter=True, expected=[], metadata=source_metadata)
            print(f'{expected_protocol}: {label}: Query/Execute(oid 26)={source_rows if source_rows is not None else "typed empty result"}; recovery passed', flush=True)
        for extended in [False, True]:
            for expression in ['pg_get_viewdef(NULL, true)', 'pg_get_viewdef(4294967295::oid, true)', 'pg_get_viewdef(oid, NULL)']:
                query(f"SELECT {expression} AS source_text FROM pg_class WHERE relname='client_view_live'", [[None]], metadata=[('source_text',25)], extended=extended)
            query("SELECT pg_get_viewdef(oid, 'wrong') FROM pg_class", sqlstate='0A000', extended=extended)
            query('SELECT 1', [['1']], extended=extended)
        # PQprepare performs Parse; PQdescribePrepared checks Statement Describe.
        # Keep this statement over DDL and read live rows at each Execute.
        ddl_sql = b"SELECT relname FROM pg_class WHERE relname='pg_parse_ddl_live'"
        result = prepare(conn, b'live_catalog', ddl_sql, 0, None)
        try: assert result_status(result)==1, result_error(result).decode()
        finally: clear(result)
        result = describe(conn, b'live_catalog')
        try:
            assert result_status(result)==1, result_error(result).decode()
            assert columns(result)==1 and field_name(result,0)==b'relname' and field_type(result,0)==25
        finally: clear(result)
        def live_rows():
            result = prepared_execute(conn,b'live_catalog',0,None,None,None,0)
            try:
                assert result_status(result)==2, result_error(result).decode()
                return [[value(result,r,col).decode() for col in range(columns(result))] for r in range(rows(result))]
            finally: clear(result)
        assert live_rows()==[]
        query('CREATE TABLE public.pg_parse_ddl_live (id INT)')
        try: assert live_rows()==[['pg_parse_ddl_live']]
        finally: query('DROP TABLE public.pg_parse_ddl_live')
        assert live_rows()==[]
        result = close_prepared(conn, b'live_catalog')
        try: assert result_status(result)==1, result_error(result).decode()
        finally: clear(result)
        print(f'{expected_protocol}: NULL/boundary view definitions, oid parameters, Describe and Parse-before-DDL live Execute passed', flush=True)
        namespace_sql = """select N.oid::bigint as id, N.xmin as state_number, nspname as name,
            D.description, pg_catalog.pg_get_userbyid(N.nspowner) as "owner"
            from pg_catalog.pg_namespace N left join pg_catalog.pg_description D on N.oid = D.objoid
            order by case when nspname = pg_catalog.current_schema() then -1::bigint else N.oid::bigint end"""
        tablespace_sql = 'SELECT oid::bigint AS id, spcname AS name, pg_catalog.pg_get_userbyid(spcowner) AS "owner", spcacl, spcoptions FROM pg_catalog.pg_tablespace ORDER BY oid'
        query('CREATE DATABASE pg_client_catalog_live')
        query('CREATE TABLE public.pg_client_relation_live (id INT)')
        for extended in [False, True]:
            relations = query("SELECT oid, relname, relnamespace, relkind FROM pg_class WHERE relname = 'pg_client_relation_live'", metadata=[('oid', 26), ('relname', 25), ('relnamespace', 26), ('relkind', 25)], extended=extended)
            assert len(relations) == 1 and relations[0][1:] == ['pg_client_relation_live', str(namespace_id), 'r'], relations
            assert int(relations[0][0]) > 0, relations
            databases = query(sys.argv[2], metadata=[('id', 20), ('name', 25), ('description', 25), ('is_template', 16), ('allow_connections', 16), ('owner', 25)], extended=extended)
            assert databases[0][1] == sys.argv[6], databases
            assert any(r[1] == 'pg_client_catalog_live' for r in databases), databases
            assert all(int(r[0]) != 0 and r[2:] == [None, 'f', 't', None] for r in databases), databases
            assert [int(r[0]) for r in databases[1:]] == sorted(int(r[0]) for r in databases[1:]), databases
            namespaces = query(namespace_sql, metadata=[('id', 20), ('state_number', 20), ('name', 25), ('description', 25), ('owner', 25)], extended=extended)
            assert namespaces[0][2] == 'public', namespaces
            assert {r[2] for r in namespaces} == {'public', 'pg_catalog'}, namespaces
            assert all(int(r[0]) > 0 and r[1] is None and r[3:] == [None, None] for r in namespaces), namespaces
            assert [int(r[0]) for r in namespaces[1:]] == sorted(int(r[0]) for r in namespaces[1:]), namespaces
            assert len({r[0] for r in namespaces}) == len(namespaces)
            query(tablespace_sql, [], metadata=[('id', 20), ('name', 25), ('owner', 25), ('spcacl', 25), ('spcoptions', 25)], extended=extended)
            # Extracted from DataGrip PgIntroQueries.sql: the ID-only probe is
            # supported; full introspection requires features beyond this phase.
            query('select oid::bigint from pg_catalog.pg_tablespace', [], metadata=[('oid', 20)], extended=extended)
            full_tablespace_sql = 'select T.oid::bigint as id, T.spcname as name, T.xmin as state_number, pg_catalog.pg_get_userbyid(T.spcowner) as owner, pg_catalog.pg_tablespace_location(T.oid) as location, T.spcoptions as options, D.description as comment from pg_catalog.pg_tablespace T left join pg_catalog.pg_shdescription D on D.objoid = T.oid'
            query(full_tablespace_sql, extended=extended, sqlstate='0A000')
            query('SELECT 1', [['1']], extended=extended)
            for sql, state in [('SELECT oid FROM pg_catalog.pg_missing', '42P01'), ('SELECT oid FROM pg_catalog.pg_namespace GROUP BY oid', '0A000'), ('SELECT (', '42601')]:
                query(sql, extended=extended, sqlstate=state)
                query('SELECT 1', [['1']], extended=extended)
        query('DROP TABLE public.pg_client_relation_live')
        query('DROP DATABASE pg_client_catalog_live')
        assert not any(r[1] == 'pg_client_catalog_live' for r in query(sys.argv[2]))
        query('CREATE TABLE pg_real_client (id INT PRIMARY KEY, v VARCHAR(30))')
        query("INSERT INTO pg_real_client VALUES (1, 'one')")
        query("UPDATE pg_real_client SET v = 'two' WHERE id = 1")
        query('SELECT v FROM pg_real_client WHERE id = $1', [['two']], parameter=1)
        query('BEGIN')
        assert txn(conn) == 2
        query("INSERT INTO pg_real_client VALUES (2, 'committed')")
        query('COMMIT')
        assert txn(conn) == 0
        query('BEGIN')
        query("UPDATE pg_real_client SET v = 'rolled back' WHERE id = 1")
        query('ROLLBACK')
        assert txn(conn) == 0
        query('SELECT v FROM pg_real_client ORDER BY id', [['two'], ['committed']])
        query('BEGIN')
        query("INSERT INTO pg_real_client VALUES (1, 'duplicate')", sqlstate='23505')
        assert txn(conn) == 3
        query('SELECT 1', sqlstate='25P02')
        query('COMMIT', sqlstate='25P02')
        assert txn(conn) == 3
        query('ROLLBACK')
        assert txn(conn) == 0
        query('SELECT v FROM pg_real_client ORDER BY id', [['two'], ['committed']])
        query('BEGIN')
        query('CREATE TABLE pg_transactional_ddl (id INT)', sqlstate='0A000')
        assert txn(conn) == 3
        query('ROLLBACK')
        assert txn(conn) == 0
        query('DELETE FROM pg_real_client WHERE id = 2')
        query('SELECT id FROM pg_real_client', [['1']])
        # libpq owns the version-specific BackendKeyData/CancelRequest framing.
        # An idle cancel must complete without poisoning the next query.
        cancel = cancel_create(conn)
        assert cancel
        try:
            assert cancel_blocking(cancel) == 1
        finally:
            cancel_finish(cancel)
        query('SELECT 1', [['1']])
        query('DROP TABLE pg_real_client')
        print(f'{expected_protocol}: startup, catalogs (simple/extended metadata, NULL, rows, ordering), error recovery, CRUD, typed parameters, I/T/E transactions and idle cancel passed', flush=True)
    finally:
        finish(conn)
"#;

#[test]
fn pg_dialect_01_libpq() {
    pg_dialect_libpq(1);
}

#[test]
fn pg_dialect_02_libpq() {
    pg_dialect_libpq(2);
}

#[test]
fn pg_dialect_03_libpq() {
    pg_dialect_libpq(3);
}

#[test]
fn pg_dialect_04_libpq() {
    pg_dialect_libpq(4);
}

#[test]
fn pg_dialect_09_libpq() {
    pg_dialect_libpq(9);
}

#[test]
fn pg_dialect_11_libpq() {
    pg_dialect_libpq(11);
}

fn pg_dialect_libpq(task: u8) {
    use std::io::BufRead;
    use std::process::Stdio;
    assert!(std::env::var_os("PG18_REFERENCE_DSN").is_some());
    assert!(std::env::var_os("PG_CORE_REFERENCE_BUILD_FILE").is_some());
    assert!(std::env::var_os("MYSQL_REFERENCE_DSN").is_some());
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain, Some(0));
    let _close = CloseServer(&server);
    let address = server.postgres_listener_addr().unwrap();
    let mysql_address = server.listener_addr().unwrap();
    let database = format!(
        "pg_dialect_{task:02}_{}_{}",
        std::process::id(),
        address.port()
    );
    let run = format!("{}-{}", std::process::id(), address.port());
    let evidence = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../target/pg-dialect-{task:02}-evidence"))
        .join(run);
    std::fs::create_dir_all(&evidence).unwrap();
    println!("PG_DIALECT_EVIDENCE_DIR={}", evidence.display());
    let executable = std::env::current_exe().unwrap();
    let build = serde_json::json!({"path": executable, "profile": "cargo-test-default-features"});
    std::fs::write(evidence.join("rust-build.json"), build.to_string()).unwrap();
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let cleanup_guard = Cleanup(|| {
        server.close();
        let dropped = native.execute(&format!("DROP DATABASE {database}")).is_ok();
        let cleanup = serde_json::json!({"database_dropped": dropped,
            "postgres_port_closed": TcpStream::connect(address).is_err(),
            "mysql_port_closed": TcpStream::connect(mysql_address).is_err()});
        let _ = std::fs::write(evidence.join("rust-cleanup.json"), cleanup.to_string());
    });
    native
        .execute(&format!("CREATE DATABASE {database}"))
        .unwrap();
    let mut mysql = TcpStream::connect(mysql_address).unwrap();
    mysql
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    mysql_handshake(&mut mysql);
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            drop(self.0.stdin.take());
            let _ = self.0.wait();
        }
    }
    let mut child = ChildGuard(
        Command::new("python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/testdata/pg_dialect_libpq.py"
            ))
            .args([
                task.to_string(),
                address.port().to_string(),
                database.clone(),
            ])
            .env("PG_DIALECT_EVIDENCE_DIR", &evidence)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    fn column_type(column: &[u8]) -> u8 {
        let mut offset = 0;
        for _ in 0..6 {
            let length = usize::from(column[offset]);
            assert!(length < 251);
            offset += length + 1;
        }
        assert_eq!(column[offset], 12);
        column[offset + 7]
    }
    let checkpoint = |socket: &mut TcpStream, phase: &str| {
        if task == 3 {
            for (mode, expected) in [
                ("", b"1".as_slice()),
                ("HIGH_NOT_PRECEDENCE", b"0".as_slice()),
            ] {
                mysql_query(socket, &format!("SET SESSION sql_mode = '{mode}'"));
                assert_eq!(
                    mysql_query(socket, "SELECT NOT 'Ab' LIKE 'x%'"),
                    vec![vec![Some(expected.to_vec())]]
                );
            }
            mysql_query(socket, "SET SESSION sql_mode = ''");
        }
        let sql = format!("SELECT id AS v FROM {database}.dialect_isolation WHERE id=31");
        let mut request = vec![3];
        request.extend_from_slice(sql.as_bytes());
        mysql_packet(socket, 0, &request);
        assert_eq!(mysql_read(socket), vec![1]);
        let column = mysql_read(socket);
        assert_eq!(column_type(&column), 3);
        assert_eq!(mysql_read(socket)[0], 0xfe);
        let row = mysql_read(socket);
        assert_eq!(&row, b"\x0231");
        assert_eq!(mysql_read(socket)[0], 0xfe);
        let mut request = vec![0x16];
        request.extend_from_slice(
            format!("SELECT id AS v FROM {database}.dialect_isolation WHERE id=?").as_bytes(),
        );
        mysql_packet(socket, 0, &request);
        let prepared = mysql_read(socket);
        assert_eq!(prepared[0], 0);
        let id = &prepared[1..5];
        assert_eq!(u16::from_le_bytes(prepared[5..7].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(prepared[7..9].try_into().unwrap()), 1);
        let parameter = mysql_read(socket);
        assert_eq!(mysql_read(socket)[0], 0xfe);
        let prepared_column = mysql_read(socket);
        assert_eq!(column_type(&prepared_column), 3);
        assert_eq!(mysql_read(socket)[0], 0xfe);
        let mut request = vec![0x17];
        request.extend_from_slice(id);
        request.extend_from_slice(&[0, 1, 0, 0, 0, 0, 1, 3, 0]);
        request.extend_from_slice(&31_i32.to_le_bytes());
        mysql_packet(socket, 0, &request);
        assert_eq!(mysql_read(socket), vec![1]);
        let executed_column = mysql_read(socket);
        assert_eq!(column_type(&executed_column), 3);
        assert_eq!(mysql_read(socket)[0], 0xfe);
        let binary = mysql_read(socket);
        assert_eq!(&binary[..2], &[0, 0]);
        assert_eq!(i32::from_le_bytes(binary[2..6].try_into().unwrap()), 31);
        assert_eq!(mysql_read(socket)[0], 0xfe);
        let mut close = vec![0x19];
        close.extend_from_slice(id);
        mysql_packet(socket, 0, &close);
        let record = serde_json::json!({"run": evidence.file_name().unwrap().to_str().unwrap(),
            "case_id": phase, "side": "mysql", "attempt": 1,
            "input": {"command": sql}, "observation": {"column": column, "text_row": row,
                "parameter": parameter, "prepared_column": prepared_column,
                "binary_row": binary, "executed_column": executed_column}, "outcome": "bypass_health"});
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(evidence.join("mysql-local.jsonl"))
            .unwrap();
        writeln!(file, "{record}").unwrap();
        file.sync_all().unwrap();
    };
    mysql_query(&mut mysql, "SET SESSION sql_mode = ''");
    mysql_query(&mut mysql, "SET NAMES utf8mb4 COLLATE utf8mb4_general_ci");
    let environment = mysql_query(
        &mut mysql,
        "SELECT @@sql_mode, @@character_set_connection, @@collation_connection",
    );
    assert_eq!(
        environment,
        vec![vec![
            Some(b"".to_vec()),
            Some(b"utf8mb4".to_vec()),
            Some(b"utf8mb4_general_ci".to_vec())
        ]]
    );
    std::fs::write(
        evidence.join("mysql-local-environment.json"),
        serde_json::to_string(&environment).unwrap(),
    )
    .unwrap();
    let baseline = mysql_query(&mut mysql, "SELECT 'Ab' ILIKE 'a%'");
    std::fs::write(
        evidence.join("mysql-legacy-extension.json"),
        serde_json::to_string(&baseline).unwrap(),
    )
    .unwrap();
    mysql_query(
        &mut mysql,
        &format!("CREATE TABLE {database}.dialect_isolation (id INT)"),
    );
    mysql_query(
        &mut mysql,
        &format!("INSERT INTO {database}.dialect_isolation VALUES (31)"),
    );
    if task == 4 {
        // Observe the existing MySQL extension inside a rolled-back transaction.
        // Keep its acceptance distinct from the official MySQL reference rejection.
        mysql_query(&mut mysql, "BEGIN");
        let baseline_sql =
            format!("INSERT INTO {database}.dialect_isolation VALUES (47) RETURNING id");
        let returned = mysql_query(&mut mysql, &baseline_sql);
        let stored = mysql_query(
            &mut mysql,
            &format!("SELECT id FROM {database}.dialect_isolation ORDER BY id"),
        );
        assert_eq!(
            stored,
            vec![vec![Some(b"31".to_vec())], vec![Some(b"47".to_vec())]]
        );
        mysql_query(&mut mysql, "ROLLBACK");
        std::fs::write(
            evidence.join("mysql-returning-baseline.json"),
            serde_json::json!({"sql": baseline_sql, "accepted": true,
                "returned": returned, "stored_before_rollback": stored,
                "rollback": true})
            .to_string(),
        )
        .unwrap();
    }
    if task == 9 {
        for sql in [
            "SELECT ARRAY[1,2]",
            "SELECT ARRAY[]",
            "SELECT ARRAY[NULL,1]",
            "SELECT ARRAY[$2,$1,$2]",
            "SELECT ARRAY[1,2",
        ] {
            let mut request = vec![3];
            request.extend_from_slice(sql.as_bytes());
            mysql_packet(&mut mysql, 0, &request);
            let response = mysql_read(&mut mysql);
            assert_eq!(response[0], 0xff);
            assert_eq!(u16::from_le_bytes(response[1..3].try_into().unwrap()), 1064);
            assert_eq!(&response[4..9], b"42000");
            let record = serde_json::json!({"sql": sql, "error_code":1064, "sqlstate":"42000", "packet":response});
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(evidence.join("mysql-array-isolation.jsonl"))
                .unwrap();
            writeln!(file, "{record}").unwrap();
        }
        assert_eq!(
            mysql_query(&mut mysql, "SELECT 'ARRAY[1,2]' /* ARRAY[] */"),
            vec![vec![Some(b"ARRAY[1,2]".to_vec())]]
        );
    }
    if task == 11 {
        let mut baseline = vec![];
        for sql in [
            "SELECT ROW(1,2)",
            "SELECT ROW(1)",
            "SELECT ROW(1,ROW(2,3))",
            "SELECT ROW(1,",
        ] {
            let mut request = vec![3];
            request.extend_from_slice(sql.as_bytes());
            mysql_packet(&mut mysql, 0, &request);
            let response = mysql_read(&mut mysql);
            assert_eq!(
                response[0], 0xff,
                "freeze existing MySQL projection/syntax errors"
            );
            let expected = if sql == "SELECT ROW(1)" || sql == "SELECT ROW(1," {
                (1064, b"42000".as_slice())
            } else {
                (1105, b"HY000".as_slice())
            };
            assert_eq!(
                u16::from_le_bytes(response[1..3].try_into().unwrap()),
                expected.0
            );
            assert_eq!(&response[4..9], expected.1);
            baseline.push(serde_json::json!({"sql":sql, "error_code":u16::from_le_bytes(response[1..3].try_into().unwrap()), "sqlstate":String::from_utf8_lossy(&response[4..9]), "packet":response}));
        }
        std::fs::write(
            evidence.join("mysql-row-baseline.json"),
            serde_json::to_string(&baseline).unwrap(),
        )
        .unwrap();
        assert_eq!(
            mysql_query(&mut mysql, "SELECT ROW(1,2)=ROW(1,2)"),
            vec![vec![Some(b"1".to_vec())]]
        );
        assert_eq!(
            mysql_query(&mut mysql, "SELECT 'ROW(1,2)' /* ROW() */"),
            vec![vec![Some(b"ROW(1,2)".to_vec())]]
        );
    }
    checkpoint(&mut mysql, "before");
    let mut checkpoints = 0;
    for line in std::io::BufReader::new(child.0.stdout.take().unwrap()).lines() {
        let line = line.unwrap();
        println!("{line}");
        if line.starts_with("PG_DIALECT_CHECKPOINT ") {
            checkpoint(&mut mysql, &line);
            checkpoints += 1;
            child
                .0
                .stdin
                .as_mut()
                .unwrap()
                .write_all(b"continue\n")
                .unwrap();
        }
    }
    let status = child.0.wait().unwrap();
    std::fs::write(
        evidence.join("child-exit.json"),
        serde_json::json!({"success": status.success(), "code": status.code()}).to_string(),
    )
    .unwrap();
    drop(child);
    checkpoint(&mut mysql, "after");
    mysql_packet(&mut mysql, 0, &[1]);
    drop(cleanup_guard);
    let cleanup: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(evidence.join("rust-cleanup.json")).unwrap())
            .unwrap();
    assert_eq!(cleanup["database_dropped"], true);
    assert_eq!(cleanup["postgres_port_closed"], true);
    assert_eq!(cleanup["mysql_port_closed"], true);
    assert!(status.success(), "dialect handoff failed; see evidence");
    if task == 11 {
        assert_eq!(checkpoints, 6);
        let summary: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(evidence.join("summary.json")).unwrap())
                .unwrap();
        assert_eq!(summary["cleanup_ok"], true);
        assert_eq!(summary["syntax_eligible"], 6);
        assert_eq!(summary["gate_cases"], 4);
        assert_eq!(summary["semantic_row_success"], 0);
        assert!((1..=2).contains(&summary["unique_effective"].as_u64().unwrap()));
        assert_eq!(
            std::fs::read_to_string(evidence.join("mysql-local.jsonl"))
                .unwrap()
                .lines()
                .count(),
            8
        );
        return;
    }
    if task == 9 {
        assert_eq!(checkpoints, 6);
        let summary: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(evidence.join("summary.json")).unwrap())
                .unwrap();
        assert_eq!(summary["cleanup_ok"], true);
        assert_eq!(summary["syntax_eligible"], 6);
        assert_eq!(summary["gate_cases"], 4);
        assert_eq!(summary["semantic_array_success"], 0);
        assert_eq!(summary["unique_effective"], 2);
        assert_eq!(
            std::fs::read_to_string(evidence.join("mysql-local.jsonl"))
                .unwrap()
                .lines()
                .count(),
            8
        );
        return;
    }
    assert_eq!(checkpoints, if task == 4 { 8 } else { 10 });
    let summary_path = evidence.join("summary.json");
    let mut summary: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&summary_path).unwrap()).unwrap();
    assert_eq!(summary["cleanup_ok"], true);
    assert_eq!(summary["syntax_eligible"], if task == 4 { 8 } else { 10 });
    assert_eq!(
        summary["self_checks"],
        if task == 1 {
            15
        } else if task == 2 {
            14
        } else if task == 3 {
            13
        } else {
            17
        }
    );
    assert_eq!(
        std::fs::read_to_string(evidence.join("mysql-local.jsonl"))
            .unwrap()
            .lines()
            .count(),
        if task == 4 { 10 } else { 12 }
    );
    if task == 4 {
        // Gate evidence is distinct from RETURNING result semantics.
        assert_eq!(summary["gate_cases"], 4);
        assert_eq!(summary["unique_effective"], 4);
        assert_eq!(summary["semantic_returning_success"], 0);
    } else {
        summary["unique_effective"] = 10.into();
    }
    std::fs::write(summary_path, summary.to_string()).unwrap();
}
