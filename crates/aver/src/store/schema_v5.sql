-- Aver 0.4 removed the signed HTTP API, the only writer of the request
-- audit log (ARCHITECTURE.md §1.11, §6.3).

DROP INDEX requests_by_ts;
DROP TABLE requests;
