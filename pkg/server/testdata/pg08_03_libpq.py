# Copyright 2026 AsterSQL.
"""Task 08.03 timestamptz observations. Reference credentials are never archived."""
import ctypes as c
import hashlib
from datetime import datetime
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


CASE_IDS = (
    'I-before',
    'I-middle',
    'I-after',
    'C-Q-microseconds',
    'C-Q-milliseconds',
    'C-Q-second',
    'C-Q-minute',
    'C-Q-hour',
    'C-Q-day',
    'C-Q-week',
    'C-Q-month',
    'C-Q-quarter',
    'C-Q-year',
    'C-Q-decade',
    'C-Q-century',
    'C-Q-millennium',
    'C-Q-week-boundary',
    'C-Q-negative-epoch',
    'C-Q-infinity',
    'C-Q-null',
    'C-Q-zero',
    'C-Q-nested',
    'C-E-nonsense',
    'C-E-timezone',
    'C-recover-nonsense',
    'C-recover-timezone',
    'C-txn-nonsense',
    'C-txn-timezone',
    'C-bind-0-0-0-0',
    'C-bind-0-0-0-1',
    'C-bind-0-0-0-2',
    'C-bind-0-0-1-0',
    'C-bind-0-0-1-1',
    'C-bind-0-0-1-2',
    'C-bind-0-1-0-0',
    'C-bind-0-1-0-1',
    'C-bind-0-1-0-2',
    'C-bind-0-1-1-0',
    'C-bind-0-1-1-1',
    'C-bind-0-1-1-2',
    'C-bind-1-0-0-0',
    'C-bind-1-0-0-1',
    'C-bind-1-0-0-2',
    'C-bind-1-0-1-0',
    'C-bind-1-0-1-1',
    'C-bind-1-0-1-2',
    'C-bind-1-1-0-0',
    'C-bind-1-1-0-1',
    'C-bind-1-1-0-2',
    'C-bind-1-1-1-0',
    'C-bind-1-1-1-1',
    'C-bind-1-1-1-2',
    'M-Q-second',
    'M-Q-microsecond',
    'M-Q-hour',
    'M-Q-date_trunc',
    'M-Q-date_part',
    'M-prepare-second',
    'M-prepare-microsecond',
    'M-prepare-hour',
    'M-prepare-date_trunc',
    'M-prepare-date_part',
    'C-bind-0-0-0-3',
    'C-bind-0-0-1-3',
    'C-bind-0-1-0-3',
    'C-bind-0-1-1-3',
    'C-bind-1-0-0-3',
    'C-bind-1-0-1-3',
    'C-bind-1-1-0-3',
    'C-bind-1-1-1-3',
    'C-Q-explicit-UTC-ny',
    'C-Q-explicit-UTC-spring-before',
    'C-Q-explicit-UTC-spring-after',
    'C-Q-explicit-UTC-fall-before',
    'C-Q-explicit-UTC-fall-after',
    'C-Q-explicit-America_New_York-ny',
    'C-Q-explicit-America_New_York-spring-before',
    'C-Q-explicit-America_New_York-spring-after',
    'C-Q-explicit-America_New_York-fall-before',
    'C-Q-explicit-America_New_York-fall-after',
    'C-Q-null-zone',
    'C-E-zone',
    'C-recover-zone',
    'C-Q-timestamp-implicit',
    'C-E-signature-interval',
    'C-Q-row',
    'C-Q-row-nested',
    'C-Q-row-zero',
    'C-bind-switch-0-0-NY',
    'C-bind-switch-0-0-UTC',
    'C-bind-switch-0-1-NY',
    'C-bind-switch-0-1-UTC',
    'C-bind-switch-1-0-NY',
    'C-bind-switch-1-0-UTC',
    'C-bind-switch-1-1-NY',
    'C-bind-switch-1-1-UTC',
    'I-explicit-isolation',
    'C-Q-implicit-ny',
    'C-Q-implicit-gap',
    'C-Q-implicit-overlap',
)

