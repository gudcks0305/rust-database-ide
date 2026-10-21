WITH target AS (
 SELECT c.*, n.nspname, n.nspowner, n.nspacl
 FROM pg_catalog.pg_class c
 JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
 WHERE c.oid=__RELATION_OID__::pg_catalog.oid
), metadata AS (
 SELECT pg_catalog.jsonb_build_object(
  'format',1,
  'relation',pg_catalog.jsonb_build_object(
   'oid',t.oid::pg_catalog.int8,'schema',t.nspname,'name',t.relname,'kind',t.relkind::pg_catalog.text,
   'persistence',t.relpersistence::pg_catalog.text,'owner',t.relowner::pg_catalog.int8,'access_method',t.relam::pg_catalog.int8,
   'tablespace',t.reltablespace::pg_catalog.int8,'options',t.reloptions,'privileges',t.relacl,
   'replica_identity',t.relreplident::pg_catalog.text,'row_security',t.relrowsecurity,
   'force_row_security',t.relforcerowsecurity,'partition',t.relispartition,
   'partition_bound',pg_catalog.to_jsonb(t)->'relpartbound',
   'namespace_owner',t.nspowner::pg_catalog.int8,'namespace_privileges',t.nspacl,
   'comment',pg_catalog.obj_description(t.oid,'pg_class')),
  'columns',COALESCE((SELECT pg_catalog.jsonb_agg(pg_catalog.jsonb_build_object(
    'attribute',pg_catalog.to_jsonb(a),'type',pg_catalog.to_jsonb(ty),
    'type_schema',tn.nspname,'default',pg_catalog.to_jsonb(d),
    'collation',pg_catalog.to_jsonb(co),'comment',pg_catalog.col_description(t.oid,a.attnum)
   ) ORDER BY a.attnum)
   FROM pg_catalog.pg_attribute a
   LEFT JOIN pg_catalog.pg_type ty ON ty.oid=a.atttypid
   LEFT JOIN pg_catalog.pg_namespace tn ON tn.oid=ty.typnamespace
   LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum
   LEFT JOIN pg_catalog.pg_collation co ON co.oid=a.attcollation
   WHERE a.attrelid=t.oid AND a.attnum>0),'[]'::pg_catalog.jsonb),
  'constraints',COALESCE((SELECT pg_catalog.jsonb_agg(pg_catalog.jsonb_build_object(
    'constraint',pg_catalog.to_jsonb(k),'source_schema',sn.nspname,
    'source_table',sc.relname,'target_schema',rn.nspname,'target_table',rc.relname,
    'comment',pg_catalog.obj_description(k.oid,'pg_constraint')) ORDER BY k.oid)
   FROM pg_catalog.pg_constraint k
   LEFT JOIN pg_catalog.pg_class sc ON sc.oid=k.conrelid
   LEFT JOIN pg_catalog.pg_namespace sn ON sn.oid=sc.relnamespace
   LEFT JOIN pg_catalog.pg_class rc ON rc.oid=k.confrelid
   LEFT JOIN pg_catalog.pg_namespace rn ON rn.oid=rc.relnamespace
   WHERE k.conrelid=t.oid OR k.confrelid=t.oid),'[]'::pg_catalog.jsonb),
  'indexes',COALESCE((SELECT pg_catalog.jsonb_agg(pg_catalog.jsonb_build_object(
    'index',pg_catalog.to_jsonb(i),'name',ic.relname,'namespace',ins.nspname,
    'owner',ic.relowner::pg_catalog.int8,'kind',ic.relkind::pg_catalog.text,'method',pg_catalog.to_jsonb(am),
    'options',ic.reloptions,'tablespace',ic.reltablespace::pg_catalog.int8,'privileges',ic.relacl,
    'comment',pg_catalog.obj_description(ic.oid,'pg_class'),
    'operator_classes',COALESCE((SELECT pg_catalog.jsonb_agg(pg_catalog.to_jsonb(op) ORDER BY slot.ordinality)
      FROM pg_catalog.unnest(i.indclass) WITH ORDINALITY slot(opid,ordinality)
      JOIN pg_catalog.pg_opclass op ON op.oid=slot.opid),'[]'::pg_catalog.jsonb)) ORDER BY i.indexrelid)
   FROM pg_catalog.pg_index i
   JOIN pg_catalog.pg_class ic ON ic.oid=i.indexrelid
   JOIN pg_catalog.pg_namespace ins ON ins.oid=ic.relnamespace
   LEFT JOIN pg_catalog.pg_am am ON am.oid=ic.relam
   WHERE i.indrelid=t.oid),'[]'::pg_catalog.jsonb),
  'triggers',COALESCE((SELECT pg_catalog.jsonb_agg(pg_catalog.to_jsonb(g) ORDER BY g.oid)
   FROM pg_catalog.pg_trigger g WHERE g.tgrelid=t.oid),'[]'::pg_catalog.jsonb),
  'rules',COALESCE((SELECT pg_catalog.jsonb_agg(pg_catalog.to_jsonb(r) ORDER BY r.oid)
   FROM pg_catalog.pg_rewrite r WHERE r.ev_class=t.oid),'[]'::pg_catalog.jsonb),
  'policies',COALESCE((SELECT pg_catalog.jsonb_agg(pg_catalog.to_jsonb(p) ORDER BY p.oid)
   FROM pg_catalog.pg_policy p WHERE p.polrelid=t.oid),'[]'::pg_catalog.jsonb),
  'inheritance',COALESCE((SELECT pg_catalog.jsonb_agg(pg_catalog.to_jsonb(h) ORDER BY h.inhrelid,h.inhparent,h.inhseqno)
   FROM pg_catalog.pg_inherits h WHERE h.inhrelid=t.oid OR h.inhparent=t.oid),'[]'::pg_catalog.jsonb),
  'partition_keys',COALESCE((SELECT pg_catalog.jsonb_agg(pg_catalog.to_jsonb(p) ORDER BY p.partrelid)
   FROM pg_catalog.pg_partitioned_table p WHERE p.partrelid=t.oid),'[]'::pg_catalog.jsonb),
  'extended_statistics',COALESCE((SELECT pg_catalog.jsonb_agg(pg_catalog.to_jsonb(s) ORDER BY s.oid)
   FROM pg_catalog.pg_statistic_ext s WHERE s.stxrelid=t.oid),'[]'::pg_catalog.jsonb),
  'publications',COALESCE((SELECT pg_catalog.jsonb_agg(pg_catalog.jsonb_build_object(
    'publication',pg_catalog.to_jsonb(p),'relation',pg_catalog.to_jsonb(pr)) ORDER BY p.oid,pr.oid)
   FROM pg_catalog.pg_publication p LEFT JOIN pg_catalog.pg_publication_rel pr
     ON pr.prpubid=p.oid AND pr.prrelid=t.oid
   WHERE p.puballtables OR pr.prrelid=t.oid),'[]'::pg_catalog.jsonb),
  'dependencies',COALESCE((SELECT pg_catalog.jsonb_agg(pg_catalog.to_jsonb(d)
    ORDER BY d.classid,d.objid,d.objsubid,d.refclassid,d.refobjid,d.refobjsubid,d.deptype)
   FROM pg_catalog.pg_depend d
   WHERE (d.classid='pg_catalog.pg_class'::pg_catalog.regclass AND d.objid=t.oid)
      OR (d.refclassid='pg_catalog.pg_class'::pg_catalog.regclass AND d.refobjid=t.oid)),'[]'::pg_catalog.jsonb)
 ) AS value FROM target t
), encoded AS (
 SELECT (SELECT value::pg_catalog.text FROM metadata) AS canonical_json
)
SELECT CASE WHEN pg_catalog.octet_length(canonical_json)<=1048576
 THEN canonical_json ELSE NULL END AS canonical_json FROM encoded;
