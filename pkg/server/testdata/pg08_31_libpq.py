# Copyright 2026 AsterSQL.
"""Analyzed pg_backend_pid identity, NULLs and protocol metadata through native clients."""
import copy
import ctypes as c
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import select
from pg_function_23_libpq import Libpq, compare, self_test
from pg_function_27_libpq import Mysql
from pg_function_25_libpq import mysql_compare

CASES = (
 ('direct','pg_backend_pid()',False,False),
 ('qualified','pg_catalog.pg_backend_pid()',False,False),
 ('nested','COALESCE(pg_backend_pid(),NULL::int4)',False,False),
 ('row','pg_backend_pid()',False,False),
 ('zero','pg_backend_pid()',True,False),
 ('rowzero','pg_backend_pid()',True,False),
 ('allnull','NULL::int4',False,True),
)
CASE_IDS = tuple([f'PG-{label}-{mode}' for label,*_ in CASES for mode in ('Q','P0','P1')]
 + [f'error-{case}-{phase}' for case in ('arity','nullarity') for phase in ('Q','P','txn')]
 + [f'M-{fn}-{mode}' for fn in ('current_database','current_schema','pg_typeof','pg_backend_pid','database','version','connection_id','health') for mode in ('Q','P')]
 + ['I-before','I-middle','I-after','reconnect','cancel-A'])