def run(port, database, evidence, mysql_port):
    assert database.replace("_", "").isalnum(), "invalid owned database name"
    evidence.mkdir(parents=True, exist_ok=False)
    repository = Path(__file__).resolve().parents[3]
    pq = Libpq()
    transaction_status = pq.lib.PQtransactionStatus
    transaction_status.restype = c.c_int
    transaction_status.argtypes = [c.c_void_p]
    parameter_status = pq.lib.PQparameterStatus
    parameter_status.restype = c.c_char_p
    parameter_status.argtypes = [c.c_void_p,c.c_char_p]
    conns, mysqls, passed = {}, {}, []
    attempted = set()
    owned_database = False
    cleanup = {}
    original_zones = {}
    def save(side, case, observation, phase="query"):
        if case.startswith(('C-Q-', 'C-E-', 'C-recover-', 'C-bind-', 'C-txn-', 'M-', 'I-')):
            attempted.add(case)
        with (evidence / (side + '.jsonl')).open('a') as out:
            out.write(json.dumps(dict(case_id=case, phase=phase, observation=observation)) + '\n')
        return observation
    def query(conn, sql):
        return pq.observe(pq.PQexec(conn, sql.encode()))
    def set_zone(side, name):
        # The existing Aster session interface uses native time_zone; the PG
        # GUC grammar is outside this task. Numeric GUC zones use POSIX signs.
        value = name
        if side != 'reference' and name.startswith(('+','-')):
            value = ('-' if name[0]=='+' else '+') + name[1:]
        sql = ("SET TIME ZONE '" if side=='reference' else "SET time_zone = '") + value.replace("'","''") + "'"
        return query(conns[side],sql)
    def paired(case, sql, expected=None, zero=False, state=''):
        observations = [save(side, case, query(conn, sql)) for side, conn in conns.items()]
        for obs in observations:
            if state:
                assert obs['status'] == 7 and obs['state'] == state, case
                assert not obs['rows'] and all(obs['errors'][k] for k in 'SVCM'), case
            else:
                assert obs['status'] == 2, (case, obs)
                assert obs['metadata'] == [[1184, 8, -1, 0, 'v', 0, 0]], case
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
        for side, conn in conns.items():
            original_zones[side] = parameter_status(conn,b'TimeZone').decode()
            assert set_zone(side,'UTC')['status'] == 1
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
        rust_binary = os.environ.get('PG08_03_RUST_BINARY')
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
            identity = {sql:mysql.query(sql) for sql in ('SELECT version()', 'SELECT @@sql_mode', 'SELECT @@character_set_connection', 'SELECT @@collation_connection')}
            save('mysql-'+side,'identity',identity)
            if side == 'reference':
                assert bytes.fromhex(identity['SELECT version()']['rows'][0][0]).decode().startswith('8.0.')
            assert mysql.query('SET NAMES utf8mb4 COLLATE utf8mb4_bin')['code'] == 0
            assert mysql.query('CREATE TABLE health(v INT)')['code'] == 0
            assert mysql.query('INSERT INTO health VALUES (7)')['code'] == 0
        interleave('before')
        source = '2024-01-02 00:00:40.123456'
        expected = {'microseconds':source,'milliseconds':'2024-01-02 00:00:40.123','second':'2024-01-02 00:00:40','minute':'2024-01-02 00:00:00','hour':'2024-01-02 00:00:00','day':'2024-01-02 00:00:00','week':'2024-01-01 00:00:00','month':'2024-01-01 00:00:00','quarter':'2024-01-01 00:00:00','year':'2024-01-01 00:00:00','decade':'2020-01-01 00:00:00','century':'2001-01-01 00:00:00','millennium':'2001-01-01 00:00:00'}
        expected = {k: v + '+00' for k, v in expected.items()}
        for unit, value in expected.items():
            paired('C-Q-'+unit, f"SELECT date_trunc('{unit}'::text, '{source}'::timestamptz,'UTC'::text) AS v", value)
        for label,unit,value,result in [('week-boundary','week','2021-01-01 12:34:56','2020-12-28 00:00:00'),('negative-epoch','day','1969-12-31 23:59:59.999999','1969-12-31 00:00:00'),('infinity','day','infinity','infinity')]:
            paired('C-Q-'+label,f"SELECT date_trunc('{unit}'::text, '{value}'::timestamptz,'UTC'::text) AS v", result if result == 'infinity' else result + '+00')
        paired('C-Q-null',"SELECT date_trunc('day'::text, NULL::timestamptz,'UTC'::text) AS v")
        paired('C-Q-zero',"SELECT date_trunc('day'::text, NULL::timestamptz,'UTC'::text) AS v WHERE false",zero=True)
        paired('C-Q-nested',f"SELECT date_trunc('day'::text,date_trunc('hour'::text,'{source}'::timestamptz,'UTC'::text),'UTC'::text) AS v",'2024-01-02 00:00:00+00')
        for unit,state in [('nonsense','22023'),('timezone','0A000')]:
            paired('C-E-'+unit,f"SELECT date_trunc('{unit}'::text,'{source}'::timestamptz,'UTC'::text) AS v",state=state)
            for side,conn in conns.items():
                obs=save(side,'C-recover-'+unit,query(conn,'SELECT 1::int4 AS v'));assert obs['rows']==[['31']]
            passed.append('C-recover-'+unit)
        interleave('middle')
        for empty in (0,1):
            name=('pg08_'+str(empty)).encode();sql='SELECT date_trunc($2::text,$1::timestamptz,$3::text) AS v'+(' WHERE false' if empty else '')
            for side,conn in conns.items():
                assert save(side,'parse-'+str(empty),pq.observe(pq.PQprepare(conn,name,sql.encode(),0,None)))['status']==1
                desc=save(side,'describe-'+str(empty),pq.observe(pq.PQdescribePrepared(conn,name)))
                assert desc['parameters']==[1184,25,25] and desc['metadata']==[[1184,8,-1,0,'v',0,0]]
            for infmt in (0,1):
                for outfmt in (0,1):
                    for index in range(4):
                        # Exact PostgreSQL epoch microseconds, independently computed from the fixed civil date.
                        delta = datetime(2024,1,2,0,0,40,123456)-datetime(2000,1,1)
                        micros = (delta.days * 86400 + delta.seconds) * 1000000 + delta.microseconds
                        val=None if index==1 else (struct.pack('!q',micros) if infmt else source.encode())
                        encoded=[val,b'milliseconds',None if index==3 else b'UTC'];case=f'C-bind-{empty}-{infmt}-{outfmt}-{index}'
                        observations=[]
                        for side,conn in conns.items():
                            obs=save(side,case,pq.observe(pq.PQexecPrepared(conn,name,3,(c.c_char_p*3)(*encoded),(c.c_int*3)(*[0 if v is None else len(v) for v in encoded]),(c.c_int*3)(infmt,infmt,infmt),outfmt)))
                            assert obs['status']==2 and obs['metadata']==[[1184,8,-1,outfmt,'v',0,0]],case
                            target=None if index in (1,3) else (struct.pack('!q',micros//1000*1000).hex() if outfmt else expected['milliseconds'].encode().hex())
                            assert obs['rows']==([] if empty else [[target]]),case
                            observations.append(obs)
                        for obs in observations[1:]:compare(observations[0],obs)
                        passed.append(case)
        for unit, state in [('nonsense', '22023'), ('timezone', '0A000')]:
            errors = []
            for side, conn in conns.items():
                case = 'C-txn-' + unit
                assert save(side, case, query(conn, 'BEGIN'), phase='begin')['status'] == 1
                assert transaction_status(conn) == 2
                encoded = [source.encode(), unit.encode(), b'UTC']
                obs = save(side, case, pq.observe(pq.PQexecPrepared(conn, b'pg08_0', 3,
                    (c.c_char_p*3)(*encoded), (c.c_int*3)(*[len(v) for v in encoded]),
                    (c.c_int*3)(0, 0, 0), 0)), phase='PQexecPrepared-error')
                assert obs['status'] == 7 and obs['state'] == state
                assert transaction_status(conn) == 3
                assert save(side, case, query(conn, 'ROLLBACK'), phase='rollback')['status'] == 1
                assert transaction_status(conn) == 0
                recovered = save(side, case, query(conn, 'SELECT 1::int4 AS v'), phase='recovery')
                assert recovered['rows'] == [['31']] and recovered['status'] == 2
                save(side, case, dict(ready_states=['T', 'E', 'I']), phase='transaction-status')
                errors.append(obs)
            for obs in errors[1:]:
                compare(errors[0], obs)
            passed.append('C-txn-' + unit)
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
        def zone(name):
            for side in conns:
                assert set_zone(side,name)['status'] == 1
        buckets = [
            ('ny','2024-01-02 01:30:00+00','2024-01-01 05:00:00+00','2024-01-01 00:00:00-05'),
            ('spring-before','2024-03-10 16:00:00+00','2024-03-10 05:00:00+00','2024-03-10 00:00:00-05'),
            ('spring-after','2024-03-11 16:00:00+00','2024-03-11 04:00:00+00','2024-03-11 00:00:00-04'),
            ('fall-before','2024-11-03 16:00:00+00','2024-11-03 04:00:00+00','2024-11-03 00:00:00-04'),
            ('fall-after','2024-11-04 16:00:00+00','2024-11-04 05:00:00+00','2024-11-04 00:00:00-05'),
        ]
        for display in ('UTC','America/New_York'):
            zone(display)
            for label,src,utc,local in buckets:
                paired('C-Q-explicit-'+display.replace('/','_')+'-'+label,
                    f"SELECT date_trunc('day'::text,'{src}'::timestamptz,'America/New_York'::text) AS v", utc if display=='UTC' else local)
        zone('UTC')
        paired('C-Q-null-zone',"SELECT date_trunc('day'::text,'2024-01-02 01:30:00+00'::timestamptz,NULL::text) AS v")
        paired('C-E-zone',"SELECT date_trunc('day'::text,'2024-01-02 01:30:00+00'::timestamptz,'NoSuchZone'::text) AS v",state='22023')
        for side,conn in conns.items():
            assert save(side,'C-recover-zone',query(conn,'SELECT 1::int4 AS v'))['rows']==[['31']]
        passed.append('C-recover-zone')
        paired('C-Q-timestamp-implicit',"SELECT date_trunc('day'::text,'2024-01-02 00:00:00'::timestamp,'UTC'::text) AS v",'2024-01-02 00:00:00+00')
        paired('C-E-signature-interval',"SELECT date_trunc('day'::text,'1 day'::interval,'UTC'::text) AS v",state='42883')
        for label,expr,empty in [('row',"date_trunc('day'::text,t,'America/New_York'::text)",False),('row-nested',"date_trunc('day'::text,date_trunc('hour'::text,t,'UTC'::text),'America/New_York'::text)",False),('row-zero',"date_trunc('day'::text,t,'America/New_York'::text)",True)]:
            paired('C-Q-'+label,f"SELECT {expr} AS v FROM (SELECT '2024-01-02 01:30:00+00'::timestamptz AS t) AS source"+(' WHERE false' if empty else ''),'2024-01-01 05:00:00+00',zero=empty)
        zone('America/New_York')
        for label,unit,source,target in [
            ('ny','day','2024-01-02 01:30:00','2024-01-01 19:00:00-05'),
            ('gap','hour','2024-03-10 02:30:00','2024-03-10 03:00:00-04'),
            ('overlap','hour','2024-11-03 01:30:00','2024-11-03 01:00:00-05'),
        ]:
            paired('C-Q-implicit-'+label,f"SELECT date_trunc('{unit}'::text,'{source}'::timestamp,'UTC'::text) AS v",target)
        zone('UTC')
        # Rebind the zone on the same prepared statement, in all format combinations.
        sql = b'SELECT date_trunc($2::text,$1::timestamptz,$3) AS v'
        for side,conn in conns.items():
            assert pq.observe(pq.PQprepare(conn,b'switch',sql,0,None))['status']==1
            desc=save(side,'describe-switch',pq.observe(pq.PQdescribePrepared(conn,b'switch')))
            assert desc['parameters']==[1184,25,25] and desc['metadata']==[[1184,8,-1,0,'v',0,0]]
        source_micros = int((datetime(2024,1,2,1,30)-datetime(2000,1,1)).total_seconds())*1000000
        for infmt in (0,1):
            for outfmt in (0,1):
                for label,tz,target in [('NY','America/New_York',datetime(2024,1,1,5)),('UTC','UTC',datetime(2024,1,2))]:
                    case=f'C-bind-switch-{infmt}-{outfmt}-{label}'
                    encoded=[struct.pack('!q',source_micros) if infmt else b'2024-01-02 01:30:00+00',b'day',tz.encode()]
                    target_micros=int((target-datetime(2000,1,1)).total_seconds())*1000000
                    payload=struct.pack('!q',target_micros) if outfmt else (target.strftime('%Y-%m-%d %H:%M:%S')+'+00').encode()
                    observations=[]
                    for side,conn in conns.items():
                        obs=save(side,case,pq.observe(pq.PQexecPrepared(conn,b'switch',3,(c.c_char_p*3)(*encoded),(c.c_int*3)(*[len(v) for v in encoded]),(c.c_int*3)(infmt,infmt,infmt),outfmt)))
                        assert obs['status']==2 and obs['metadata']==[[1184,8,-1,outfmt,'v',0,0]] and obs['rows']==[[payload.hex()]],(case,obs)
                        observations.append(obs)
                    for obs in observations[1:]:compare(observations[0],obs)
                    passed.append(case)
        assert set_zone('B','America/New_York')['status']==1
        for side,conn in conns.items():
            obs=save(side,'I-explicit-isolation',query(conn,"SELECT date_trunc('day'::text,'2024-01-02 01:30:00+00'::timestamptz,'America/New_York'::text) AS v"))
            target='2024-01-01 00:00:00-05' if side=='B' else '2024-01-01 05:00:00+00'
            assert obs['status']==2 and obs['rows']==[[target.encode().hex()]]
        passed.append('I-explicit-isolation')
        assert set_zone('B','UTC')['status']==1
        interleave('after')
        assert len(passed)==len(set(passed))==100 and set(passed)==set(CASE_IDS)
    finally:
        if owned_database:
            cleanup['reference_database_dropped']=mysqls['reference'].query(f'DROP DATABASE {database}')['code']==0
        for m in mysqls.values():m.close()
        restored = True
        for side, conn in conns.items():
            if conn:
                if transaction_status(conn) in (2,3): query(conn,'ROLLBACK')
                if side in original_zones:
                    restored &= set_zone(side,original_zones[side])['status']==1
                pq.PQfinish(conn)
        cleanup['timezones_restored'] = restored
        cleanup.update(
            connections_closed=True,
            negative_self_checks=3,
            expected=100,
            actual=len(attempted),
            passed=len(passed),
            failed=len(attempted-set(passed)),
            skipped=100-len(attempted),
            unique=len(set(passed)),
            case_ids=passed,
            expected_case_ids=CASE_IDS,
            counts=dict(
                success=sum(x.startswith('C-Q-') and x not in ('C-Q-null', 'C-Q-null-zone', 'C-Q-zero', 'C-Q-row-zero') for x in passed),
                null=sum(x in passed for x in ('C-Q-null','C-Q-null-zone')),
                zero=sum(x in passed for x in ('C-Q-zero','C-Q-row-zero')),
                error=sum(x.startswith('C-E-') for x in passed),
                recovery=sum(x.startswith('C-recover-') for x in passed),
                binding=sum(x.startswith('C-bind-') for x in passed),
                transaction=sum(x.startswith('C-txn-') for x in passed),
                mysql=sum(x.startswith('M-') for x in passed),
                interleaved=sum(x.startswith('I-') for x in passed),
            ),
        )
        (evidence/'summary.json').write_text(json.dumps(cleanup,indent=2))
    print('pg08_03_clients: 100 unique scenarios passed; 3 negative self-checks')

if __name__=='__main__':
    def interrupted(signum, _frame):
        raise SystemExit(128 + signum)
    signal.signal(signal.SIGTERM, interrupted)
    assert self_test()==3
    if sys.argv[1:]!=['--self-test']:
        run(int(sys.argv[1]),sys.argv[2],Path(sys.argv[3]),int(sys.argv[4]))
