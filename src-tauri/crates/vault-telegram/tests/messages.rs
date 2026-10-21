#![allow(clippy::unwrap_used)]
use serde_json::json;
use vault_domain::ChatId;
use vault_telegram::*;

fn message() -> serde_json::Value {
    json!({"@type":"message","id":10485760,"chat_id":-100,"sending_state":null,"scheduling_state":null,"is_channel_post":true,"forward_info":null,"edit_date":0,"sender_id":{"@type":"messageSenderChat","chat_id":-100},"content":{"@type":"messageText","text":{"text":"personal note","entities":[]}}})
}
#[test]
fn temporary_scheduled_foreign_and_inexact_ids_never_enter_replay() {
    let chat = ChatId::new(-100).unwrap();
    let value = message();
    assert_eq!(parse_message(&value, chat).unwrap().id.get(), 10485760);
    for (key, corrupt) in [
        (
            "sending_state",
            json!({"@type":"messageSendingStatePending"}),
        ),
        (
            "scheduling_state",
            json!({"@type":"messageSchedulingStateSendAtDate"}),
        ),
        ("chat_id", json!(-101)),
        ("id", json!(10485761)),
        ("id", json!(10485760.5)),
    ] {
        let mut value = value.clone();
        value[key] = corrupt;
        assert!(parse_message(&value, chat).is_err());
    }
    for invalid in [
        json!("01"),
        json!("+42"),
        json!("9223372036854775808"),
        json!(9.25),
    ] {
        assert!(integer(&invalid).is_err());
    }
    assert_eq!(
        integer(&json!("9007199254740993")).unwrap(),
        9007199254740993
    );
}
#[test]
fn channel_ownership_privacy_and_membership_fail_closed() {
    let chat = json!({"id":-100,"type":{"@type":"chatTypeSupergroup","is_channel":true},"title":"renamed","message_auto_delete_time":0});
    let group = json!({"is_channel":true,"status":{"@type":"chatMemberStatusCreator","is_member":true},"has_location":false,"has_linked_chat":false,"usernames":null});
    let full = json!({"member_count":1,"administrator_count":1,"linked_chat_id":0});
    let id = ChatId::new(-100).unwrap();
    let owner = vault_domain::UserId::new(42).unwrap();
    assert!(validate_channel(&chat, &group, &full, id, owner).is_ok());
    let mut public = group.clone();
    public["usernames"] = json!({"active_usernames":["public"]});
    assert!(validate_channel(&chat, &public, &full, id, owner).is_err());
    let mut member = full.clone();
    member["member_count"] = json!(2);
    assert!(validate_channel(&chat, &group, &member, id, owner).is_err());
    let mut admin = group.clone();
    admin["status"]["@type"] = json!("chatMemberStatusAdministrator");
    assert!(validate_channel(&chat, &admin, &full, id, owner).is_err());
    let mut expires = chat.clone();
    expires["message_auto_delete_time"] = json!(3600);
    assert!(validate_channel(&expires, &group, &full, id, owner).is_err());
}
