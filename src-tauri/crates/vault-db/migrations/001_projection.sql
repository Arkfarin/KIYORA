CREATE TABLE vaults (
    vault_id TEXT PRIMARY KEY, telegram_user_id INTEGER NOT NULL,
    chat_id INTEGER NOT NULL, generation INTEGER NOT NULL CHECK(generation >= 0),
    settings_json TEXT NOT NULL
);
CREATE TABLE sync_state (
    vault_id TEXT PRIMARY KEY REFERENCES vaults(vault_id),
    cursor INTEGER NOT NULL DEFAULT 0 CHECK(cursor >= 0),
    last_event_id INTEGER, last_verified_commit_id INTEGER,
    barrier_id INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE tracks (
    vault_id TEXT NOT NULL REFERENCES vaults(vault_id), track_id TEXT NOT NULL,
    title TEXT NOT NULL, performer TEXT NOT NULL, album TEXT NOT NULL,
    json TEXT NOT NULL, PRIMARY KEY(vault_id,track_id)
);
CREATE TABLE track_sources (
    vault_id TEXT NOT NULL, track_id TEXT NOT NULL, source_id TEXT NOT NULL,
    chat_id INTEGER NOT NULL, message_id INTEGER NOT NULL CHECK(message_id > 0),
    json TEXT NOT NULL, PRIMARY KEY(vault_id,track_id,source_id),
    FOREIGN KEY(vault_id,track_id) REFERENCES tracks(vault_id,track_id) ON DELETE CASCADE
);
CREATE INDEX sources_remote ON track_sources(chat_id,message_id);
CREATE TABLE playlists (
    vault_id TEXT NOT NULL REFERENCES vaults(vault_id), playlist_id TEXT NOT NULL,
    title TEXT NOT NULL, json TEXT NOT NULL, PRIMARY KEY(vault_id,playlist_id)
);
CREATE TABLE playlist_entries (
    vault_id TEXT NOT NULL, entry_id TEXT NOT NULL, playlist_id TEXT NOT NULL,
    track_id TEXT NOT NULL, after_entry_id TEXT, order_key INTEGER NOT NULL, json TEXT NOT NULL,
    PRIMARY KEY(vault_id,entry_id),
    FOREIGN KEY(vault_id,playlist_id) REFERENCES playlists(vault_id,playlist_id),
    FOREIGN KEY(vault_id,track_id) REFERENCES tracks(vault_id,track_id)
);
CREATE INDEX entries_playlist_order ON playlist_entries(vault_id,playlist_id,order_key,entry_id);
CREATE TABLE favorites (
    vault_id TEXT NOT NULL, set_name TEXT NOT NULL, track_id TEXT NOT NULL,
    PRIMARY KEY(vault_id,set_name,track_id),
    FOREIGN KEY(vault_id,track_id) REFERENCES tracks(vault_id,track_id)
);
CREATE TABLE tombstones (
    vault_id TEXT NOT NULL REFERENCES vaults(vault_id),
    kind TEXT NOT NULL CHECK(kind IN ('track','playlist','entry')), entity_id TEXT NOT NULL,
    PRIMARY KEY(vault_id,kind,entity_id)
);
CREATE TABLE applied_events (
    vault_id TEXT NOT NULL REFERENCES vaults(vault_id), op_id TEXT NOT NULL,
    message_id INTEGER NOT NULL CHECK(message_id > 0), action TEXT NOT NULL,
    event_sha256 TEXT NOT NULL, outcome TEXT NOT NULL,
    PRIMARY KEY(vault_id,op_id), UNIQUE(vault_id,message_id)
);
CREATE TABLE observed_messages (
    vault_id TEXT NOT NULL REFERENCES vaults(vault_id), message_id INTEGER NOT NULL,
    kind TEXT NOT NULL, record_sha256 TEXT NOT NULL,
    PRIMARY KEY(vault_id,message_id)
);
CREATE TABLE snapshot_registry (
    vault_id TEXT NOT NULL REFERENCES vaults(vault_id), snapshot_id TEXT NOT NULL,
    commit_id INTEGER NOT NULL, barrier_id INTEGER NOT NULL, generation INTEGER NOT NULL,
    status TEXT NOT NULL, json TEXT NOT NULL, PRIMARY KEY(vault_id,commit_id)
);
CREATE TABLE cache_entries (
    key TEXT PRIMARY KEY, path TEXT NOT NULL UNIQUE, bytes INTEGER NOT NULL CHECK(bytes >= 0),
    last_access INTEGER NOT NULL, pinned INTEGER NOT NULL CHECK(pinned IN (0,1)),
    download_state TEXT NOT NULL, sha256 TEXT
);
CREATE INDEX cache_lru ON cache_entries(pinned,last_access);
CREATE TABLE local_settings (key TEXT PRIMARY KEY,value TEXT NOT NULL);
CREATE TABLE inflight_commands (
    vault_id TEXT NOT NULL REFERENCES vaults(vault_id), op_id TEXT NOT NULL,
    intent TEXT NOT NULL, status TEXT NOT NULL, temp_msg_id INTEGER, server_msg_id INTEGER,
    result_json TEXT, PRIMARY KEY(vault_id,op_id)
);
CREATE TABLE gc_plans (
    plan_id TEXT PRIMARY KEY, vault_id TEXT NOT NULL REFERENCES vaults(vault_id),
    json TEXT NOT NULL, completed_json TEXT NOT NULL, status TEXT NOT NULL
);
CREATE VIRTUAL TABLE track_fts USING fts5(vault_id UNINDEXED,track_id UNINDEXED,title,performer,album,tokenize='unicode61');
CREATE VIRTUAL TABLE playlist_fts USING fts5(vault_id UNINDEXED,playlist_id UNINDEXED,title,tokenize='unicode61');
CREATE TRIGGER tracks_insert AFTER INSERT ON tracks BEGIN
    INSERT INTO track_fts VALUES(new.vault_id,new.track_id,new.title,new.performer,new.album);
END;
CREATE TRIGGER tracks_update AFTER UPDATE ON tracks BEGIN
    DELETE FROM track_fts WHERE vault_id=old.vault_id AND track_id=old.track_id;
    INSERT INTO track_fts VALUES(new.vault_id,new.track_id,new.title,new.performer,new.album);
END;
CREATE TRIGGER tracks_delete AFTER DELETE ON tracks BEGIN
    DELETE FROM track_fts WHERE vault_id=old.vault_id AND track_id=old.track_id;
END;
CREATE TRIGGER playlists_insert AFTER INSERT ON playlists BEGIN
    INSERT INTO playlist_fts VALUES(new.vault_id,new.playlist_id,new.title);
END;
CREATE TRIGGER playlists_update AFTER UPDATE ON playlists BEGIN
    DELETE FROM playlist_fts WHERE vault_id=old.vault_id AND playlist_id=old.playlist_id;
    INSERT INTO playlist_fts VALUES(new.vault_id,new.playlist_id,new.title);
END;
CREATE TRIGGER playlists_delete AFTER DELETE ON playlists BEGIN
    DELETE FROM playlist_fts WHERE vault_id=old.vault_id AND playlist_id=old.playlist_id;
END;
PRAGMA user_version=1;
