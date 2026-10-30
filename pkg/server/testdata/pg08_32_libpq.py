# Copyright 2026 AsterSQL.
"""Analyzed version dialect identity, NULLs and protocol metadata through native clients."""
import copy
import ctypes as c
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
from pg_function_23_libpq import Libpq, compare, self_test
from pg_function_27_libpq import Mysql
from pg_function_25_libpq import mysql_compare

CASES = (
 ('direct','version()',False,False),
 ('qualified','pg_catalog.version()',False,False),
 ('nested','COALESCE(version(),NULL::text)',False,False),
 ('row','version()',False,False),
 ('zero','version()',True,False),
 ('rowzero','version()',True,False),
 ('allnull','NULL::text',False,True),
)
CASE_IDS = tuple([f'PG-{label}-{mode}' for label,*_ in CASES for mode in ('Q','P0','P1')]
 + [f'error-{case}-{phase}' for case in ('arity','nullarity') for phase in ('Q','P','txn')]
 + [f'M-{fn}-{mode}' for fn in ('current_database','current_schema','pg_typeof','pg_backend_pid','database','version','connection_id','health') for mode in ('Q','P')]
 + ['I-before','I-middle','I-after','reconnect'])
assert len(CASE_IDS)==47 and len(set(CASE_IDS))==47


