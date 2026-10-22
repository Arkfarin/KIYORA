use crate::*;
use std::collections::BTreeMap;

fn load<T: serde::de::DeserializeOwned>(
    conn: &Connection,
    table: &str,
    vault: &str,
) -> Result<Vec<T>, DbError> {
    let mut stmt = conn.prepare(&format!("SELECT json FROM {table} WHERE vault_id=?1"))?;
    let mut rows = stmt.query([vault])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(serde_json::from_str(&row.get::<_, String>(0)?)?);
    }
    Ok(out)
}
pub(crate) fn read(conn: &Connection, vault_id: Uuid) -> Result<State, DbError> {
    let vault = vault_id.to_string();
    let mut state = State::new(vault_id);
    let (generation, settings): (i64, String) = conn.query_row(
        "SELECT generation,settings_json FROM vaults WHERE vault_id=?1",
        [&vault],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    state.generation = u64::try_from(generation).map_err(|_| DbError::Schema)?;
    state.settings = serde_json::from_str(&settings)?;
    let last: Option<i64> = conn.query_row(
        "SELECT last_event_id FROM sync_state WHERE vault_id=?1",
        [&vault],
        |r| r.get(0),
    )?;
    state.last_event_message_id = last
        .map(MessageId::new)
        .transpose()
        .map_err(|_| DbError::Schema)?;
    state.tracks = load::<Track>(conn, "tracks", &vault)?
        .into_iter()
        .map(|v| (v.track_id, v))
        .collect();
    state.playlists = load::<Playlist>(conn, "playlists", &vault)?
        .into_iter()
        .map(|v| (v.playlist_id, v))
        .collect();
    state.entries = load::<PlaylistEntry>(conn, "playlist_entries", &vault)?
        .into_iter()
        .map(|v| (v.entry_id, v))
        .collect();
    let mut stmt = conn.prepare("SELECT set_name,track_id FROM favorites WHERE vault_id=?1")?;
    let mut rows = stmt.query([&vault])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(0)?;
        let id: String = row.get(1)?;
        state
            .favorites
            .entry(name)
            .or_default()
            .insert(id.parse().map_err(|_| DbError::Schema)?);
    }
    let mut stmt = conn.prepare("SELECT kind,entity_id FROM tombstones WHERE vault_id=?1")?;
    let mut rows = stmt.query([&vault])?;
    while let Some(row) = rows.next()? {
        let kind: String = row.get(0)?;
        let id: String = row.get(1)?;
        let id = id.parse().map_err(|_| DbError::Schema)?;
        match kind.as_str() {
            "track" => {
                state.tombstones.tracks.insert(id);
            }
            "playlist" => {
                state.tombstones.playlists.insert(id);
            }
            "entry" => {
                state.tombstones.entries.insert(id);
            }
            _ => return Err(DbError::Schema),
        }
    }
    let mut stmt = conn.prepare(
        "SELECT op_id,message_id,action,event_sha256,outcome FROM applied_events WHERE vault_id=?1",
    )?;
    let mut rows = stmt.query([&vault])?;
    while let Some(row) = rows.next()? {
        let op: String = row.get(0)?;
        let mid: i64 = row.get(1)?;
        let outcome: String = row.get(4)?;
        state.applied.insert(
            op.parse().map_err(|_| DbError::Schema)?,
            Receipt {
                message_id: MessageId::new(mid).map_err(|_| DbError::Schema)?,
                action: row.get(2)?,
                event_sha256: row.get(3)?,
                outcome: serde_json::from_str(&outcome)?,
            },
        );
    }
    state.validate()?;
    Ok(state)
}