assert len(CASE_IDS)==48 and len(set(CASE_IDS))==48


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
    backend = pq.lib.PQbackendPID; backend.restype=c.c_int; backend.argtypes=[c.c_void_p]
    def check_pg(obs, pid, zero=False, null=False, fmt=0):
        assert 0 < pid <= 2147483647
        assert obs['status']==2 and obs['metadata']==[[23,4,-1,fmt,'v',0,0]],obs
        payload=(pid.to_bytes(4,'big') if fmt else str(pid).encode()).hex()
        assert obs['rows']==([] if zero else [[None if null else payload]]),obs
    def equivalent(a,b):
        # First assert each raw PID equals that connection's BackendKeyData.
        # Compare all portable fields after replacing only proven PID payloads.
        a,b=copy.deepcopy(a),copy.deepcopy(b)
        a['rows']=b['rows']=[]
        compare(a,b)
    def interleave(label):
        case='I-'+label
        for side,conn in conns.items():
            obs=save(side,case,query(conn,'SELECT pg_backend_pid() AS v'))
            check_pg(obs,backend(conn))
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
        save('build','identity',dict(head=subprocess.check_output(['git','rev-parse','HEAD'],cwd=repo,text=True).strip(),fixture_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),libpq=pq.PQlibVersion(),binaries=bins,tzdata_version=tz.splitlines()[0].decode(),tzdata_sha256=hashlib.sha256(tz).hexdigest(),source_diff_sha256=hashlib.sha256(subprocess.check_output(['git','diff','HEAD','--','pkg'],cwd=repo)).hexdigest(),rust_binary_sha256=hashlib.sha256(Path(os.environ['PG08_31_RUST_BINARY']).read_bytes()).hexdigest()))
        for suffix in ('','_b'):
            db=database+suffix
            assert query(admin,f'CREATE DATABASE {db}')['status']==1
            owned_pg.append(db)
            for side,dsn in [('reference',os.environ['PG18_REFERENCE_DSN']+f' dbname={database}'),('aster',f'host=127.0.0.1 port={port} user=root dbname={database}')]:
                key=side+('-B' if suffix else '-A')
                conn=pq.PQconnectdb(dsn.encode());assert conn and pq.PQstatus(conn)==0,'login failed (DSN redacted)';conns[key]=conn
                if not suffix:
                    assert query(conn,'CREATE TABLE pg0831_source(v integer PRIMARY KEY)')['status']==1
                    assert query(conn,'INSERT INTO pg0831_source VALUES(7)')['status']==1
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
        for side in ('reference','aster'):
            assert backend(conns[side+'-A'])!=backend(conns[side+'-B'])
        for label,expr,zero,null in CASES:
            sql=f'SELECT {expr} AS v'+(' FROM pg0831_source' if label in ('row','rowzero','allnull') else '')+(' WHERE false' if zero else ' WHERE pg_backend_pid()>0' if null else '')
            for mode in ('Q','P0','P1'):
                case=f'PG-{label}-{mode}';peers={}
                for side,conn in conns.items():
                    fmt=0 if mode=='Q' else int(mode[-1])
                    if mode=='Q':obs=query(conn,sql)
                    else:
                        statement=('pid_'+label+mode).encode()
                        assert pq.observe(pq.PQprepare(conn,statement,sql.encode(),0,None))['status']==1
                        desc=save(side,'describe',pq.observe(pq.PQdescribePrepared(conn,statement)))
                        assert desc['parameters']==[] and desc['metadata']==[[23,4,-1,0,'v',0,0]],desc
                        obs=pq.observe(pq.PQexecPrepared(conn,statement,0,None,None,None,fmt))
                    save(side,case,obs,mode);check_pg(obs,backend(conn),zero,null,fmt);peers[side]=obs
                for suffix in ('A','B'):equivalent(peers['reference-'+suffix],peers['aster-'+suffix])
                passed.append(case)
        # libpq's own cancellation framing must not poison an idle backend.
        create=pq.lib.PQcancelCreate;create.restype=c.c_void_p;create.argtypes=[c.c_void_p]
        blocking=pq.lib.PQcancelBlocking;blocking.restype=c.c_int;blocking.argtypes=[c.c_void_p]
        finish=pq.lib.PQcancelFinish;finish.restype=None;finish.argtypes=[c.c_void_p]
        for side,conn in conns.items():
            cancel=create(conn);assert cancel
            try:assert blocking(cancel)==1
            finally:finish(cancel)
            check_pg(save(side,'idle-cancel-recovery',query(conn,'SELECT pg_backend_pid() AS v')),backend(conn))
        # A waits only for B's row lock on our own fixture. Cancel A with
        # libpq's BackendKeyData secret, while B remains usable and unchanged.
        send=pq.lib.PQsendQuery;send.restype=c.c_int;send.argtypes=[c.c_void_p,c.c_char_p]
        result=pq.lib.PQgetResult;result.restype=c.c_void_p;result.argtypes=[c.c_void_p]
        consume=pq.lib.PQconsumeInput;consume.restype=c.c_int;consume.argtypes=[c.c_void_p]
        busy=pq.lib.PQisBusy;busy.restype=c.c_int;busy.argtypes=[c.c_void_p]
        for side in ('reference','aster'):
            a,b=conns[side+'-A'],conns[side+'-B']
            assert query(b,'BEGIN')['status']==1
            lock=query(b,'SELECT v FROM pg0831_source WHERE v=7 FOR UPDATE')
            assert lock['status']==2 and lock['rows']==[['37']],lock
            assert query(a,'BEGIN')['status']==1
            assert send(a,b'SELECT v FROM pg0831_source WHERE v=7 FOR UPDATE')==1
            # A must actually be blocked before cancellation; fast success is a
            # broken lock fixture, not evidence of a canceled query.
            if select.select([pq.PQsocket(a)],[],[],0.15)[0]:assert consume(a)==1
            assert busy(a)==1, 'A did not wait for the owned row lock'
            check_pg(save(side,'cancel-B-live',query(b,'SELECT pg_backend_pid() AS v')),backend(b))
            cancel=create(a);assert cancel
            try:assert blocking(cancel)==1
            finally:finish(cancel)
            obs=save(side,'cancel-A',pq.observe(result(a)),'locked-select-execution')
            assert obs['status']==7 and obs['state']=='57014' and not obs['rows'],obs
            assert all(obs['errors'][field] for field in 'SVCM'),obs
            assert not result(a)
            assert tx(a)==3 and tx(b)==2
            assert query(a,'ROLLBACK')['status']==1 and tx(a)==0
            assert query(b,'COMMIT')['status']==1 and tx(b)==0
            check_pg(save(side,'cancel-A-recovery',query(a,'SELECT pg_backend_pid() AS v')),backend(a))
            assert query(b,'SELECT v FROM pg0831_source')['rows']==[['37']]
        passed.append('cancel-A')
        for side in ('reference','aster'):
            old=conns.pop(side+'-A');pq.PQfinish(old)
            dsn=(os.environ['PG18_REFERENCE_DSN']+f' dbname={database}') if side=='reference' else f'host=127.0.0.1 port={port} user=root dbname={database}'
            conn=pq.PQconnectdb(dsn.encode());assert conn and pq.PQstatus(conn)==0
            conns[side+'-A']=conn
            assert backend(conn)!=backend(conns[side+'-B'])
            check_pg(save(side,'reconnect',query(conn,'SELECT pg_backend_pid() AS v')),backend(conn))
        passed.append('reconnect')
        interleave('middle')
        for label,sql in [('arity','SELECT pg_backend_pid(1::int4) AS v'),('nullarity','SELECT pg_backend_pid(NULL::int4) AS v')]:
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
                assert query(conn,'SELECT v FROM pg0831_source')['rows']==[['37']]
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
                            for side,record in (('reference',left),('aster',right)):
                                raw=mysqls[side].lib.mysql_get_server_info(mysqls[side].conn)
                                assert len(record['metadata'])==1,record
                                field=record['metadata'][0]
                                # Native VERSION describes its own handshake build string.
                                # Prove the utf8mb4 byte capacity before comparing peers;
                                # code/flags/charset/decimals and every diagnostic stay exact.
                                assert field[4]==len(raw)*4,(case,side,field,len(raw))
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
        # Release every transaction before any DROP: B may hold A's fixture lock.
        for conn in conns.values():
            if conn and tx(conn) in (2,3):query(conn,'ROLLBACK')
        for key,conn in conns.items():
            if conn:
                cleanup[key+'-table-dropped']=query(conn,'DROP TABLE IF EXISTS pg0831_source')['status']==1
                pq.PQfinish(conn)
        if 'admin' in locals() and admin:
            cleanup['reference_database_dropped']=all(query(admin,f'DROP DATABASE {db}')['status']==1 for db in owned_pg)
            pq.PQfinish(admin)
        if owned_mysql:cleanup['mysql-reference-database-dropped']=mysqls['reference'].query(f'DROP DATABASE {database}')['code']==0
        for m in mysqls.values():m.close()
        cat=lambda x:'zero' if x.startswith(('PG-zero-','PG-rowzero-')) else 'null' if x.startswith('PG-allnull-') else 'binding' if x.endswith(('P0','P1')) else 'error-recovery' if x.startswith('error-') else 'interleaved' if x.startswith('I-') else 'mysql' if x.startswith('M-') else 'reconnect' if x=='reconnect' else 'cancellation' if x=='cancel-A' else 'success'
        summary=dict(failures=failures,mysql_native_facts_passed=native_facts, null_input="no input parameters; outer typed NULL projection and zero rows covered", recovery_groups=7, expected=len(CASE_IDS),actual=len(attempted),unique=len(attempted),passed=len(passed),failed=len(attempted-set(passed)),skipped=len(set(CASE_IDS)-attempted),negative_self_checks=3,connections_closed=True,reference_database_dropped=cleanup.get('reference_database_dropped',False),cleanup=cleanup,case_ids=passed,expected_case_ids=CASE_IDS,counts={k:sum(cat(x)==k for x in passed) for k in set(map(cat,CASE_IDS))})
        (evidence/'summary.json').write_text(json.dumps(summary,indent=2))
    assert all(cleanup.values()),cleanup
    print(json.dumps(summary))

if __name__=='__main__':
    signal.signal(signal.SIGTERM,lambda *_:sys.exit(143))
    assert self_test()==3
    if sys.argv[1:]!=['--self-test']:run(int(sys.argv[1]),sys.argv[2],Path(sys.argv[3]),int(sys.argv[4]))
