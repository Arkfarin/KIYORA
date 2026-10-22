//! Run with `cargo run -p vault-db --example replay-benchmark -- 1000`.
//! Add `--batch-only` for larger histories; results are wall times, not CI thresholds.
use std::{error::Error, time::Instant};
use uuid::Uuid;
use vault_db::{Binding, Database};
use vault_domain::{Action, ChatId, Event, MessageId, State, UserId};
use vault_protocol::Record;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let count: usize = args.first().map_or(Ok(1_000), |v| v.parse())?;
    let batch_only = args.get(1).is_some_and(|arg| arg == "--batch-only");
    if !(1..=50_000).contains(&count) || args.len() > 2 || (args.len() == 2 && !batch_only) {
        return Err("usage: replay-benchmark [1..50000] [--batch-only]".into());
    }
    let binding = Binding {
        vault_id: Uuid::from_u128(1),
        owner_id: UserId::new(42)?,
        chat_id: ChatId::new(-100)?,
    };
    let mut reference = State::new(binding.vault_id);
    let mut records = Vec::with_capacity(count);
    for index in 0..count {
        let id = MessageId::new(9_007_199_254_740_993 + i64::try_from(index)? * 17)?;
        let event = Event {
            vault_id: binding.vault_id,
            op_id: Uuid::from_u128(100_000 + index as u128),
            device_id: "replay-benchmark".into(),
            base_generation: 0,
            entity_id: Uuid::from_u128(200_000 + index as u128),
            action: Action::PlaylistCreate {
                title: format!("Playlist {index}"),
            },
        };
        reference.apply(id, &event)?;
        records.push((id, Record::Event(Box::new(event))));
    }
    let directory = tempfile::tempdir()?;
    for width in [1, 100] {
        if batch_only && width == 1 {
            continue;
        }
        let path = directory.path().join(format!("batch-{width}.sqlite"));
        let mut db = Database::open(&path, binding)?;
        let started = Instant::now();
        for batch in records.chunks(width) {
            db.apply_records(batch)?;
        }
        let elapsed = started.elapsed();
        if db.state()? != reference {
            return Err("replay differs from the pure reducer".into());
        }
        println!(
            "{}",
            serde_json::json!({
                "records": count,
                "batch_size": width,
                "elapsed_ms": elapsed.as_millis(),
                "records_per_second": count as f64 / elapsed.as_secs_f64(),
                "cursor": db.cursor()?.to_string(),
                "matches_reducer": true,
                "database": "temporary SQLite file, WAL, synchronous=FULL",
                "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            })
        );
    }
    Ok(())
}
