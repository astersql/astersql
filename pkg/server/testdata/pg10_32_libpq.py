# Copyright 2026 AsterSQL.
"""Writable CTE frozen cases, real libpq and MySQL8 clients; no SQL mocks."""
from contextlib import contextmanager
import ctypes as c
import itertools
import json
import os
from pathlib import Path
import signal
import struct
import sys
from pg_function_23_libpq import Libpq, Wire, Mysql, MysqlBind


def encoded(rows):
    return [[None if v is None else str(v).encode().hex() for v in row] for row in rows]



def run(evidence, port, database, mysql_port):
    evidence.mkdir(parents=True, exist_ok=True)
    pq = Libpq()
    summary = dict(mysql_client_found_rows=False, passed=False, cases=0, mysql_cases=0, libpq=pq.PQlibVersion(), cleanup={}, failures=[])
    pq.lib.PQcmdStatus.restype = c.c_char_p
    pq.lib.PQcmdStatus.argtypes = [c.c_void_p]
    connections, mysqls = [], []
    prepared_scope = None
    observations = {}
    table = 'public.cte_move_src'
    destination = 'public.cte_move_dst'
    scan = f'SELECT id,v FROM {table} ORDER BY id'
    base = [(1,10),(2,None)]
    target = f'host=127.0.0.1 port={port} user=root dbname={database} sslmode=disable'

    def save(side, case, phase, sql, obs):
        with (evidence / (side + '.jsonl')).open('a') as f:
            f.write(json.dumps(dict(case=case, phase=phase, sql=sql, observation=obs)) + '\n')
        return obs

    def query(wire, side, case, phase, sql, ready=None):
        obs = save(side, case, phase, sql, wire.query(sql))
        if ready:
            assert not obs['errors'] and obs['ready'] == ready, (case, phase, obs)
        return obs

    def readback(wire, side, case, phase, expected):
        obs = query(wire, side, case, phase, scan)
        assert not obs['errors'] and obs['rows'] == encoded(expected), (case, phase, obs, expected)
        return obs

    def parse(w, side, case, sql, oids):
        obs = save(side, case, 'parse', sql, w.phase(b'P', b's\0' + sql.encode() + b'\0' + struct.pack('!H', len(oids)) + b''.join(struct.pack('!I', o) for o in oids), '1'))
        if obs['errors']:
            save(side, case, 'parse-sync', sql, w.sync())
        return obs

    def bind(w, side, case, values, portal='p'):
        body = portal.encode() + b'\0s\0' + struct.pack('!HH', 0, len(values))
        for value in values:
            data = None if value is None else str(value).encode()
            body += struct.pack('!i', -1 if data is None else len(data)) + (data or b'')
        obs = save(side, case, 'bind', '', w.phase(b'B', body + struct.pack('!H', 0), '2'))
        assert not obs['errors'], obs

    def execute(w, side, case, sql, extended, values=()):
        if not extended:
            return query(w, side, case, 'execute', sql)
        bind(w, side, case, values)
        desc = save(side, case, 'portal', sql, w.phase(b'D', b'Pp\0', 'Tn'))
        assert not desc['errors'], desc
        w.send(b'E', b'p\0' + struct.pack('!I', 0))
        w.send(b'H')
        result = w.read('C')
        sync = w.sync()
        result['ready'] = sync['ready']
        result['metadata'] = desc['metadata']
        return save(side, case, 'execute', sql, result)

    @contextmanager
    def mysql_prepared_int(client, case, sql, values=(1,), allow_warnings=False):
        # Preserve the real statement across before/during/after PG observations.
        stmt = client.mysql_stmt_init(client.conn)
        assert stmt
        try:
            assert client.mysql_stmt_prepare(stmt, sql.encode(), len(sql.encode())) == 0
            assert client.mysql_stmt_param_count(stmt) == len(values)
            params=(MysqlBind*len(values))()
            keep=[]
            for i,value in enumerate(values):
                buf=c.create_string_buffer(value.encode()) if isinstance(value,str) else c.c_int(value)
                length=c.c_ulong(len(value.encode()) if isinstance(value,str) else 4)
                null=c.c_bool(False)
                keep.extend((buf,length,null))
                params[i].buffer_type=253 if isinstance(value,str) else 3
                params[i].buffer=c.addressof(buf)
                params[i].buffer_length=length.value
                params[i].length,params[i].is_null=c.addressof(length),c.addressof(null)
            assert not client.mysql_stmt_bind_param(stmt,params)
            def execute_native(label):
                assert client.mysql_stmt_execute(stmt) == 0
                result = client.mysql_stmt_result_metadata(stmt)
                try:
                    meta = client.metadata(result)
                finally:
                    if result: client.mysql_free_result(result)
                client.lib.mysql_stmt_affected_rows.restype = c.c_ulonglong
                client.lib.mysql_stmt_affected_rows.argtypes = [c.c_void_p]
                obs = dict(metadata=meta, rows=[], code=client.mysql_stmt_errno(stmt),
                           state=client.mysql_stmt_sqlstate(stmt).decode(),
                           affected_rows=client.lib.mysql_stmt_affected_rows(stmt),
                           warning_count=client.mysql_warning_count(client.conn))
                if meta:
                    buffers = [c.create_string_buffer(4096) for _ in meta]
                    lengths = [c.c_ulong() for _ in meta]
                    nulls = [c.c_bool() for _ in meta]
                    errors = [c.c_bool() for _ in meta]
                    binds = (MysqlBind * len(meta))()
                    for i in range(len(meta)):
                        binds[i].buffer_type = 253
                        binds[i].buffer = c.addressof(buffers[i])
                        binds[i].buffer_length = 4096
                        binds[i].length, binds[i].is_null = c.addressof(lengths[i]), c.addressof(nulls[i])
                        binds[i].error = c.addressof(errors[i])
                    assert not client.mysql_stmt_bind_result(stmt, binds)
                    assert client.mysql_stmt_store_result(stmt) == 0
                    while True:
                        rc = client.mysql_stmt_fetch(stmt)
                        if rc == 100: break
                        assert rc == 0 and not any(e.value for e in errors)
                        obs['rows'].append([None if nulls[i].value else buffers[i].raw[:lengths[i].value].hex() for i in range(len(meta))])
                    assert not client.mysql_stmt_free_result(stmt)
                obs['warnings'] = client.query('SHOW WARNINGS', warnings=False)['rows']
                save('mysql', case, label, sql, obs)
                assert obs['code'] == 0 and (allow_warnings or (obs['warning_count'] == 0 and not obs['warnings'])), obs
                return obs
            yield execute_native
        finally:
            assert not client.mysql_stmt_close(stmt)

    def mysql_query(client, case, sql):
        obs = client.query(sql, warnings=False)
        client.lib.mysql_affected_rows.restype = c.c_ulonglong
        client.lib.mysql_affected_rows.argtypes = [c.c_void_p]
        obs['affected_rows'] = client.lib.mysql_affected_rows(client.conn)
        obs['warnings'] = client.query('SHOW WARNINGS', warnings=False)['rows']
        save('mysql', case, 'query', sql, obs)
        assert obs['code'] == 0 and obs['warning_count'] == 0, obs
        return obs

    try:
        for _ in range(2): mysqls.append(Mysql('127.0.0.1',mysql_port,'root','',database))
        m,n=mysqls
        for client in mysqls: mysql_query(client,'setup','SET autocommit=1')
        mysql_query(m,'setup','CREATE TABLE cte_move_mysql(id INT PRIMARY KEY,v INT)')
        mysql_query(m,'setup','INSERT INTO cte_move_mysql VALUES(1,10),(2,NULL)')
        mysql_query(m,'identity','SELECT @@sql_mode,@@transaction_isolation,DATABASE()')
        native_sql='SELECT id,v FROM cte_move_mysql ORDER BY id'
        native_scan='SELECT id,v FROM cte_move_mysql ORDER BY id'
        with mysql_prepared_int(m,'before-zero','SELECT id,v FROM cte_move_mysql WHERE id=? ORDER BY id',(3,)) as read:
            assert read('empty')['rows']==[]
        mysql_query(m,'before','BEGIN')
        mysql_query(m,'before','UPDATE cte_move_mysql SET v=11 WHERE id=1')
        prepared_scope=mysql_prepared_int(m,'coexist','SELECT id,v FROM cte_move_mysql WHERE id=? ORDER BY id',(2,))
        prepared_read=prepared_scope.__enter__()
        native_base=mysql_query(m,'before',native_sql)
        prepared_base=prepared_read('before')
        assert native_base['rows']==encoded([(1,11),(2,None)])
        assert prepared_base['rows']==encoded([(2,None)])
        def coexist(label):
            assert mysql_query(m,label,native_sql)==native_base
            assert prepared_read(label)==prepared_base
            assert mysql_query(m,label,native_scan)['rows']==encoded([(1,11),(2,None)])
            assert mysql_query(n,label,native_scan)['rows']==encoded(base)
        for side,dsn in [('reference',os.environ['PG18_REFERENCE_DSN']),('target',target)]:
            w,o=Wire(pq,dsn),Wire(pq,dsn)
            connections.extend((w,o))
            identity=query(w,side,'identity','version','SELECT version()','I')
            if side=='reference':
                assert query(w,side,'identity','number','SHOW server_version_num','I')['rows']==encoded([(180006,)])
                summary['reference_build']=identity['rows']
            for conn in (w,o):
                query(conn,side,'setup','encoding',"SET client_encoding='UTF8'",'I')
                query(conn,side,'setup','timezone',"SET TIME ZONE 'UTC'" if side=='reference' else "SET time_zone='+00:00'",'I')
                query(conn,side,'setup','isolation','SHOW transaction_isolation' if side=='reference' else 'SELECT @@transaction_isolation','I')
            base_src=[(1,10),(2,None),(3,30)]
            moved=[(1,10),(2,None)]
            move=f'WITH moved AS (DELETE FROM {table} WHERE id<=2 RETURNING id,v) INSERT INTO {destination}(id,v) SELECT id,v FROM moved RETURNING id,v'
            def check(conn,case,phase,src,dst):
                for name,expected in ((table,src),(destination,dst)):
                    obs=query(conn,side,case,phase,f'SELECT id,v FROM {name} ORDER BY id')
                    assert not obs['errors'] and obs['rows']==encoded(expected),(case,phase,obs,expected)
            def setup(case):
                for name in (table,destination): query(w,side,case,'create',f'CREATE TABLE {name}(id integer PRIMARY KEY,v integer)','I')
                query(w,side,case,'seed',f'INSERT INTO {table} VALUES(1,10),(2,NULL),(3,30)','I')
                for conn in (w,o): check(conn,case,'before',base_src,[])
            def cleanup(case):
                w.sync(); query(w,side,case,'rollback','ROLLBACK','I')
                for tag,body in ((b'C',b'Pp\0'),(b'C',b'Ss\0')):
                    w.phase(tag,body,'3'); w.sync()
                for name in (destination,table): query(w,side,case,'drop',f'DROP TABLE IF EXISTS {name}','I')
            successes=[('move',move,moved,base_src[2:]),('empty',move.replace('id<=2','id<0'),[],base_src),
                ('snapshot',f'WITH moved AS (DELETE FROM {table} WHERE id<=2 RETURNING id,v), seen AS (SELECT id,v FROM {table} WHERE id<=2) INSERT INTO {destination} SELECT seen.id,seen.v FROM seen JOIN moved ON seen.id=moved.id RETURNING id,v',moved,base_src[2:]),
                ('repeat',f'WITH moved AS (DELETE FROM {table} WHERE id<=2 RETURNING id,v) INSERT INTO {destination} SELECT a.id,a.v FROM moved AS a JOIN moved AS b ON a.id=b.id RETURNING id,v',moved,base_src[2:])]
            successes += [(name+'-no-return',sql.rsplit(' RETURNING id,v',1)[0],expected,src) for name,sql,expected,src in successes]
            for (name,sql,expected,src),extended,ending in itertools.product(successes,(False,True),('COMMIT','ROLLBACK',None)):
                case=f'{name}-{extended}-{ending}'
                try:
                    setup(case)
                    if ending: query(w,side,case,'begin','BEGIN','T')
                    if extended:
                        assert not parse(w,side,case,sql,[])['errors']
                        desc=save(side,case,'statement',sql,w.phase(b'D',b'Ss\0','Tn'))
                        assert not desc['errors']; w.sync()
                        for conn in (w,o): check(conn,case,'describe-no-effect',base_src,[])
                    result=execute(w,side,case,sql,extended)
                    assert not result['errors'] and result['ready']==('T' if ending else 'I'),(case,result)
                    assert sorted(result['rows'],key=str)==sorted(encoded([] if name.endswith('-no-return') else expected),key=str),(case,result)
                    assert result['commands']==[f'INSERT 0 {len(expected)}'],(case,result)
                    assert all(col[:4]==[23,4,-1,0] for col in result['metadata']),result
                    if extended: assert result['metadata']==desc['metadata'],(result,desc)
                    observations.setdefault(case,{})[side]=dict(rows=sorted(result['rows'],key=str),metadata=[col[:5]+[bool(col[5]),col[6]] for col in result['metadata']],commands=result['commands'])
                    check(w,case,'writer',src,expected)
                    check(o,case,'observer',base_src if ending else src,[] if ending else expected)
                    if ending: query(w,side,case,'end',ending,'I')
                    for conn in (w,o): check(conn,case,'final',base_src if ending=='ROLLBACK' else src,[] if ending=='ROLLBACK' else expected)
                    assert query(w,side,case,'recovery','SELECT 1','I')['rows']==encoded([(1,)])
                    summary['cases']+=1
                finally: cleanup(case)
            errors=[('destination',move,'23505'),('source',move.replace('RETURNING id,v)', 'RETURNING id,10/(id-2) AS v)'),'22012'),
                ('nested',f'SELECT * FROM (WITH moved AS (DELETE FROM {table} WHERE id<=2 RETURNING id,v) SELECT * FROM moved) AS x','0A000'),
                ('recursive-write',f'WITH RECURSIVE moved AS (DELETE FROM {table} WHERE id IN (SELECT id FROM moved) RETURNING id,v) SELECT * FROM moved','42P19'),
                ('recursive',f'WITH RECURSIVE keys AS (SELECT 1 AS id) DELETE FROM {table} WHERE id IN (SELECT id FROM keys) RETURNING id,v','limit'),
                ('consumer',f'WITH moved AS (DELETE FROM {table} WHERE id<=2 RETURNING id,v) SELECT * FROM moved','limit'),
                ('unused',f'WITH moved AS (DELETE FROM {table} WHERE id<=2 RETURNING id,v) SELECT 7 AS marker','limit'),
                ('unreturned',f'WITH moved AS (DELETE FROM {table} WHERE id<=2) INSERT INTO {destination} VALUES(4,40)','limit')]
            for (name,sql,state),extended,transaction in itertools.product(errors,(False,True),(False,True)):
                case=f'{name}-{extended}-{transaction}'
                try:
                    setup(case)
                    dst=[(2,200)] if name=='destination' else []
                    if dst: query(w,side,case,'duplicate',f'INSERT INTO {destination} VALUES(2,200)','I')
                    if transaction:
                        query(w,side,case,'begin','BEGIN','T')
                        query(w,side,case,'prefix',f'INSERT INTO {table} VALUES(9,90)','T')
                    phase='query'
                    if extended:
                        phase='parse'; result=parse(w,side,case,sql,[])
                        if not result['errors']: phase='execute'; result=execute(w,side,case,sql,True)
                    else: result=query(w,side,case,'execute',sql)
                    ready=w.sync()['ready']
                    if state=='limit' and side=='reference':
                        assert not result['errors'],(case,result)
                        expected={'recursive':[(1,10)],'consumer':moved,'unused':[(7,)],'unreturned':[]}[name]
                        assert sorted(result['rows'],key=str)==sorted(encoded(expected),key=str),result
                        successful_src=base_src[1:] if name=='recursive' else base_src[2:]
                        successful_dst=[(4,40)] if name=='unreturned' else []
                        check(w,case,'limit-writer',successful_src+([(9,90)] if transaction else []),successful_dst)
                        check(o,case,'limit-observer',base_src if transaction else successful_src,[] if transaction else successful_dst)
                    else:
                        assert [e['C'] for e in result['errors']]==['0A000' if state=='limit' else state],(case,result)
                        assert not result['rows'] and not result['commands'],result
                        assert ready==('E' if transaction else 'I'),(case,ready)
                        check(o,case,'observer',base_src,dst)
                        if transaction:
                            failed=query(w,side,case,'failed','SELECT 1')
                            assert [e['C'] for e in failed['errors']]==['25P02'] and w.sync()['ready']=='E',failed
                        if state!='limit': observations.setdefault(case,{})[side]=dict(state=state,phase=phase,ready=ready)
                    query(w,side,case,'rollback','ROLLBACK','I')
                    for conn in (w,o):
                        check(conn,case,'restored',successful_src if state=='limit' and not transaction and side=='reference' else base_src,successful_dst if state=='limit' and not transaction and side=='reference' else dst)
                    assert query(w,side,case,'recovery','SELECT 1','I')['rows']==encoded([(1,)])
                    summary['cases']+=1
                finally: cleanup(case)
            for ending in ('COMMIT','ROLLBACK'):
                case='parameters-'+ending
                try:
                    setup(case)
                    sql=move.replace('id<=2','id<=$1')
                    assert not parse(w,side,case,sql,[23])['errors']
                    desc=save(side,case,'statement',sql,w.phase(b'D',b'Ss\0','Tn'))
                    assert desc['parameters']==[23] and not desc['errors']; w.sync()
                    for conn in (w,o): check(conn,case,'describe-no-effect',base_src,[])
                    query(w,side,case,'begin','BEGIN','T')
                    for value in (0,None,2,0):
                        bind(w,side,case,[value])
                        portal=save(side,case,'portal',sql,w.phase(b'D',b'Pp\0','Tn'))
                        result=dict(rows=[],commands=[],errors=[],metadata=portal['metadata'])
                        while True:
                            part=save(side,case,'segment',sql,w.phase(b'E',b'p\0'+struct.pack('!I',1),'sC'))
                            for key in ('rows','commands','errors'): result[key]+=part[key]
                            assert w.sync()['ready']=='T'
                            check(o,case,'segment-observer',base_src,[])
                            if part['commands'] or part['errors']: break
                        expected=moved if value==2 else []
                        assert not result['errors'] and sorted(result['rows'],key=str)==sorted(encoded(expected),key=str),result
                        assert result['commands']==['INSERT 0 0'] and result['metadata']==desc['metadata'],result
                        observations.setdefault(case+'-'+str(value),{})[side]=dict(rows=sorted(result['rows'],key=str),commands=result['commands'],metadata=[col[:5]+[bool(col[5]),col[6]] for col in result['metadata']])
                        if value==2: query(w,side,case,'reinstate',f'INSERT INTO {table} VALUES(1,10)','T')
                        repeated=save(side,case,'repeat',sql,w.phase(b'E',b'p\0'+struct.pack('!I',0),'C'))
                        assert not repeated['rows'] and not repeated['errors'] and repeated['commands']==['INSERT 0 0'],repeated
                        if value==2:
                            check(w,case,'repeat-no-write',[(1,10),(3,30)],moved)
                            query(w,side,case,'remove-reinstated',f'DELETE FROM {table} WHERE id=1','T')
                        w.phase(b'C',b'Pp\0','3'); w.sync()
                        check(o,case,'observer',base_src,[])
                    check(w,case,'writer',base_src[2:],moved)
                    query(w,side,case,'end',ending,'I')
                    for conn in (w,o): check(conn,case,'final',base_src if ending=='ROLLBACK' else base_src[2:],[] if ending=='ROLLBACK' else moved)
                    summary['cases']+=1
                finally: cleanup(case)
            coexist(side)
        coexist('after')
        prepared_scope.__exit__(None,None,None); prepared_scope=None
        mysql_query(m,'after','COMMIT')
        for client in (m,n): assert mysql_query(client,'committed',native_scan)['rows']==encoded([(1,11),(2,None)])
        with mysql_prepared_int(m,'zero','SELECT id,v FROM cte_move_mysql WHERE id=? ORDER BY id',(1,)) as read:
            assert read('three')['rows']==encoded([(1,11)])
        with mysql_prepared_int(m,'zero','SELECT id,v FROM cte_move_mysql WHERE id=? ORDER BY id',(4,)) as read:
            assert read('empty')['rows']==[]
        mysql_query(m,'rollback-setup','UPDATE cte_move_mysql SET v=10 WHERE id=1')
        mysql_query(m,'rollback','BEGIN')
        mysql_query(m,'rollback','UPDATE cte_move_mysql SET v=11 WHERE id=1')
        assert mysql_query(n,'rollback-observer',native_scan)['rows']==encoded(base)
        with mysql_prepared_int(m,'rollback-read','SELECT id,v FROM cte_move_mysql WHERE id=? ORDER BY id',(1,)) as read:
            assert read('pending')['rows']==encoded([(1,11)])
            mysql_query(m,'rollback','ROLLBACK')
            assert read('restored')['rows']==encoded([(1,10)])
        for client in (m,n): assert mysql_query(client,'rollback-final',native_scan)['rows']==encoded(base)
        summary['mysql_cases']=2
        differences={case:sides for case,sides in observations.items() if sides['reference']!=sides['target']}
        (evidence/'differences.json').write_text(json.dumps(differences,indent=2))
        summary['differences']=len(differences)
        assert not differences, differences
        assert summary['cases']==164,summary
        summary['passed']=True
    except BaseException as error:
        summary['failures'].append(str(error)); raise
    finally:
        if prepared_scope is not None: prepared_scope.__exit__(None,None,None)
        for index,wire in enumerate(connections):
            try:
                wire.sync(); wire.query('ROLLBACK')
                result=save('reference' if index<2 else 'target','cleanup','drop',f'DROP TABLE IF EXISTS {table}',wire.query(f'DROP TABLE IF EXISTS {table}'))
                dropped=save('reference' if index<2 else 'target','cleanup','drop',f'DROP TABLE IF EXISTS {destination}',wire.query(f'DROP TABLE IF EXISTS {destination}'))
                assert not dropped['errors'] and dropped['ready']=='I',dropped
                assert not result['errors'] and result['ready']=='I',result
                summary['cleanup']['reference' if index<2 else 'target']=True
            finally: wire.close()
        summary['cleanup']['pg_connections']=len(connections)
        for client in mysqls:
            try:
                client.query('ROLLBACK'); client.query('DROP TABLE IF EXISTS cte_move_mysql')
            finally: client.close()
        summary['cleanup']['mysql_connections']=len(mysqls)
        (evidence/'summary.json').write_text(json.dumps(summary,indent=2))
    print(json.dumps(summary),flush=True)

if __name__=='__main__':
    signal.signal(signal.SIGTERM,lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    run(Path(sys.argv[1]),int(sys.argv[2]),sys.argv[3],int(sys.argv[4]))
