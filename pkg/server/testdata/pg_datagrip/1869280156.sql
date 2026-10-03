select D.objoid id, C.relkind::char as kind, D.objsubid sub_id, D.description
from pg_catalog.pg_description D
  join pg_catalog.pg_class C on D.objoid = C.oid
where C.relnamespace = ?::oid and C.relkind != 'c' and D.classoid = 'pg_catalog.pg_class'::regclass
--  and pg_catalog.age(D.xmin) <= #TXAGE
-- all table-like things + seqs + iets anders?
union all
select T.oid id, 'T'::char as kind, D.objsubid sub_id, D.description
from pg_catalog.pg_description D
  join pg_catalog.pg_type T on T.oid = D.objoid or T.typrelid = D.objoid
  left join pg_catalog.pg_class C on T.typrelid = C.oid
where T.typnamespace = ?::oid and (C.relkind = 'c' or C.relkind is null)
--  and pg_catalog.age(D.xmin) <= #TXAGE
-- relkind = c (composite types?)
union all
select D.objoid id, pg_catalog.translate(C.contype, 'pufc', 'kkxz')::char as kind, D.objsubid sub_id, D.description
from pg_catalog.pg_description D
  join pg_catalog.pg_constraint C on D.objoid = C.oid
where C.connamespace = ?::oid and D.classoid = 'pg_catalog.pg_constraint'::regclass
--  and pg_catalog.age(D.xmin) <= #TXAGE
-- table constraints
union all
select D.objoid id, 't'::char as kind, D.objsubid sub_id, D.description
from pg_catalog.pg_description D
  join pg_catalog.pg_trigger T on T.oid = D.objoid
  join pg_catalog.pg_class C on C.oid = T.tgrelid
where C.relnamespace = ?::oid and D.classoid = 'pg_catalog.pg_trigger'::regclass
--  and pg_catalog.age(D.xmin) <= #TXAGE
-- triggers
union all
select D.objoid id, 'R'::char as kind, D.objsubid sub_id, D.description
from pg_catalog.pg_description D
  join pg_catalog.pg_rewrite R on R.oid = D.objoid
  join pg_catalog.pg_class C on C.oid = R.ev_class
where C.relnamespace = ?::oid and D.classoid = 'pg_catalog.pg_rewrite'::regclass
--  and pg_catalog.age(D.xmin) <= #TXAGE
-- rules
union all
select D.objoid id, 'F'::char as kind, D.objsubid sub_id, D.description
from pg_catalog.pg_description D
  join pg_catalog.pg_proc P on P.oid = D.objoid
where P.pronamespace = ?::oid and D.classoid = 'pg_catalog.pg_proc'::regclass
--  and pg_catalog.age(D.xmin) <= #TXAGE
-- more routines
union all
select D.objoid id, 'O'::char as kind, D.objsubid sub_id, D.description
from pg_catalog.pg_description D
  join pg_catalog.pg_operator O on O.oid = D.objoid
where O.oprnamespace = ?::oid and D.classoid = 'pg_catalog.pg_operator'::regclass
--  and pg_catalog.age(D.xmin) <= #TXAGE
-- operators
union all
select D.objoid id, 'f'::char as kind, D.objsubid sub_id, D.description
from pg_catalog.pg_description D
  join pg_catalog.pg_opfamily O on O.oid = D.objoid
where O.opfnamespace = ?::oid and D.classoid = 'pg_catalog.pg_opfamily'::regclass
--  and pg_catalog.age(D.xmin) <= #TXAGE
-- op families
union all
select D.objoid id, 'c'::char as kind, D.objsubid sub_id, D.description
from pg_catalog.pg_description D
  join pg_catalog.pg_opclass O on O.oid = D.objoid
where O.opcnamespace = ?::oid and D.classoid = 'pg_catalog.pg_opclass'::regclass
--  and pg_catalog.age(D.xmin) <= #TXAGE
-- op class
  union all
select D.objoid id, 'C'::char as kind, D.objsubid sub_id, D.description
from pg_catalog.pg_description D
  join pg_catalog.pg_collation C on C.oid = D.objoid
where C.collnamespace = ?::oid and D.classoid = 'pg_catalog.pg_collation'::regclass
--  and pg_catalog.age(D.xmin) <= #TXAGE

-- collations
  union all
select D.objoid id, 'P'::char as kind, D.objsubid sub_id, D.description
from pg_catalog.pg_description D
       join pg_catalog.pg_policy P on P.oid = D.objoid
       join pg_catalog.pg_class C on P.polrelid = C.oid
where C.relnamespace = ?::oid and D.classoid = 'pg_catalog.pg_policy'::regclass
--  and pg_catalog.age(D.xmin) <= #TXAGE

-- security policies (also by table name...)
