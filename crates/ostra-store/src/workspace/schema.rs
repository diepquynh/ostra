//! The workspace database migrations, applied in order and tracked by `user_version`.

pub(crate) const MIGRATIONS: &[&str] = &[
    r#"
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE projects (
  key TEXT PRIMARY KEY,
  path TEXT NOT NULL,
  init_status TEXT NOT NULL,
  stack TEXT
);
CREATE TABLE sessions (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  request TEXT NOT NULL,
  category TEXT,
  status TEXT NOT NULL,
  lane TEXT NOT NULL,
  stage_label TEXT NOT NULL,
  projects TEXT NOT NULL,
  yolo INTEGER NOT NULL,
  cost_usd REAL NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE events (
  session_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  type TEXT NOT NULL,
  payload TEXT NOT NULL,
  at TEXT NOT NULL,
  PRIMARY KEY (session_id, seq)
);
CREATE TABLE executions (
  id TEXT PRIMARY KEY,
  session_id TEXT,
  agent TEXT NOT NULL,
  purpose TEXT,
  stage TEXT,
  project TEXT NOT NULL,
  executor TEXT NOT NULL,
  model TEXT NOT NULL,
  params TEXT NOT NULL,
  spawn_block TEXT NOT NULL,
  report_path TEXT,
  status TEXT NOT NULL,
  native_session_id TEXT,
  usage TEXT NOT NULL,
  cost_usd REAL NOT NULL DEFAULT 0,
  error TEXT,
  submit TEXT,
  final_text TEXT,
  started_at TEXT NOT NULL,
  ended_at TEXT
);
CREATE INDEX executions_session ON executions(session_id);
CREATE INDEX executions_status ON executions(status);
CREATE TABLE messages (
  execution_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  role TEXT NOT NULL,
  content TEXT NOT NULL,
  PRIMARY KEY (execution_id, seq)
);
CREATE TABLE activity (
  execution_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  delta TEXT NOT NULL,
  at TEXT NOT NULL,
  PRIMARY KEY (execution_id, seq)
);
CREATE TABLE gates (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  title TEXT NOT NULL,
  explanation TEXT NOT NULL,
  payload TEXT NOT NULL,
  answer TEXT,
  source TEXT,
  reason TEXT,
  opened_at TEXT NOT NULL,
  answered_at TEXT
);
CREATE INDEX gates_session ON gates(session_id);
CREATE TABLE decisions (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  judge TEXT NOT NULL,
  subject TEXT,
  input_summary TEXT NOT NULL,
  output TEXT NOT NULL,
  reason TEXT NOT NULL,
  overridden INTEGER NOT NULL DEFAULT 0,
  can_override INTEGER NOT NULL DEFAULT 1,
  at TEXT NOT NULL
);
CREATE INDEX decisions_session ON decisions(session_id);
CREATE TABLE tool_calls (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  execution_id TEXT NOT NULL,
  call_id TEXT NOT NULL,
  tool TEXT NOT NULL,
  input TEXT NOT NULL,
  decision TEXT,
  rule TEXT,
  duration_ms INTEGER,
  output TEXT,
  at TEXT NOT NULL
);
CREATE INDEX tool_calls_execution ON tool_calls(execution_id);
"#,
    r#"
ALTER TABLE sessions ADD COLUMN title TEXT;
ALTER TABLE executions ADD COLUMN summary TEXT;
"#,
    r#"
CREATE TABLE search_docs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  kind TEXT NOT NULL,
  ref TEXT NOT NULL,
  session_id TEXT NOT NULL,
  label TEXT NOT NULL,
  body TEXT NOT NULL,
  mtime INTEGER NOT NULL DEFAULT 0,
  UNIQUE(kind, ref)
);
CREATE VIRTUAL TABLE search_fts USING fts5(
  label, body, content='search_docs', content_rowid='id', tokenize='unicode61 remove_diacritics 2', prefix='2 3'
);
CREATE TRIGGER search_docs_ai AFTER INSERT ON search_docs BEGIN
  INSERT INTO search_fts(rowid, label, body) VALUES (new.id, new.label, new.body);
END;
CREATE TRIGGER search_docs_ad AFTER DELETE ON search_docs BEGIN
  INSERT INTO search_fts(search_fts, rowid, label, body) VALUES ('delete', old.id, old.label, old.body);
END;
CREATE TRIGGER search_docs_au AFTER UPDATE ON search_docs BEGIN
  INSERT INTO search_fts(search_fts, rowid, label, body) VALUES ('delete', old.id, old.label, old.body);
  INSERT INTO search_fts(rowid, label, body) VALUES (new.id, new.label, new.body);
END;
CREATE TRIGGER sessions_search_ai AFTER INSERT ON sessions BEGIN
  INSERT INTO search_docs (kind, ref, session_id, label, body) VALUES ('session', new.id, new.id, COALESCE(new.title, ''), new.request);
END;
CREATE TRIGGER sessions_search_au AFTER UPDATE OF title, request ON sessions
  WHEN old.title IS NOT new.title OR old.request IS NOT new.request BEGIN
  UPDATE search_docs SET label = COALESCE(new.title, ''), body = new.request WHERE kind = 'session' AND ref = new.id;
END;
INSERT INTO search_docs (kind, ref, session_id, label, body)
  SELECT 'session', id, id, COALESCE(title, ''), request FROM sessions;
"#,
];
