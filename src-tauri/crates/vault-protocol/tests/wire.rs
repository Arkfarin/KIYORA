#![allow(clippy::unwrap_used)]
use uuid::Uuid;
use vault_domain::*;
use vault_protocol::*;

fn event() -> Record {
    Record::Event(Box::new(Event {
        vault_id: Uuid::from_u128(1),
        op_id: Uuid::from_u128(2),
        device_id: "kiyora-test".into(),
        base_generation: 0,
        entity_id: Uuid::from_u128(3),
        action: Action::PlaylistCreate {
            title: "夜の音楽".into(),
        },
    }))
}

#[test]
fn event_matches_rfc_flat_action_payload_and_roundtrips() {
    let r = event();
    let encoded = encode(&r).unwrap();
    assert_eq!(decode(&encoded).unwrap(), r);
    let value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(value["action"], "playlist.create");
    assert_eq!(value["payload"]["title"], "夜の音楽");
    assert_eq!(value["magic"], "TMV");
    assert_eq!(value["type"], "EVT");
    assert_eq!(
        classify_text(&format!(
            "TMV/1 EVT\n{}",
            String::from_utf8(encoded).unwrap()
        ))
        .unwrap(),
        Some(r)
    );
}

#[test]
fn checked_in_wire_fixture_matches_canonical_event_and_schema_contract() {
    let fixture = include_str!("../../../spec/test-vectors/evt-playlist-create-v1.json")
        .trim_end()
        .as_bytes();
    let record = decode(fixture).unwrap();
    assert_eq!(record, event());
    assert_eq!(encode(&record).unwrap(), fixture);

    let schema: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../../spec/schema/event-v1.schema.json"))
            .unwrap();
    assert_eq!(schema["properties"]["magic"]["const"], "TMV");
    assert_eq!(schema["properties"]["protocol"]["const"], 1);
    assert_eq!(schema["properties"]["type"]["const"], "EVT");
    assert!(
        schema["properties"]["action"]["enum"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("track.media.replace"))
    );
    assert!(
        schema["properties"]["action"]["enum"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("playlist.metadata.update"))
    );
    assert_eq!(
        schema["properties"]["action"]["enum"]
            .as_array()
            .unwrap()
            .len(),
        14
    );
}

#[test]
fn unknown_version_action_field_and_duplicate_keys_fail_closed() {
    let encoded = encode(&event()).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    for (field, replacement) in [
        ("protocol", serde_json::json!(2)),
        ("type", serde_json::json!("FUTURE")),
        ("action", serde_json::json!("future.op")),
        ("unexpected", serde_json::json!(true)),
    ] {
        let mut corrupt = value.clone();
        corrupt[field] = replacement;
        assert!(decode(&serde_json::to_vec(&corrupt).unwrap()).is_err());
    }
    let mut payload = value.clone();
    payload["payload"]["secret_future_field"] = serde_json::json!(1);
    assert!(decode(&serde_json::to_vec(&payload).unwrap()).is_err());
    let json = String::from_utf8(encoded).unwrap();
    let duplicate = json.replacen('{', "{\"protocol\":2,", 1);
    assert!(decode(duplicate.as_bytes()).is_err());
    assert!(classify_text("TMV/2 EVT\n{}").is_err());
    assert!(classify_text("TMV/1 FUTURE\n{}").is_err());
    assert!(classify_text("TMV/1 EVT\n{broken").is_err());
}

#[test]
fn ordinary_notes_stay_unclassified_and_canonical_objects_sort_keys() {
    assert_eq!(classify_text("my music note").unwrap(), None);
    assert_eq!(classify_text("{\"personal\":true}").unwrap(), None);
    assert_eq!(
        canonical_json(&serde_json::json!({"z":1,"a":{"z":2,"a":3}})).unwrap(),
        br#"{"a":{"a":3,"z":2},"z":1}"#
    );
    for invalid in [
        "0",
        "-1",
        "01",
        "+1",
        " 1",
        "1e3",
        "-0",
        "9223372036854775808",
    ] {
        assert!(invalid.parse::<MessageId>().is_err());
    }
}