fn delete_removed<T>(
    conn: &Connection,
    table: &str,
    key: &str,
    vault: &str,
    before: &BTreeMap<Uuid, T>,
    after: &BTreeMap<Uuid, T>,
) -> Result<(), DbError> {
    for id in before.keys().filter(|id| !after.contains_key(id)) {
        conn.execute(
            &format!("DELETE FROM {table} WHERE vault_id=?1 AND {key}=?2"),
            params![vault, id.to_string()],
        )?;
    }
    Ok(())
}
pub(crate) fn write_delta(conn: &Connection, before: &State, after: &State) -> Result<(), DbError> {
    let vault = after.vault_id.to_string();
    delete_removed(
        conn,
        "playlist_entries",
        "entry_id",
        &vault,
        &before.entries,
        &after.entries,
    )?;
    if before.favorites != after.favorites {
        conn.execute("DELETE FROM favorites WHERE vault_id=?1", [&vault])?;
    }
    delete_removed(
        conn,
        "tracks",
        "track_id",
        &vault,
        &before.tracks,
        &after.tracks,
    )?;
    delete_removed(
        conn,
        "playlists",
        "playlist_id",
        &vault,
        &before.playlists,
        &after.playlists,
    )?;
    for (id, t) in after
        .tracks
        .iter()
        .filter(|(id, t)| before.tracks.get(id) != Some(*t))
    {
        conn.execute("INSERT INTO tracks VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(vault_id,track_id) DO UPDATE SET title=excluded.title,performer=excluded.performer,album=excluded.album,json=excluded.json",params![vault,id.to_string(),t.metadata.title,t.metadata.performer,t.metadata.album,serde_json::to_string(t)?])?;
        conn.execute(
            "DELETE FROM track_sources WHERE vault_id=?1 AND track_id=?2",
            params![vault, id.to_string()],
        )?;
        for (source_id, s) in &t.sources {
            conn.execute(
                "INSERT INTO track_sources VALUES (?1,?2,?3,?4,?5,?6)",
                params![
                    vault,
                    id.to_string(),
                    source_id.to_string(),
                    s.chat_id.get(),
                    s.message_id.get(),
                    serde_json::to_string(s)?
                ],
            )?;
        }
    }
    for (id, p) in after
        .playlists
        .iter()
        .filter(|(id, p)| before.playlists.get(id) != Some(*p))
    {
        conn.execute("INSERT INTO playlists VALUES (?1,?2,?3,?4) ON CONFLICT(vault_id,playlist_id) DO UPDATE SET title=excluded.title,json=excluded.json",params![vault,id.to_string(),p.title,serde_json::to_string(p)?])?;
    }
    for (id, e) in after
        .entries
        .iter()
        .filter(|(id, e)| before.entries.get(id) != Some(*e))
    {
        conn.execute("INSERT INTO playlist_entries VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(vault_id,entry_id) DO UPDATE SET playlist_id=excluded.playlist_id,track_id=excluded.track_id,after_entry_id=excluded.after_entry_id,order_key=excluded.order_key,json=excluded.json",params![vault,id.to_string(),e.playlist_id.to_string(),e.track_id.to_string(),e.after_entry_id.map(|x|x.to_string()),e.order_key.get(),serde_json::to_string(e)?])?;
    }
    if before.favorites != after.favorites {
        for (name, set) in &after.favorites {
            for id in set {
                conn.execute(
                    "INSERT INTO favorites VALUES (?1,?2,?3)",
                    params![vault, name, id.to_string()],
                )?;
            }
        }
    }
    for (kind, old, new) in [
        ("track", &before.tombstones.tracks, &after.tombstones.tracks),
        (
            "playlist",
            &before.tombstones.playlists,
            &after.tombstones.playlists,
        ),
        (
            "entry",
            &before.tombstones.entries,
            &after.tombstones.entries,
        ),
    ] {
        for id in old.difference(new) {
            conn.execute(
                "DELETE FROM tombstones WHERE vault_id=?1 AND kind=?2 AND entity_id=?3",
                params![vault, kind, id.to_string()],
            )?;
        }
        for id in new.difference(old) {
            conn.execute(
                "INSERT INTO tombstones VALUES (?1,?2,?3)",
                params![vault, kind, id.to_string()],
            )?;
        }
    }
    for (op, r) in after
        .applied
        .iter()
        .filter(|(op, r)| before.applied.get(op) != Some(*r))
    {
        conn.execute(
            "INSERT INTO applied_events VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                vault,
                op.to_string(),
                r.message_id.get(),
                r.action,
                r.event_sha256,
                serde_json::to_string(&r.outcome)?
            ],
        )?;
    }
    conn.execute(
        "UPDATE vaults SET generation=?1,settings_json=?2 WHERE vault_id=?3",
        params![
            i64::try_from(after.generation).map_err(|_| DbError::Schema)?,
            serde_json::to_string(&after.settings)?,
            vault
        ],
    )?;
    conn.execute(
        "UPDATE sync_state SET last_event_id=?1 WHERE vault_id=?2",
        params![after.last_event_message_id.map(MessageId::get), vault],
    )?;
    Ok(())
}
