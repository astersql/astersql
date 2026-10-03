select case
  when pg_catalog.pg_is_in_recovery()
    then null
  else
    (pg_catalog.txid_current() % 4294967296)::varchar::bigint
  end as current_txid
