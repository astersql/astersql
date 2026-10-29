# Copyright 2026 AsterSQL.
"""Task 08.07 interval EXTRACT observations. Reference credentials are never archived."""
import ctypes as c
import hashlib
import json
import os
import signal
from pathlib import Path
import struct
import subprocess
import sys
from pg_function_23_libpq import Libpq, compare, self_test
from pg_function_27_libpq import Mysql
from pg_function_25_libpq import mysql_compare


CASE_IDS = ('I-before', 'I-middle', 'I-after', 'C-Q-year', 'C-Q-month', 'C-Q-day', 'C-Q-hour', 'C-Q-second', 'C-Q-minute', 'C-Q-negative-year', 'C-Q-negative-month', 'C-Q-negative-day', 'C-Q-negative-hour', 'C-Q-negative-second', 'C-Q-negative-fraction', 'C-Q-long-hour', 'C-Q-microseconds', 'C-Q-milliseconds', 'C-Q-null', 'C-Q-zero', 'C-Q-nested', 'C-Q-row', 'C-Q-row-nested', 'C-Q-row-zero', 'C-E-aster_bad_unit', 'C-E-timezone', 'C-recover-aster_bad_unit', 'C-recover-timezone', 'C-txn-aster_bad_unit', 'C-txn-timezone', 'C-bind-0-0-0-0', 'C-bind-0-0-0-1', 'C-bind-0-0-0-2', 'C-bind-0-0-1-0', 'C-bind-0-0-1-1', 'C-bind-0-0-1-2', 'C-bind-0-1-0-0', 'C-bind-0-1-0-1', 'C-bind-0-1-0-2', 'C-bind-0-1-1-0', 'C-bind-0-1-1-1', 'C-bind-0-1-1-2', 'C-bind-1-0-0-0', 'C-bind-1-0-0-1', 'C-bind-1-0-0-2', 'C-bind-1-0-1-0', 'C-bind-1-0-1-1', 'C-bind-1-0-1-2', 'C-bind-1-1-0-0', 'C-bind-1-1-0-1', 'C-bind-1-1-0-2', 'C-bind-1-1-1-0', 'C-bind-1-1-1-1', 'C-bind-1-1-1-2', 'M-Q-second', 'M-Q-microsecond', 'M-Q-hour', 'M-Q-date_trunc', 'M-Q-date_part', 'M-prepare-second', 'M-prepare-microsecond', 'M-prepare-hour', 'M-prepare-date_trunc', 'M-prepare-date_part', 'C-Q-negative-milliseconds', 'C-Q-negative-minute', 'C-Q-epoch', 'C-Q-week', 'C-Q-quarter', 'C-Q-quarter-year', 'C-Q-quarter-zero', 'C-Q-decade', 'C-Q-century', 'C-Q-millennium', 'C-Q-infinity-hour', 'C-Q-negative-infinity-day', 'C-Q-infinity-month', 'C-Q-infinity-week')


