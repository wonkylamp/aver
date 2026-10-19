-- Aver 0.4 deleted the Docker sandbox's dependency layers and build cache
-- volumes (`deps_layers`) and the host dependency stores (`host_deps`), the
-- only writers and readers of these tables (ARCHITECTURE.md §1.11).

DROP TABLE deps_layers;
DROP TABLE host_deps;
