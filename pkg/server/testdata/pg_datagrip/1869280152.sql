select P.oid as id,
       P.amprocnum as num,
       P.amproc::oid as proc_id,
       P.amproc::regprocedure::varchar as proc_sig,
       P.amproclefttype::regtype::varchar as left_type,
       P.amprocrighttype::regtype::varchar as right_type,
       P.amprocfamily as family_id,
       C.oid as class_id
from pg_catalog.pg_amproc P
    left join pg_opfamily F on P.amprocfamily = F.oid
    left join pg_depend D on D.classid = 'pg_amproc'::regclass and P.oid = D.objid and D.objsubid = 0
    left join pg_opclass C on D.refclassid = 'pg_opclass'::regclass and C.oid = D.refobjid and D.refobjsubid = 0
where C.opcnamespace = ?::oid or C.opcnamespace is null and F.opfnamespace = ?::oid
  --  and pg_catalog.age(P.xmin) <= #TXAGE
order by C.oid, F.oid
