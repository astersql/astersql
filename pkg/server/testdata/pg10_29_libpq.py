# Copyright 2026 AsterSQL.
"""Conflict DO NOTHING frozen cases, real libpq and MySQL8 clients; no SQL mocks."""
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


def ordered(rows):
    return sorted(rows, key=lambda row: json.dumps(row))


def run(evidence, port, database, mysql_port):
    evidence.mkdir(parents=True, exist_ok=True)
    pq = Libpq()
    summary = dict(passed=False, cases=0, mysql_cases=0, libpq=pq.PQlibVersion(), cleanup={}, failures=[])
    pq.lib.PQcmdStatus.restype = c.c_char_p
    pq.lib.PQcmdStatus.argtypes = [c.c_void_p]
    connections, mysqls = [], []
    prepared_scope = None
    observations = {}
    table = 'public.conflict_doc'
    scan = f'SELECT id,email,v FROM {table} ORDER BY id'
    base = [(1,'a',10),(2,'b',20)]
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

    def close_statement(w):
        obs = w.phase(b'C', b'Ss\0', '3')
        assert not obs['errors'], obs
        assert w.sync()['ready'] == 'I'

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
        obs = client.query(sql)
        client.lib.mysql_affected_rows.restype = c.c_ulonglong
        client.lib.mysql_affected_rows.argtypes = [c.c_void_p]
        obs['affected_rows'] = client.lib.mysql_affected_rows(client.conn)
        save('mysql', case, 'query', sql, obs)
        assert obs['code'] == 0 and obs['warning_count'] == 0, obs
        return obs

    try:
        # Native M/N use a separate database on the same Server, never PG syntax.
        m = Mysql('127.0.0.1', mysql_port, 'root', '', database)
        mysqls.append(m)
        n = Mysql('127.0.0.1', mysql_port, 'root', '', database)
        mysqls.append(n)
        for client in mysqls:
            mysql_query(client, 'setup', 'SET autocommit=1')
        mysql_query(m, 'setup', 'CREATE TABLE returning_mysql_doc(id INT PRIMARY KEY,v INT,note TEXT)')
        mysql_query(m, 'setup', "INSERT INTO returning_mysql_doc VALUES(1,10,'before')")
        native_scan = 'SELECT id,v,note FROM returning_mysql_doc ORDER BY id'
        native_base = mysql_query(m, 'before', native_scan)
        mode_base = mysql_query(m, 'before', 'SELECT @@sql_mode,@@transaction_isolation,DATABASE()')
        mysql_query(m, 'before', 'BEGIN')
        mysql_query(m, 'before', 'UPDATE returning_mysql_doc SET v=v+1 WHERE id=1')
        assert mysql_query(m, 'before', native_scan)['rows'] == encoded([(1,11,'before')])
        assert mysql_query(n, 'before', native_scan)['rows'] == encoded([(1,10,'before')])
        prepared_scope = mysql_prepared_int(m, 'query-coexist', 'SELECT v,note FROM returning_mysql_doc WHERE id = ?')
        prepared_read = prepared_scope.__enter__()
        prepared_base = prepared_read('before')
        assert prepared_base['rows'] == encoded([(11,'before')])
        for side, dsn in [('reference', os.environ['PG18_REFERENCE_DSN']), ('target', target)]:
            w = Wire(pq, dsn)
            connections.append(w)
            o = Wire(pq, dsn)
            connections.append(o)
            identity = query(w, side, 'identity', 'version', 'SELECT version()', 'I')
            if side == 'reference':
                version = query(w, side, 'identity', 'version-num', 'SHOW server_version_num', 'I')
                assert version['rows'] == encoded([(180006,)]), 'exact PostgreSQL18.6 required; 18.4 is separate'
                summary['reference_build'] = identity['rows']
            for conn in (w,o):
                query(conn, side, 'setup', 'utf8', "SET client_encoding = 'UTF8'", 'I')
                query(conn, side, 'setup', 'utc', "SET TIME ZONE 'UTC'" if side == 'reference' else "SET time_zone = '+00:00'", 'I')
                query(conn, side, 'setup', 'isolation', 'SHOW transaction_isolation' if side == 'reference' else 'SELECT @@transaction_isolation', 'I')
            success = [
                ('existing', "(1,'changed',99)", '(id)', []),
                ('fresh', "(3,'c',30)", '(id)', [(3,'c',30)]),
                ('all-keys', "(3,'b',30)", '', []),
                ('named', "(3,'b',30)", 'ON CONSTRAINT conflict_doc_email_key', []),
                ('mixed', "(1,'changed',99),(3,'c',30)", '(id)', [(3,'c',30)]),
                ('null-email', "(3,NULL,30)", '(id)', [(3,None,30)]),
            ]
            errors = [
                ('other-key', "(3,'b',30)", '(id)', '23505', False),
                ('bad-target', "(3,'c',30)", '(v)', '42P10', True),
                ('bad-name', "(3,'c',30)", 'ON CONSTRAINT missing_constraint', '42704', True),
                ('not-null', "(3,'c',NULL)", '', '23502', False),
                ('check', "(3,'c',0)", '', '23514', False),
                ('expression', "(3,'c',1/0)", '', '22012', True),
                ('atomic', "(3,'c',30),(4,'b',40)", '(id)', '23505', False),
            ]
            def setup_case(case):
                query(w,side,case,'create',f'CREATE TABLE {table}(id integer CONSTRAINT conflict_doc_pkey PRIMARY KEY,email text CONSTRAINT conflict_doc_email_key UNIQUE,v integer NOT NULL CHECK(v>0))','I')
                query(w,side,case,'seed',f"INSERT INTO {table} VALUES(1,'a',10),(2,'b',20)",'I')
                for conn in (w,o): readback(conn,side,case,'initial',base)
            def cleanup_case(case, extended):
                w.sync()
                query(w,side,case,'cleanup-rollback','ROLLBACK','I')
                if extended: close_statement(w)
                query(w,side,case,'cleanup-drop',f'DROP TABLE IF EXISTS {table}','I')
            for name, candidates, arbiter, returned in success:
                for extended,commit,returning in itertools.product((False,True), repeat=3):
                    case=f'{name}-{extended}-{commit}-{returning}'
                    sql=f'INSERT INTO {table} VALUES {candidates} ON CONFLICT {arbiter} DO NOTHING'+(' RETURNING id,email,v' if returning else '')
                    values,oids=[],[]
                    if name=='fresh' and extended:
                        sql=f'INSERT INTO {table} VALUES ($1,$2,$3) ON CONFLICT (id) DO NOTHING'+(' RETURNING id,email,v' if returning else '')
                        values,oids=[3,'c',30],[23,25,23]
                    try:
                        setup_case(case)
                        desc=None
                        if extended:
                            assert not parse(w,side,case,sql,oids)['errors']
                            desc=save(side,case,'statement',sql,w.phase(b'D',b'Ss\0','Tn'))
                            assert not desc['errors'] and desc['parameters']==oids,desc
                            assert w.sync()['ready']=='I'
                            for conn in (w,o): readback(conn,side,case,'describe-no-write',base)
                        query(w,side,case,'begin','BEGIN','T')
                        result=execute(w,side,case,sql,extended,values)
                        assert not result['errors'] and result['ready']=='T',(case,result)
                        assert ordered(result['rows'])==ordered(encoded(returned if returning else [])),(case,result)
                        assert result['commands']==['INSERT 0 '+str(len(returned))],(case,result)
                        if desc: assert desc['metadata']==result['metadata'],(case,desc,result)
                        stored=base+returned
                        readback(w,side,case,'writer',stored)
                        readback(o,side,case,'observer',base)
                        query(w,side,case,'end','COMMIT' if commit else 'ROLLBACK','I')
                        for conn in (w,o): readback(conn,side,case,'final',stored if commit else base)
                        observations.setdefault(case,{})[side]=dict(rows=ordered(result['rows']),commands=result['commands'],metadata=[x[:5]+[bool(x[5]),x[6]] for x in result['metadata']])
                        summary['cases']+=1
                    finally: cleanup_case(case,extended)
            for name,candidates,arbiter,state,early in errors:
                for extended,transaction in itertools.product((False,True), repeat=2):
                    case=f'{name}-{extended}-{transaction}'
                    sql=f'INSERT INTO {table} VALUES {candidates} ON CONFLICT {arbiter} DO NOTHING'
                    try:
                        setup_case(case)
                        if transaction:
                            query(w,side,case,'begin','BEGIN','T')
                            query(w,side,case,'prefix',f"INSERT INTO {table} VALUES(9,'pending',90)",'T')
                        parsed=parse(w,side,case,sql,[]) if extended else None
                        if parsed and parsed['errors']:
                            assert early,(case,'unexpected Parse failure',parsed)
                            result,phase=parsed,'parse'
                        else:
                            if extended:
                                bound=save(side,case,'bind',sql,w.phase(b'B',b'p\0s\0'+struct.pack('!HHH',0,0,0),'2'))
                                if bound['errors']:
                                    assert early,(case,'unexpected Bind failure',bound)
                                    result,phase=bound,'bind'
                                else:
                                    result=save(side,case,'execute',sql,w.phase(b'E',b'p\0'+struct.pack('!I',0),'C'))
                                    phase='execute'
                            else:
                                result,phase=execute(w,side,case,sql,False),'query'
                        ready=save(side,case,'sync',sql,w.sync())['ready']
                        assert [e['C'] for e in result['errors']]==[state],(case,result)
                        assert not result['rows'] and not result['commands'],(case,result)
                        assert ready==('E' if transaction else 'I'),(case,ready)
                        readback(o,side,case,'observer-error',base)
                        if transaction:
                            failed=query(w,side,case,'failed-gate','SELECT 1')
                            assert [e['C'] for e in failed['errors']]==['25P02'],failed
                            assert w.sync()['ready']=='E'
                        query(w,side,case,'rollback','ROLLBACK','I')
                        for conn in (w,o): readback(conn,side,case,'restored',base)
                        query(w,side,case,'recovery','SELECT 1','I')
                        observations.setdefault(case,{})[side]=dict(state=state,phase=phase,ready=ready)
                        summary['cases']+=1
                    finally: cleanup_case(case,extended)
            gates = [
                ("(id) WHERE v > 0",False), ("(id) WHERE v < 0",False),
                ("(id) WHERE NULL",False), ("(id) WHERE true",False),
                ("(lower(email))",True), ("((lower(email)))",True), ("(lower(email))",True),
                ("((id+1))",True), ("(id,(v+1))",True),
                ("((id))",False), ("((v+1)) WHERE v>0",True),
            ]
            for gate_index,(arbiter,ref_error) in enumerate(gates):
                for extended,transaction,returning in itertools.product((False,True),repeat=3):
                    case=f'gate-{gate_index}-{arbiter}-{extended}-{transaction}-{returning}'
                    candidates="(1,'changed',99)" if gate_index in (3,4) else "(3,'c',30)"
                    skipped=gate_index==3
                    sql=f"INSERT INTO {table} VALUES{candidates} ON CONFLICT {arbiter} DO NOTHING"+(' RETURNING id,email,v' if returning else '')
                    try:
                        setup_case(case)
                        if extended:
                            assert not parse(w,side,case,sql,[])['errors']
                            assert w.sync()['ready']=='I'
                        if transaction:
                            query(w,side,case,'begin','BEGIN','T')
                            query(w,side,case,'prefix',f"INSERT INTO {table} VALUES(9,'pending',90)",'T')
                        phase='query'
                        if extended:
                            result=save(side,case,'bind',sql,w.phase(b'B',b'p\0s\0'+struct.pack('!HHH',0,0,0),'2'))
                            phase='bind'
                            if not result['errors']:
                                result=save(side,case,'execute',sql,w.phase(b'E',b'p\0'+struct.pack('!I',0),'C'))
                                phase='execute'
                            ready=save(side,case,'sync',sql,w.sync())['ready']
                        else:
                            result=query(w,side,case,'execute',sql)
                            ready=result.get('tail',result)['ready']
                        if side=='target' or ref_error:
                            state='0A000' if side=='target' else '42P10'
                            assert [e['C'] for e in result['errors']]==[state],(case,result)
                            assert not result['rows'] and not result['commands'],result
                            assert ready==('E' if transaction else 'I'),result
                            if transaction:
                                failed=query(w,side,case,'failed-gate','SELECT 1')
                                assert [e['C'] for e in failed['errors']]==['25P02'],failed
                                assert w.sync()['ready']=='E'
                        else:
                            assert not result['errors'] and result['commands']==['INSERT 0 '+str(int(not skipped))],(case,result)
                            assert result['rows']==encoded([(3,'c',30)] if returning and not skipped else []),result
                        readback(o,side,case,'observer',base if transaction or side=='target' or ref_error or skipped else base+[(3,'c',30)])
                        query(w,side,case,'rollback','ROLLBACK','I')
                        expected=base if transaction or side=='target' or ref_error or skipped else base+[(3,'c',30)]
                        for conn in (w,o): readback(conn,side,case,'restored',expected)
                        save(side,case,'gate-contract',sql,dict(phase=phase,ready=ready,product_limit=side=='target'))
                        summary['cases']+=1
                    finally: cleanup_case(case,extended)
            for commit in (False,True):
                case=f'typed-null-rebind-{commit}'
                sql=f'INSERT INTO {table} VALUES($1,$2,$3) ON CONFLICT(id) DO NOTHING RETURNING id,email,v'
                try:
                    setup_case(case)
                    assert not parse(w,side,case,sql,[23,25,23])['errors']
                    desc=save(side,case,'statement',sql,w.phase(b'D',b'Ss\0','T'))
                    assert desc['parameters']==[23,25,23] and not desc['errors'],desc
                    assert w.sync()['ready']=='I'
                    query(w,side,case,'begin','BEGIN','T')
                    stored=base[:]
                    for values in ([3,None,30],[4,'d',40]):
                        result=execute(w,side,case,sql,True,values)
                        assert not result['errors'] and result['ready']=='T',result
                        assert result['rows']==encoded([values]) and result['commands']==['INSERT 0 1'],result
                        assert result['metadata']==desc['metadata'],result
                        stored.append(tuple(values))
                        readback(w,side,case,'writer',stored)
                        readback(o,side,case,'observer',base)
                        assert not w.phase(b'C',b'Pp\0','3')['errors']
                    query(w,side,case,'end','COMMIT' if commit else 'ROLLBACK','I')
                    for conn in (w,o): readback(conn,side,case,'final',stored if commit else base)
                    summary['cases']+=1
                finally: cleanup_case(case,True)
            # Partial unique-index setup is legal on PG and explicitly gated
            # on the target. Keep reference execution separate from that limit.
            for returning,commit,extended in itertools.product((False,True),repeat=3):
                case=f'partial-{returning}-{commit}-{extended}'
                pt='public.conflict_partial'
                try:
                    query(w,side,case,'create',f'CREATE TABLE {pt}(id integer PRIMARY KEY,k integer,active boolean)','I')
                    query(w,side,case,'seed',f'INSERT INTO {pt} VALUES(1,7,true)','I')
                    idx=query(w,side,case,'index',f'CREATE UNIQUE INDEX conflict_partial_active ON {pt}(k) WHERE active')
                    if side=='target':
                        assert [e['C'] for e in idx['errors']]==['0A000'],idx
                        assert idx['tail']['ready']=='I',idx
                        summary.setdefault('limits',[]).append(dict(case=case,phase='setup',state='0A000'))
                    else:
                        assert not idx['errors'],idx
                        query(w,side,case,'begin','BEGIN','T')
                        for ident,active,count in ((2,'true',0),(3,'false',1)):
                            sql=f'INSERT INTO {pt} VALUES({ident},7,{active}) ON CONFLICT(k) WHERE active DO NOTHING'+(' RETURNING id' if returning else '')
                            if extended:
                                assert not parse(w,side,case,sql,[])['errors']
                                result=execute(w,side,case,sql,True)
                                assert not w.phase(b'C',b'Pp\0','3')['errors']
                                assert not w.phase(b'C',b'Ss\0','3')['errors']
                                assert w.sync()['ready']=='T'
                            else:
                                result=query(w,side,case,'execute',sql,'T')
                            assert result['rows']==encoded([(ident,)] if returning and count else []) and result['commands']==[f'INSERT 0 {count}'],result
                        for conn,expected in ((w,[(1,7,'t'),(3,7,'f')]),(o,[(1,7,'t')])):
                            result=query(conn,side,case,'visible',f'SELECT id,k,active FROM {pt} ORDER BY id')
                            assert result['rows']==encoded(expected),result
                        query(w,side,case,'end','COMMIT' if commit else 'ROLLBACK','I')
                    expected=[(1,7,'t'),(3,7,'f')] if side=='reference' and commit else [(1,7,'t')]
                    for conn in (w,o):
                        result=query(conn,side,case,'final',f'SELECT id,k,active FROM {pt} ORDER BY id')
                        assert result['rows']==encoded(expected),result
                    bad=query(w,side,case,'missing-predicate',f'INSERT INTO {pt} VALUES(9,7,true) ON CONFLICT(k) DO NOTHING')
                    assert [e['C'] for e in bad['errors']]==['42P10'] and bad['tail']['ready']=='I',bad
                    summary['cases']+=1
                finally:
                    w.sync()
                    query(w,side,case,'rollback','ROLLBACK','I')
                    query(w,side,case,'drop',f'DROP TABLE IF EXISTS {pt}','I')
            assert mysql_query(m,side,native_scan)['rows']==encoded([(1,11,'before')])
            assert mysql_query(n,side,native_scan)['rows']==encoded([(1,10,'before')])
            assert prepared_read(side)==prepared_base
        prepared_scope.__exit__(None,None,None)
        differences = {case: sides for case,sides in observations.items() if sides['reference'] != sides['target']}
        (evidence/'differences.json').write_text(json.dumps(differences,indent=2))
        summary['differences'] = len(differences)
        mysql_query(m, 'after', 'ROLLBACK')
        assert mysql_query(m, 'after', native_scan) == native_base
        assert mysql_query(n, 'after', native_scan)['rows'] == encoded([(1,10,'before')])
        # Native INSERT IGNORE is its own contract, never a PG substitute.
        for prepared,commit in itertools.product((False,True),repeat=2):
            case=f'conflict-mysql-{prepared}-{commit}'
            t='conflict_mysql_doc'
            sql=f"INSERT IGNORE INTO {t} VALUES (1,'changed',99),(3,'c',30)"
            try:
                mysql_query(m,case,f'CREATE TABLE {t}(id INT PRIMARY KEY,email VARCHAR(30) UNIQUE,v INT NOT NULL)')
                mysql_query(m,case,f"INSERT INTO {t} VALUES(1,'a',10),(2,'b',20)")
                scan_mysql=f'SELECT id,email,v FROM {t} ORDER BY id'
                mysql_query(m,case,'BEGIN')
                if prepared:
                    with mysql_prepared_int(m,case,f'INSERT IGNORE INTO {t} VALUES(?,?,?),(?,?,?)',(1,'changed',99,3,'c',30),True) as change:
                        result=change('execute')
                else:
                    result=m.query(sql)
                    result['affected_rows']=m.lib.mysql_affected_rows(m.conn)
                    save('mysql',case,'query',sql,result)
                assert result['code']==0 and result['warning_count']==1,result
                assert result['affected_rows']==1,result
                for label in ('before','during','after'):
                    with mysql_prepared_int(m,case,f'SELECT v FROM {t} WHERE id=?') as read:
                        assert read(label)['rows']==encoded([(10,)]),case
                    assert mysql_query(m,case,scan_mysql)['rows']==encoded(base+[(3,'c',30)])
                    assert mysql_query(n,case,scan_mysql)['rows']==encoded(base)
                    # Interleave a real PG connection while native work is pending.
                    if label=='during':
                        probe=Wire(pq,target)
                        try:
                            query(probe,'target',case,'create','CREATE TABLE conflict_interleave(id integer PRIMARY KEY,v integer)','I')
                            query(probe,'target',case,'seed','INSERT INTO conflict_interleave VALUES(1,10)','I')
                            query(probe,'target',case,'begin','BEGIN','T')
                            changed=query(probe,'target',case,'interleave','INSERT INTO conflict_interleave VALUES(1,99),(2,20) ON CONFLICT(id) DO NOTHING RETURNING id,v','T')
                            assert changed['rows']==encoded([(2,20)]) and changed['commands']==['INSERT 0 1'],changed
                            failed=query(probe,'target',case,'failure','INSERT INTO conflict_interleave VALUES(1,30)')
                            assert [e['C'] for e in failed['errors']]==['23505'] and failed['tail']['ready']=='E',failed
                            query(probe,'target',case,'rollback','ROLLBACK','I')
                            restored=query(probe,'target',case,'restored','SELECT id,v FROM conflict_interleave ORDER BY id','I')
                            assert restored['rows']==encoded([(1,10)]),restored
                            query(probe,'target',case,'drop','DROP TABLE conflict_interleave','I')
                        finally: probe.close()
                mysql_query(m,case,'COMMIT' if commit else 'ROLLBACK')
                for client in (m,n):
                    assert mysql_query(client,case,scan_mysql)['rows']==encoded(base+[(3,'c',30)] if commit else base)
                summary['mysql_cases']+=1
            finally:
                mysql_query(m,case,'ROLLBACK')
                mysql_query(m,case,f'DROP TABLE IF EXISTS {t}')
        # Independent prepared UPDATE, with an integer parameter and observer.
        with mysql_prepared_int(m, 'prepared-update', 'UPDATE returning_mysql_doc SET v=v+1 WHERE id = ?') as update_native:
            mysql_query(m, 'prepared-update', 'BEGIN')
            changed = update_native('execute')
            assert changed['affected_rows'] == 1, changed
            assert mysql_query(m, 'prepared-update', native_scan)['rows'] == encoded([(1,11,'before')])
            assert mysql_query(n, 'prepared-update', native_scan)['rows'] == encoded([(1,10,'before')])
            mysql_query(m, 'prepared-update', 'ROLLBACK')
            assert mysql_query(m, 'prepared-restored', native_scan) == native_base
            assert mysql_query(n, 'prepared-restored', native_scan)['rows'] == encoded([(1,10,'before')])
        assert summary['cases'] == 348, summary
        assert not differences and not summary['failures'], ('differential mismatches', differences, summary['failures'])
        summary['passed'] = True
    except BaseException as error:
        summary['failures'].append(str(error))
        raise
    finally:
        for index,wire in enumerate(connections):
            try:
                wire.sync()
                wire.query('ROLLBACK')
                result=wire.query(f'DROP TABLE IF EXISTS {table}')
                save('reference' if index<2 else 'target','cleanup','drop',f'DROP TABLE IF EXISTS {table}',result)
                assert not result['errors'] and result['ready']=='I',result
                summary['cleanup']['reference' if index<2 else 'target']=True
            finally:
                wire.close()
        summary['cleanup']['pg_connections'] = len(connections)
        if prepared_scope is not None:
            prepared_scope.__exit__(None,None,None)
        for client in mysqls:
            try:
                client.query('ROLLBACK')
                client.query('DROP TABLE IF EXISTS returning_mysql_doc')
            finally:
                client.close()
        summary['cleanup']['mysql_connections'] = len(mysqls)
        (evidence/'summary.json').write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary), flush=True)


if __name__ == '__main__':
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    run(Path(sys.argv[1]), int(sys.argv[2]), sys.argv[3], int(sys.argv[4]))
