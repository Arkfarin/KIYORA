use crate::DbError;
use rusqlite::{Connection, TransactionBehavior};

// Append migrations; never change a released migration or infer the version
// from the presence of individual tables. user_version is the schema version.
const MIGRATIONS: &[&str] = &[include_str!("../migrations/001_projection.sql")];
pub const SCHEMA_VERSION: u32 = MIGRATIONS.len() as u32;

pub(crate) fn version(connection: &Connection) -> Result<u32, DbError> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let version = u32::try_from(version).map_err(|_| DbError::Schema)?;
    if version > SCHEMA_VERSION {
        return Err(DbError::Schema);
    }
    Ok(version)
}

pub(crate) fn migrate(connection: &mut Connection) -> Result<(), DbError> {
    apply(connection, MIGRATIONS)
}

fn apply(connection: &mut Connection, migrations: &[&str]) -> Result<(), DbError> {
    // Check again while holding the writer lock. The whole upgrade is atomic,
    // including all version increments, if a later migration fails.
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: i64 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let start = usize::try_from(version).map_err(|_| DbError::Schema)?;
    if start > migrations.len() {
        return Err(DbError::Schema);
    }
    for (index, sql) in migrations.iter().enumerate().skip(start) {
        tx.execute_batch(sql)?;
        let next = i64::try_from(index + 1).map_err(|_| DbError::Schema)?;
        tx.pragma_update(None, "user_version", next)?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn sequential_upgrade_preserves_old_data_and_rolls_back_all_steps_on_failure() {
        let mut db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE old_data(value TEXT); INSERT INTO old_data VALUES ('keep'); PRAGMA user_version=1;").unwrap();
        let steps = [
            "CREATE TABLE old_data(value TEXT);",
            "ALTER TABLE old_data ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1;",
            "INSERT INTO missing_table VALUES (1);",
        ];
        assert!(apply(&mut db, &steps).is_err());
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(db.prepare("SELECT enabled FROM old_data").is_err());
        assert_eq!(
            db.query_row("SELECT value FROM old_data", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "keep"
        );
        apply(&mut db, &steps[..2]).unwrap();
        assert_eq!(
            db.query_row("SELECT value,enabled FROM old_data", [], |r| Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?
            )))
            .unwrap(),
            ("keep".into(), 1)
        );
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        apply(&mut db, &steps[..2]).unwrap();
    }
}