def run(port, database, evidence, mysql_port):
    assert database.replace("_", "").isalnum(), "invalid owned database name"
    evidence.mkdir(parents=True, exist_ok=False)
    repository = Path(__file__).resolve().parents[3]
    pq = Libpq()
    transaction_status = pq.lib.PQtransactionStatus
    transaction_status.restype = c.c_int
    transaction_status.argtypes = [c.c_void_p]
    conns, mysqls, passed = {}, {}, []
    attempted = set()
    owned_database = False
    reference_interval_created = False
    interval_table = "iv_" + database
    cleanup = {}
    def save(side, case, observation, phase="query"):
        if case.startswith(('C-Q-', 'C-E-', 'C-recover-', 'C-bind-', 'C-txn-', 'M-', 'I-')):
            attempted.add(case)
        with (evidence / (side + '.jsonl')).open('a') as out:
            out.write(json.dumps(dict(case_id=case, phase=phase, observation=observation)) + '\n')
        return observation
    def query(conn, sql):
        return pq.observe(pq.PQexec(conn, sql.encode()))
    def paired(case, sql, expected=None, zero=False, state=''):
        observations = [save(side, case, query(conn, sql)) for side, conn in conns.items()]
        for obs in observations:
            if state:
                assert obs['status'] == 7 and obs['state'] == state, case
                assert not obs['rows'] and all(obs['errors'][k] for k in 'SVCM'), case
            else:
                assert obs['status'] == 2, (case, obs)
                assert obs['metadata'] == [[1700, -1, -1, 0, 'v', 0, 0]], case
                assert obs['rows'] == ([] if zero else [[None if expected is None else expected.encode().hex()]]), case
        for obs in observations[1:]: compare(observations[0], obs)
        passed.append(case)
    def interleave(label):
        for side, conn in conns.items():
            obs = save(side, 'I-'+label, query(conn, 'SELECT 1::int4 AS v'))
            assert obs['rows'] == [['31']] and obs['status'] == 2
        observations = [save('mysql-'+side, 'I-'+label, mysql.query('SELECT v FROM health'), phase='health') for side, mysql in mysqls.items()]
        for obs in observations: assert obs['rows'] == [['37']] and obs['code'] == 0
        mysql_compare(*observations)
        behavior = [save('mysql-'+side, 'I-'+label, mysql.query("SELECT EXTRACT(SECOND FROM CAST('2024-01-02 00:00:40.123456' AS DATETIME(6))) AS v"), phase='native-behavior') for side, mysql in mysqls.items()]
        for obs in behavior:
            assert obs['code'] == 0 and obs['rows'] == [['3430']]
        mysql_compare(*behavior)
        passed.append('I-'+label)
    try:
        dsns = {'reference': os.environ['PG18_REFERENCE_DSN'],
                'A': f'host=127.0.0.1 port={port} user=root dbname={database}',
                'B': f'host=127.0.0.1 port={port} user=root dbname={database}'}
        for side, dsn in dsns.items():
            conn = pq.PQconnectdb(dsn.encode()); conns[side] = conn
            assert conn and pq.PQstatus(conn) == 0, 'PG connection failed (DSN redacted)'
        identity = {sql: query(conns['reference'], sql) for sql in ('SELECT version()', 'SHOW server_version_num', 'SHOW server_encoding', 'SHOW client_encoding', 'SHOW lc_time', 'SHOW TimeZone')}
        assert identity['SHOW server_version_num']['rows'] == [['313830303036']]
        save('reference', 'identity', identity)
        save('build', 'identity', dict(head=subprocess.check_output(['git','rev-parse','HEAD'],cwd=repository,text=True).strip(), fixture_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(), libpq=pq.PQlibVersion(), source_diff_sha256=hashlib.sha256(subprocess.check_output(['git','diff','HEAD','--','pkg/expression','pkg/server','pkg/session','pkg/types'],cwd=repository)).hexdigest(), cargo_lock_sha256=hashlib.sha256((repository/'Cargo.lock').read_bytes()).hexdigest()))
        reference_bin = os.environ.get('PG08_REFERENCE_BIN')
        build = dict()
        if reference_bin:
            binary = Path(reference_bin) / 'postgres'
            sharedir = Path(subprocess.check_output([str(Path(reference_bin)/'pg_config'),'--sharedir'], text=True).strip())
            timezone = sharedir / 'timezone' / 'Etc' / 'UTC'
            build.update(postgres_version=subprocess.check_output([str(binary),'--version'],text=True).strip(),
                         postgres_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                         configure=subprocess.check_output([str(Path(reference_bin)/'pg_config'),'--configure'],text=True).strip(),
                         tzdata_utc_sha256=hashlib.sha256(timezone.read_bytes()).hexdigest())
        tzdata_source = os.environ.get('PG08_TZDATA_SOURCE')
        if tzdata_source:
            tzdata = Path(tzdata_source).read_bytes()
            build['tzdata_version'] = tzdata.splitlines()[0].decode()
            build['tzdata_sha256'] = hashlib.sha256(tzdata).hexdigest()
        mysql_bin = os.environ.get('PG08_MYSQL_BIN')
        if mysql_bin:
            binary = Path(mysql_bin) / 'mysqld'
            build['mysql_version'] = subprocess.check_output([str(binary),'--version'],text=True).strip()
            build['mysql_sha256'] = hashlib.sha256(binary.read_bytes()).hexdigest()
        rust_binary = os.environ.get('PG08_07_RUST_BINARY')
        if rust_binary:
            build['rust_binary_sha256'] = hashlib.sha256(Path(rust_binary).read_bytes()).hexdigest()
        save('build', 'binary-identity', build)
        mysqls['reference'] = Mysql(os.environ['MYSQL8_REFERENCE_HOST'], int(os.environ['MYSQL8_REFERENCE_PORT']), os.environ['MYSQL8_REFERENCE_USER'], os.environ['MYSQL8_REFERENCE_PASSWORD'])
        save('build', 'mysql-client', dict(version=mysqls['reference'].mysql_get_client_version()))
        mysqls['aster'] = Mysql('127.0.0.1', mysql_port, 'root', '', database)
        assert mysqls['reference'].query(f'CREATE DATABASE {database}')['code'] == 0
        owned_database = True
        assert mysqls['reference'].query(f'USE {database}')['code'] == 0
        for side, mysql in mysqls.items():
            save('mysql-'+side,'identity',{sql:mysql.query(sql) for sql in ('SELECT version()', 'SELECT @@sql_mode', 'SELECT @@character_set_connection', 'SELECT @@collation_connection')})
            assert mysql.query('SET NAMES utf8mb4 COLLATE utf8mb4_bin')['code'] == 0
            assert mysql.query('CREATE TABLE health(v INT)')['code'] == 0
            assert mysql.query('INSERT INTO health VALUES (7)')['code'] == 0
        interleave('before')
        source = '27 months 4 days 05:06:07.123456'
        paired('C-Q-year',"SELECT EXTRACT(year FROM '27 months 4 days 05:06:07.123456'::interval) AS v",'2')
        paired('C-Q-month',"SELECT EXTRACT(month FROM '27 months 4 days 05:06:07.123456'::interval) AS v",'3')
        paired('C-Q-day',"SELECT EXTRACT(day FROM '27 months 4 days 05:06:07.123456'::interval) AS v",'4')
        paired('C-Q-hour',"SELECT EXTRACT(hour FROM '27 months 4 days 05:06:07.123456'::interval) AS v",'5')
        paired('C-Q-second',"SELECT EXTRACT(second FROM '27 months 4 days 05:06:07.123456'::interval) AS v",'7.123456')
        paired('C-Q-minute',"SELECT EXTRACT(minute FROM '80 minutes'::interval) AS v",'20')
        paired('C-Q-negative-year',"SELECT EXTRACT(year FROM '-27 months -4 days -05:06:07.123456'::interval) AS v",'-2')
        paired('C-Q-negative-month',"SELECT EXTRACT(month FROM '-27 months -4 days -05:06:07.123456'::interval) AS v",'-3')
        paired('C-Q-negative-day',"SELECT EXTRACT(day FROM '-27 months -4 days -05:06:07.123456'::interval) AS v",'-4')
        paired('C-Q-negative-hour',"SELECT EXTRACT(hour FROM '-27 months -4 days -05:06:07.123456'::interval) AS v",'-5')
        paired('C-Q-negative-second',"SELECT EXTRACT(second FROM '-27 months -4 days -05:06:07.123456'::interval) AS v",'-7.123456')
        paired('C-Q-negative-fraction',"SELECT EXTRACT(second FROM '-0.123456 seconds'::interval) AS v",'-0.123456')
        paired('C-Q-long-hour',"SELECT EXTRACT(hour FROM '2 days 27 hours'::interval) AS v",'27')
        paired('C-Q-microseconds',"SELECT EXTRACT(microseconds FROM '27 months 4 days 05:06:07.123456'::interval) AS v",'7123456')
        paired('C-Q-milliseconds',"SELECT EXTRACT(milliseconds FROM '27 months 4 days 05:06:07.123456'::interval) AS v",'7123.456')
        paired('C-Q-null','SELECT EXTRACT(second FROM NULL::interval) AS v',None)
        paired('C-Q-zero',"SELECT EXTRACT(second FROM NULL::interval) AS v WHERE false",zero=True)
        paired('C-Q-nested',"SELECT EXTRACT(second FROM date_trunc('milliseconds'::text,'27 months 4 days 05:06:07.123456'::interval)) AS v",'7.123000')
        paired("C-Q-negative-milliseconds","SELECT EXTRACT(milliseconds FROM '-0.123456 seconds'::interval) AS v",'-123.456')
        paired("C-Q-negative-minute","SELECT EXTRACT(minute FROM '-80 minutes'::interval) AS v",'-20')
        paired("C-Q-epoch","SELECT EXTRACT(epoch FROM '27 months 4 days 05:06:07.123456'::interval) AS v",'71255167.123456')
        paired("C-Q-week","SELECT EXTRACT(week FROM '-15 days'::interval) AS v",'-2')
        paired("C-Q-quarter","SELECT EXTRACT(quarter FROM '-27 months'::interval) AS v",'-2')
        paired("C-Q-quarter-year","SELECT EXTRACT(quarter FROM '-24 months'::interval) AS v",'-1')
        paired("C-Q-quarter-zero","SELECT EXTRACT(quarter FROM '0'::interval) AS v",'1')
        paired("C-Q-decade","SELECT EXTRACT(decade FROM '24001 months'::interval) AS v",'200')
        paired("C-Q-century","SELECT EXTRACT(century FROM '24001 months'::interval) AS v",'20')
        paired("C-Q-millennium","SELECT EXTRACT(millennium FROM '24001 months'::interval) AS v",'2')
        paired("C-Q-infinity-hour","SELECT EXTRACT(hour FROM 'infinity'::interval) AS v",'Infinity')
        paired("C-Q-negative-infinity-day","SELECT EXTRACT(day FROM '-infinity'::interval) AS v",'-Infinity')
        paired("C-Q-infinity-month","SELECT EXTRACT(month FROM 'infinity'::interval) AS v",None)
        paired("C-Q-infinity-week","SELECT EXTRACT(week FROM 'infinity'::interval) AS v",None)
        for unit,state in [('aster_bad_unit','22023'),('timezone','0A000')]:
            paired('C-E-'+unit,f"SELECT EXTRACT({unit} FROM '{source}'::interval) AS v",state=state)
            for side,conn in conns.items():
                obs=save(side,'C-recover-'+unit,query(conn,'SELECT 1::int4 AS v'));assert obs['rows']==[['31']]
            passed.append('C-recover-'+unit)
        assert query(conns['reference'], f'CREATE TEMP TABLE {interval_table}(value varchar(100))')['status'] == 1
        reference_interval_created = True
        assert query(conns['reference'], f"INSERT INTO {interval_table} VALUES ('{source}')")['status'] == 1
        assert mysqls['aster'].query(f'CREATE TABLE {interval_table}(value varchar(100))')['code'] == 0
        assert mysqls['aster'].query(f"INSERT INTO {interval_table} VALUES ('{source}')")['code'] == 0
        paired('C-Q-row', f"SELECT EXTRACT(second FROM value::interval) AS v FROM {interval_table}", '7.123456')
        paired('C-Q-row-nested', f"SELECT EXTRACT(second FROM date_trunc('milliseconds'::text,value::interval)) AS v FROM {interval_table}", '7.123000')
        paired('C-Q-row-zero', f"SELECT EXTRACT(second FROM value::interval) AS v FROM {interval_table} WHERE false", zero=True)
        interleave('middle')
        for empty in (0,1):
            name=('pg08_'+str(empty)).encode();sql='SELECT EXTRACT(second FROM $1::interval) AS v'+(' WHERE false' if empty else '')
            for side,conn in conns.items():
                assert save(side,'parse-'+str(empty),pq.observe(pq.PQprepare(conn,name,sql.encode(),0,None)))['status']==1
                desc=save(side,'describe-'+str(empty),pq.observe(pq.PQdescribePrepared(conn,name)))
                assert desc['parameters']==[1186] and desc['metadata']==[[1700,-1,-1,0,'v',0,0]],desc
            for infmt in (0,1):
                for outfmt in (0,1):
                    for index in range(3):
                        val=None if index==1 else (struct.pack('!qii',40123456,0,0) if infmt else b'40.123456 seconds')
                        encoded=[val];case=f'C-bind-{empty}-{infmt}-{outfmt}-{index}';observations=[]
                        for side,conn in conns.items():
                            obs=save(side,case,pq.observe(pq.PQexecPrepared(conn,name,1,(c.c_char_p*1)(*encoded),(c.c_int*1)(0 if val is None else len(val)),(c.c_int*1)(infmt),outfmt)))
                            assert obs['status']==2 and obs['metadata']==[[1700,-1,-1,outfmt,'v',0,0]],(case,obs)
                            target=None if index==1 else (struct.pack('!hhhhhhh',3,0,0,6,40,1234,5600).hex() if outfmt else b'40.123456'.hex())
                            assert obs['rows']==([] if empty else [[target]]),(case,obs)
                            observations.append(obs)
                        for obs in observations[1:]:compare(observations[0],obs)
                        passed.append(case)
        for unit,state in [('aster_bad_unit','22023'),('timezone','0A000')]:
            errors=[]
            for side,conn in conns.items():
                case='C-txn-'+unit
                assert save(side,case,query(conn,'BEGIN'),phase='begin')['status']==1
                assert transaction_status(conn)==2
                sql=f"SELECT EXTRACT({unit} FROM '{source}'::interval) AS v"
                obs=save(side,case,pq.observe(pq.PQprepare(conn,('bad_'+unit).encode(),sql.encode(),0,None)),phase='Parse')
                if obs['status']==1:
                    obs=save(side,case,pq.observe(pq.PQexecPrepared(conn,('bad_'+unit).encode(),0,None,None,None,0)),phase='Execute')
                assert obs['status']==7 and obs['state']==state,(case,obs)
                assert transaction_status(conn)==3
                assert save(side,case,query(conn,'ROLLBACK'),phase='rollback')['status']==1
                assert transaction_status(conn)==0
                recovered=save(side,case,query(conn,'SELECT 1::int4 AS v'),phase='recovery')
                assert recovered['rows']==[['31']] and recovered['status']==2
                save(side,case,dict(ready_states=['T','E','I']),phase='transaction-status');errors.append(obs)
            for obs in errors[1:]:compare(errors[0],obs)
            passed.append('C-txn-'+unit)
        for label,sql in [('second',"EXTRACT(SECOND FROM CAST('2024-01-02 00:00:40.123456' AS DATETIME(6)))"),('microsecond',"EXTRACT(MICROSECOND FROM CAST('2024-01-02 00:00:40.123456' AS DATETIME(6)))"),('hour',"EXTRACT(HOUR FROM CAST('27:01:02' AS TIME))"),('date_trunc',"date_trunc('day',CAST('2024-01-02 00:00:40' AS DATETIME))"),('date_part',"date_part('day',CAST('2024-01-02 00:00:40' AS DATETIME))")]:
            query_sql='SELECT '+sql+' AS v'
            observations=[save('mysql-'+side,'M-Q-'+label,m.query(query_sql)) for side,m in mysqls.items()]
            wanted = {'second': '40', 'microsecond': '123456', 'hour': '27'}.get(label)
            for obs in observations:
                if wanted is None:
                    assert obs['code'] == 1305 and obs['state'] == '42000' and not obs['rows'], label
                else:
                    assert obs['code'] == 0 and obs['rows'] == [[wanted.encode().hex()]], label
            mysql_compare(*observations);passed.append('M-Q-'+label)
            observations=[save('mysql-'+side,'M-prepare-'+label,m.prepared(query_sql,[()])) for side,m in mysqls.items()]
            for phases in observations:
                if wanted is None:
                    assert any(obs['code'] == 1305 and obs['state'] == '42000' for obs in phases), label
                else:
                    assert phases[-1]['code'] == 0 and phases[-1]['rows'] == [[wanted.encode().hex()]], label
            assert observations[0]==observations[1],label
            passed.append('M-prepare-'+label)
        interleave('after')
        assert len(passed)==len(set(passed))==78
        assert set(passed)==set(CASE_IDS)
    finally:
        # A failure assertion may leave the owned connection in an aborted transaction.
        for conn in conns.values():
            if conn and transaction_status(conn) in (2,3):
                assert query(conn, 'ROLLBACK')['status'] == 1
        if reference_interval_created:
            cleanup['reference_interval_table_dropped'] = query(conns['reference'], f'DROP TABLE {interval_table}')['status'] == 1
        if owned_database:
            cleanup['reference_database_dropped']=mysqls['reference'].query(f'DROP DATABASE {database}')['code']==0
        for m in mysqls.values():m.close()
        for conn in conns.values():
            if conn:pq.PQfinish(conn)
        cleanup.update(
            connections_closed=True,
            negative_self_checks=3,
            expected=78,
            actual=len(attempted),
            passed=len(passed),
            failed=len(attempted-set(passed)),
            skipped=78-len(attempted),
            unique=len(set(passed)),
            case_ids=passed,
            expected_case_ids=CASE_IDS,
            counts=dict(
                success=sum(x.startswith('C-Q-') and x not in ('C-Q-null','C-Q-infinity-month','C-Q-infinity-week','C-Q-zero','C-Q-row-zero') for x in passed),
                null=sum(x in ('C-Q-null','C-Q-infinity-month','C-Q-infinity-week') for x in passed),
                zero=sum(x in passed for x in ('C-Q-zero', 'C-Q-row-zero')),
                error=sum(x.startswith('C-E-') for x in passed),
                recovery=sum(x.startswith('C-recover-') for x in passed),
                binding=sum(x.startswith('C-bind-') for x in passed),
                transaction=sum(x.startswith('C-txn-') for x in passed),
                mysql=sum(x.startswith('M-') for x in passed),
                interleaved=sum(x.startswith('I-') for x in passed),
            ),
        )
        (evidence/'summary.json').write_text(json.dumps(cleanup,indent=2))
    print('pg08_07_clients: 78 unique scenarios passed; 3 negative self-checks')

if __name__=='__main__':
    def interrupted(signum, _frame):
        raise SystemExit(128 + signum)
    signal.signal(signal.SIGTERM, interrupted)
    assert self_test()==3
    if sys.argv[1:]!=['--self-test']:
        run(int(sys.argv[1]),sys.argv[2],Path(sys.argv[3]),int(sys.argv[4]))