def run(port, database, evidence, mysql_port):
    assert database.replace('_','').isalnum()
    evidence.mkdir(parents=True, exist_ok=False)
    pq = Libpq()
    assert pq.PQlibVersion() >= 180000
    tx = pq.lib.PQtransactionStatus;tx.restype=c.c_int;tx.argtypes=[c.c_void_p]
    conns, mysqls, passed, attempted, cleanup = {}, {}, [], set(), {}
    failures = {}
    native_facts = []
    owned_pg, owned_mysql = [], False
    def query(conn, sql): return pq.observe(pq.PQexec(conn, sql.encode()))
    def save(side, case, obs, phase='query'):
        if case in CASE_IDS: attempted.add(case)
        with (evidence/(side+'.jsonl')).open('a') as f:f.write(json.dumps(dict(case_id=case,phase=phase,observation=obs))+'\n')
        return obs
    expected_versions = {}
    parameter = pq.lib.PQparameterStatus; parameter.restype=c.c_char_p; parameter.argtypes=[c.c_void_p,c.c_char_p]
    numeric_version=pq.lib.PQserverVersion; numeric_version.restype=c.c_int; numeric_version.argtypes=[c.c_void_p]
    def expected(conn): return expected_versions[conn]
    def check_pg(obs, version, zero=False, null=False, fmt=0):
        assert obs['status']==2 and obs['metadata']==[[25,-1,-1,fmt,'v',0,0]],obs
        payload=version.encode().hex()
        assert obs['rows']==([] if zero else [[None if null else payload]]),obs
    def equivalent(a,b):
        # Assert each raw version against its independent build/startup facts above.
        # Only then compare all portable descriptor/diagnostic fields.
        a,b=copy.deepcopy(a),copy.deepcopy(b)
        a['rows']=b['rows']=[]
        compare(a,b)
    def interleave(label):
        case='I-'+label
        for side,conn in conns.items():
            obs=save(side,case,query(conn,'SELECT version() AS v'))
            check_pg(obs,expected(conn))
            if label=='before':
                assert pq.observe(pq.PQprepare(conn,b'version_shared',b'SELECT version() AS v',0,None))['status']==1
            prepared=save(side,case,pq.observe(pq.PQexecPrepared(conn,b'version_shared',0,None,None,None,1)),'interleaved-prepared')
            check_pg(prepared,expected(conn),fmt=1)
        for side,m in mysqls.items():
            # Exercise the same SQL text in A, B and native M at every checkpoint.
            raw=m.lib.mysql_get_server_info(m.conn)
            for mode in ('Q','P'):
                obs=save('mysql-'+side,case,m.query('SELECT version() AS v') if mode=='Q' else m.prepared('SELECT version() AS v',[()]),'interleaved-'+mode)
                parts=[obs] if mode=='Q' else obs
                assert all(part['code']==0 for part in parts),obs
                assert parts[-1]['rows']==[[raw.hex()]],(side,obs,raw.decode())
            assert m.query('SELECT database() AS v')['rows']==[[database.encode().hex()]]
            assert int(bytes.fromhex(m.query('SELECT connection_id() AS v')['rows'][0][0]))==m.lib.mysql_thread_id(m.conn)
        peers=[save('mysql-'+side,case,m.query('SELECT v FROM health')) for side,m in mysqls.items()]
        for obs in peers:assert obs['code']==0 and obs['rows']==[['37']],obs
        mysql_compare(*peers);passed.append(case)
    try:
        admin=pq.PQconnectdb(os.environ['PG18_REFERENCE_DSN'].encode())
        assert admin and pq.PQstatus(admin)==0,'reference connection failed (DSN redacted)'
        identity={sql:query(admin,sql) for sql in ('SELECT version()','SHOW server_version_num','SHOW server_encoding','SHOW client_encoding','SHOW lc_time','SHOW lc_numeric','SHOW lc_monetary','SHOW TimeZone','SHOW search_path')}
        assert identity['SHOW server_version_num']['rows']==[['313830303036']]
        save('reference','identity',identity)
        repo=Path(__file__).resolve().parents[3]
        bins={}
        for key,env,name in [('postgres','PG08_REFERENCE_BIN','postgres'),('mysql','PG08_MYSQL_BIN','mysqld')]:
            binary=Path(os.environ[env])/name
            bins[key]=dict(version=subprocess.check_output([str(binary),'--version'],text=True).strip(),sha256=hashlib.sha256(binary.read_bytes()).hexdigest())
        tz=Path(os.environ['PG08_TZDATA_SOURCE']).read_bytes()
        save('build','identity',dict(head=subprocess.check_output(['git','rev-parse','HEAD'],cwd=repo,text=True).strip(),fixture_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),libpq=pq.PQlibVersion(),binaries=bins,tzdata_version=tz.splitlines()[0].decode(),tzdata_sha256=hashlib.sha256(tz).hexdigest(),source_diff_sha256=hashlib.sha256(subprocess.check_output(['git','diff','HEAD','--','pkg'],cwd=repo)).hexdigest(),expected_version=os.environ['PG08_32_EXPECTED_VERSION'],rust_binary_sha256=hashlib.sha256(Path(os.environ['PG08_32_RUST_BINARY']).read_bytes()).hexdigest()))
        for suffix in ('','_b'):
            db=database+suffix
            assert query(admin,f'CREATE DATABASE {db}')['status']==1
            owned_pg.append(db)
            for side,dsn in [('reference',os.environ['PG18_REFERENCE_DSN']+f' dbname={database}'),('aster',f'host=127.0.0.1 port={port} user=root dbname={database}')]:
                key=side+('-B' if suffix else '-A')
                conn=pq.PQconnectdb(dsn.encode());assert conn and pq.PQstatus(conn)==0,'login failed (DSN redacted)';conns[key]=conn
                startup=parameter(conn,b'server_version').decode()
                if side=='reference':
                    version=bytes.fromhex(identity['SELECT version()']['rows'][0][0]).decode()
                    assert startup.startswith('18.6') and version.startswith('PostgreSQL '+startup.split()[0]+' on '),(startup,version)
                else:
                    assert startup=='18.0 (AsterSQL)',startup
                    version=os.environ['PG08_32_EXPECTED_VERSION']
                    assert version.startswith('PostgreSQL '+startup+' compatibility\n')
                    assert 'RustVersion: ' in version and 'Git Commit Hash:' in version
                expected_versions[conn]=version
                assert numeric_version(conn)==(180006 if side=='reference' else 180000)
                save(key,'startup-version',dict(server_version=startup,server_version_num=numeric_version(conn),expected_version=version))
                if not suffix:
                    assert query(conn,'CREATE TABLE pg0832_source(v integer PRIMARY KEY)')['status']==1
                    assert query(conn,'INSERT INTO pg0832_source VALUES(7)')['status']==1
        mysqls['reference']=Mysql(os.environ['MYSQL8_REFERENCE_HOST'],int(os.environ['MYSQL8_REFERENCE_PORT']),os.environ['MYSQL8_REFERENCE_USER'],os.environ['MYSQL8_REFERENCE_PASSWORD'])
        mysqls['aster']=Mysql('127.0.0.1',mysql_port,'root','',database)
        assert mysqls['reference'].query(f'CREATE DATABASE {database}')['code']==0;owned_mysql=True
        assert mysqls['reference'].query(f'USE {database}')['code']==0
        for side,m in mysqls.items():
            save('mysql-'+side,'identity',{sql:m.query(sql) for sql in ('SELECT version()','SELECT @@sql_mode','SELECT @@character_set_connection','SELECT @@collation_connection')})
            server_info=m.lib.mysql_get_server_info;server_info.restype=c.c_char_p;server_info.argtypes=[c.c_void_p]
            thread=m.lib.mysql_thread_id;thread.restype=c.c_ulong;thread.argtypes=[c.c_void_p]
            save('mysql-'+side,'native-identity',dict(client_version=m.mysql_get_client_version(),server_info=server_info(m.conn).decode(),connection_id=thread(m.conn)))
            assert m.query('SET NAMES utf8mb4 COLLATE utf8mb4_bin')['code']==0
            assert m.query('CREATE TABLE health(v INT)')['code']==0
            assert m.query('INSERT INTO health VALUES(7)')['code']==0
        interleave('before')
        for label,expr,zero,null in CASES:
            sql=f'SELECT {expr} AS v'+(' FROM pg0832_source' if label in ('row','rowzero','allnull') else '')+(' WHERE false' if zero else ' WHERE version() IS NOT NULL' if null else '')
            for mode in ('Q','P0','P1'):
                case=f'PG-{label}-{mode}';peers={}
                for side,conn in conns.items():
                    fmt=0 if mode=='Q' else int(mode[-1])
                    if mode=='Q':obs=query(conn,sql)
                    else:
                        statement=('version_'+label+mode).encode()
                        assert pq.observe(pq.PQprepare(conn,statement,sql.encode(),0,None))['status']==1
                        desc=save(side,'describe',pq.observe(pq.PQdescribePrepared(conn,statement)))
                        assert desc['parameters']==[] and desc['metadata']==[[25,-1,-1,0,'v',0,0]],desc
                        obs=pq.observe(pq.PQexecPrepared(conn,statement,0,None,None,None,fmt))
                    save(side,case,obs,mode);check_pg(obs,expected(conn),zero,null,fmt);peers[side]=obs
                for suffix in ('A','B'):equivalent(peers['reference-'+suffix],peers['aster-'+suffix])
                passed.append(case)
        for side in ('reference','aster'):
            old=conns.pop(side+'-A');previous=expected_versions.pop(old);pq.PQfinish(old)
            dsn=(os.environ['PG18_REFERENCE_DSN']+f' dbname={database}') if side=='reference' else f'host=127.0.0.1 port={port} user=root dbname={database}'
            conn=pq.PQconnectdb(dsn.encode());assert conn and pq.PQstatus(conn)==0
            conns[side+'-A']=conn
            expected_versions[conn]=previous
            assert parameter(conn,b'server_version')
            assert pq.observe(pq.PQprepare(conn,b'version_shared',b'SELECT version() AS v',0,None))['status']==1
            check_pg(save(side,'reconnect',query(conn,'SELECT version() AS v')),expected(conn))
        passed.append('reconnect')
        interleave('middle')
        for label,sql in [('arity','SELECT version(1::int4) AS v'),('nullarity','SELECT version(NULL::int4) AS v')]:
          for phase in ('Q','P','txn'):
            case=f'error-{label}-{phase}';peers={}
            for side,conn in conns.items():
                if phase=='txn':assert query(conn,'BEGIN')['status']==1
                if phase=='P':
                    obs=save(side,case,pq.observe(pq.PQprepare(conn,b'bad',sql.encode(),0,None)),'parse-analyze')
                    if obs['status']==1:obs=save(side,case,pq.observe(pq.PQexecPrepared(conn,b'bad',0,None,None,None,0)),'plan-execute')
                else:obs=save(side,case,query(conn,sql),phase)
                assert obs['status']==7 and obs['state']=='42883' and not obs['rows'] and not obs['metadata'],obs
                assert all(obs['errors'][key] for key in 'SVCM'),obs
                peers[side]=obs
                if phase=='txn':
                    assert tx(conn)==3
                    assert query(conn,'ROLLBACK')['status']==1 and tx(conn)==0
                # libpq synchronizes failed extended Parse; next query must succeed.
                health=save(side,case,query(conn,'SELECT 1::int4 AS v'),'recovery');assert health['rows']==[['31']],health
                assert query(conn,'SELECT v FROM pg0832_source')['rows']==[['37']]
            for suffix in ('A','B'):compare(peers['reference-'+suffix],peers['aster-'+suffix])
            passed.append(case)
        for fn in ('current_database','current_schema','pg_typeof','pg_backend_pid','database','version','connection_id','health'):
          sql='SELECT '+('v FROM health' if fn=='health' else fn+('('+('1' if fn=='pg_typeof' else '')+')'))+' AS v' if fn!='health' else 'SELECT v FROM health'
          for mode in ('Q','P'):
            case=f'M-{fn}-{mode}';peers={}
            try:
                for side,m in mysqls.items():
                    obs=save('mysql-'+side,case,m.query(sql) if mode=='Q' else m.prepared(sql,[()]),mode)
                    phases=[obs] if mode=='Q' else obs
                    if fn.startswith(('current_','pg_')):
                        assert any(p['code']==1305 and p['state']=='42000' for p in phases),obs
                    else:
                        assert all(p['code']==0 for p in phases),obs
                        result=phases[-1]
                        if fn=='database':assert result['rows']==[[database.encode().hex()]],obs
                        elif fn=='health':assert result['rows']==[['37']],obs
                        elif fn=='version':
                            # VERSION must match the independent client's handshake identity.
                            version=m.lib.mysql_get_server_info;version.restype=c.c_char_p;version.argtypes=[c.c_void_p]
                            assert result['rows']==[[version(m.conn).hex()]],dict(observation=obs,handshake_version=version(m.conn).decode())
                        else:
                            thread=m.lib.mysql_thread_id;thread.restype=c.c_ulong;thread.argtypes=[c.c_void_p]
                            assert int(bytes.fromhex(result['rows'][0][0]))==thread(m.conn),obs
                    peers[side]=obs
                if fn in ('database','version','connection_id'):native_facts.append(case)
                if fn not in ('version','connection_id'):
                    if mode=='Q':mysql_compare(peers['reference'],peers['aster'])
                    else:assert peers['reference']==peers['aster'],peers
                else:
                    # Values are connection/build facts; compare all column definitions and diagnostics.
                    a,b=copy.deepcopy(peers['reference']),copy.deepcopy(peers['aster'])
                    for left,right in zip([a] if mode=='Q' else a,[b] if mode=='Q' else b):
                        if fn=='version':
                            for record in (left,right):
                                for field in record['metadata']:
                                    # Both Prepare and Execute describe VERSION's own build string.
                                    side='reference' if record is left else 'aster'
                                    raw=mysqls[side].lib.mysql_get_server_info(mysqls[side].conn)
                                    assert field[4]==len(raw)*4,(case,field,len(raw))
                                    field[4]=0
                        left['rows']=right['rows']=[]
                    assert a==b,(case,a,b)
                passed.append(case)
            except AssertionError as error:
                failures[case] = str(error)
        interleave('after')
        assert set(passed)|set(failures)==set(CASE_IDS) and len(passed)==len(set(passed))
        assert not failures, failures
    finally:
        # Roll back every connection before dropping only this fixture's objects.
        for conn in conns.values():
            if conn and tx(conn) in (2,3):query(conn,'ROLLBACK')
        for key,conn in conns.items():
            if conn:
                cleanup[key+'-table-dropped']=query(conn,'DROP TABLE IF EXISTS pg0832_source')['status']==1
                pq.PQfinish(conn)
        if 'admin' in locals() and admin:
            cleanup['reference_database_dropped']=all(query(admin,f'DROP DATABASE {db}')['status']==1 for db in owned_pg)
            pq.PQfinish(admin)
        if owned_mysql:cleanup['mysql-reference-database-dropped']=mysqls['reference'].query(f'DROP DATABASE {database}')['code']==0
        for m in mysqls.values():m.close()
        cat=lambda x:'zero' if x.startswith(('PG-zero-','PG-rowzero-')) else 'null' if x.startswith('PG-allnull-') else 'binding' if x.endswith(('P0','P1')) else 'error-recovery' if x.startswith('error-') else 'interleaved' if x.startswith('I-') else 'mysql' if x.startswith('M-') else 'reconnect' if x=='reconnect' else 'success'
        summary=dict(failures=failures,mysql_native_facts_passed=native_facts, null_input="no input parameters; outer typed NULL projection and zero rows covered", recovery_groups=6, expected=len(CASE_IDS),actual=len(attempted),unique=len(attempted),passed=len(passed),failed=len(attempted-set(passed)),skipped=len(set(CASE_IDS)-attempted),negative_self_checks=3,connections_closed=True,reference_database_dropped=cleanup.get('reference_database_dropped',False),cleanup=cleanup,case_ids=passed,expected_case_ids=CASE_IDS,counts={k:sum(cat(x)==k for x in passed) for k in set(map(cat,CASE_IDS))})
        (evidence/'summary.json').write_text(json.dumps(summary,indent=2))
    assert all(cleanup.values()),cleanup
    print(json.dumps(summary))

if __name__=='__main__':
    signal.signal(signal.SIGTERM,lambda *_:sys.exit(143))
    assert self_test()==3
    if sys.argv[1:]!=['--self-test']:run(int(sys.argv[1]),sys.argv[2],Path(sys.argv[3]),int(sys.argv[4]))
