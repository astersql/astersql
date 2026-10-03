select O.oid as op_id,
       O.xmin as state_number,
       oprname as op_name,
       oprkind as op_kind,
       oprleft as arg_left_type_id,
       oprright as arg_right_type_id,
       oprresult as arg_result_type_id,
       oprcode::oid as main_id,
       oprcode::varchar as main_name,
       oprrest::oid as restrict_id,
       oprrest::varchar as restrict_name,
       oprjoin::oid as join_id,
       oprjoin::varchar as join_name,
       oprcom::oid as com_id,
       oprcom::regoper::varchar as com_name,
       oprnegate::oid as neg_id,
       oprnegate::regoper::varchar as neg_name,
       oprcanmerge as merges,
       oprcanhash as hashes,
       pg_catalog.pg_get_userbyid(O.oprowner) as "owner"
from pg_catalog.pg_operator O
where oprnamespace = ?::oid
  --  and oprname in ( :[*f_names] )
  --  and pg_catalog.age(xmin) <= #TXAGE
