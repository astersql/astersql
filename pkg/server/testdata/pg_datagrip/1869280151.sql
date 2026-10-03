select O.oid as id,
       O.amopstrategy as strategy,
       O.amopopr as op_id,
       O.amopopr::regoperator::varchar as op_sig,
       O.amopsortfamily /* null */ as sort_family_id,
       SF.opfname /* null */ as sort_family,
       O.amopfamily as family_id,
       C.oid as class_id
from pg_catalog.pg_amop O
    left join pg_opfamily F on O.amopfamily = F.oid
    left join pg_opfamily SF on O.amopsortfamily = SF.oid
    left join pg_depend D on D.classid = 'pg_amop'::regclass and O.oid = D.objid and D.objsubid = 0
    left join pg_opclass C on D.refclassid = 'pg_opclass'::regclass and C.oid = D.refobjid and D.refobjsubid = 0
where C.opcnamespace = ?::oid or C.opcnamespace is null and F.opfnamespace = ?::oid
  --  and pg_catalog.age(O.xmin) <= #TXAGE
order by C.oid, F.oid
