-- Drop the two tables: LSP persistence and "visited git repository history".
-- workspace_language_server references workspace_metadata via the workspace_id FK,
-- so the child table must be dropped first.
DROP TABLE IF EXISTS workspace_language_server;
DROP TABLE IF EXISTS workspace_metadata;
