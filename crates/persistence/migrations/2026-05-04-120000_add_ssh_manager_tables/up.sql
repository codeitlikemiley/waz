CREATE TABLE ssh_nodes (
  id          TEXT PRIMARY KEY NOT NULL,
  parent_id   TEXT REFERENCES ssh_nodes(id) ON DELETE CASCADE,
  kind        TEXT NOT NULL CHECK(kind IN ('folder','server')),
  name        TEXT NOT NULL,
  sort_order  INTEGER NOT NULL DEFAULT 0,
  created_at  TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at  TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX idx_ssh_nodes_parent ON ssh_nodes(parent_id, sort_order);

CREATE TABLE ssh_servers (
  node_id           TEXT PRIMARY KEY NOT NULL REFERENCES ssh_nodes(id) ON DELETE CASCADE,
  host              TEXT NOT NULL,
  port              INTEGER NOT NULL DEFAULT 22,
  username          TEXT NOT NULL DEFAULT '',
  auth_type         TEXT NOT NULL CHECK(auth_type IN ('password','key')) DEFAULT 'password',
  key_path          TEXT,
  last_connected_at TIMESTAMP
);
