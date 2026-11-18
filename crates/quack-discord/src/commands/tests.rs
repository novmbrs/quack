use httpmock::MockServer;
use quack_core::ids::GuildId;
use quack_text::model::Command as TextCommand;
use serde_json::{Value, json};

use super::sync::matches;
use super::*;
use crate::rest::RegisteredCommand;
use crate::rest::tests::client;

fn local_json() -> Vec<Value> {
    definitions()
        .iter()
        .map(|c| serde_json::to_value(c).unwrap())
        .collect()
}

/// What Discord answers for a registered definition: ids, a version, the
/// application, and every default field filled in.
fn as_discord_returns(definition: &Value, id: u64) -> Value {
    fn fill(value: &mut Value) {
        if let Value::Object(map) = value {
            map.entry("name_localizations").or_insert(Value::Null);
            map.entry("description_localizations")
                .or_insert(Value::Null);
            if let Some(Value::Array(options)) = map.get_mut("options") {
                for option in options {
                    fill(option);
                    if let Value::Object(o) = option {
                        o.entry("required").or_insert(json!(false));
                    }
                }
            }
        }
    }
    let mut value = definition.clone();
    fill(&mut value);
    let map = value.as_object_mut().unwrap();
    map.insert("id".into(), json!(id.to_string()));
    map.insert("application_id".into(), json!("42"));
    map.insert("version".into(), json!("1300000000000000000"));
    map.insert("dm_permission".into(), json!(true));
    map.insert("nsfw".into(), json!(false));
    map.entry("default_member_permissions")
        .or_insert(Value::Null);
    value
}

fn registered(values: &[Value]) -> Vec<RegisteredCommand> {
    values
        .iter()
        .map(|v| RegisteredCommand {
            id: v["id"].as_str().unwrap().parse().unwrap(),
            kind: u8::try_from(v["type"].as_u64().unwrap_or(1)).unwrap(),
            name: v["name"].as_str().unwrap().to_owned(),
            definition: v.clone(),
        })
        .collect()
}

fn remote() -> Vec<Value> {
    local_json()
        .iter()
        .zip(100..)
        .map(|(v, id)| as_discord_returns(v, id))
        .collect()
}

#[test]
fn definitions_snapshot() {
    insta::assert_json_snapshot!(local_json());
}

#[test]
fn the_registered_form_drops_defaults_twilight_fills_in() {
    let case = canonical(&local_json()[0]);
    let rule = &case["options"][0]["options"][1];
    assert_eq!(rule["autocomplete"], json!(true));
    assert!(rule.get("choices").is_none());
    assert!(case.get("version").is_none());
    assert!(case.get("default_member_permissions").is_none());
    let help = canonical(&local_json()[4]);
    assert_eq!(
        help["options"][0]["choices"].as_array().map(Vec::len),
        Some(4)
    );
}

#[test]
fn every_command_is_guild_only_and_only_setup_needs_a_permission() {
    let local = local_json();
    assert_eq!(local.len(), 7);
    for command in &local {
        assert_eq!(command["contexts"], json!([0]), "{}", command["name"]);
        assert_eq!(command["integration_types"], json!([0]));
        let expected = if command["name"] == "setup" {
            json!("32")
        } else {
            Value::Null
        };
        assert_eq!(
            command
                .get("default_member_permissions")
                .cloned()
                .unwrap_or(Value::Null),
            expected,
            "{}",
            command["name"]
        );
    }
}

#[test]
fn discords_echo_of_the_definitions_matches() {
    let local = definitions();
    let mut remote = remote();
    assert!(matches(&local, &registered(&remote)));
    // Order of commands doesn't matter; option order does.
    remote.reverse();
    assert!(matches(&local, &registered(&remote)));
    let mut reordered = remote.clone();
    let options = reordered
        .iter_mut()
        .find(|c| c["name"] == "case")
        .and_then(|c| c["options"].as_array_mut())
        .unwrap();
    options.swap(0, 1);
    assert!(!matches(&local, &registered(&reordered)));
}

#[test]
fn any_difference_or_go_era_command_needs_an_overwrite() {
    let local = definitions();
    let mut changed = remote();
    changed[1]["description"] = json!("Old wording.");
    assert!(!matches(&local, &registered(&changed)));
    let mut extra = remote();
    extra.push(as_discord_returns(
        &json!({"name": "template", "description": "Go-era", "type": 1}),
        999,
    ));
    assert!(!matches(&local, &registered(&extra)));
    let mut missing = remote();
    missing.pop();
    assert!(!matches(&local, &registered(&missing)));
}

#[test]
fn command_ids_cover_subcommands() {
    let ids = command_ids(&registered(&remote()));
    let id_of = |c: TextCommand| ids.0.iter().find(|(k, _)| *k == c).map(|(_, id)| *id);
    assert_eq!(id_of(TextCommand::CaseView), Some(100));
    assert_eq!(id_of(TextCommand::CaseVoid), Some(100));
    assert_eq!(id_of(TextCommand::Appeals), Some(101));
    assert_eq!(id_of(TextCommand::Help), Some(104));
    assert_eq!(ids.0.len(), 9);
}

#[tokio::test]
async fn sync_keeps_equal_commands_and_overwrites_changed_ones() {
    let server = MockServer::start_async().await;
    let list = server
        .mock_async(|when, then| {
            when.method("GET")
                .path("/api/v10/applications/42/guilds/1/commands");
            then.status(200).json_body(json!(remote()));
        })
        .await;
    let put = server
        .mock_async(|when, then| {
            when.method("PUT")
                .path("/api/v10/applications/42/commands")
                .json_body(json!(
                    local_json().iter().map(canonical).collect::<Vec<_>>()
                ));
            then.status(200).json_body(json!(remote()));
        })
        .await;
    let stale = server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/applications/42/commands");
            then.status(200).json_body(json!([as_discord_returns(
                &json!({"name": "template", "description": "Go-era", "type": 1}),
                5
            )]));
        })
        .await;
    let client = client(&server.base_url());
    let dev = sync(
        &client,
        Scope::Guild(GuildId::new(1).unwrap()),
        &definitions(),
    )
    .await
    .unwrap();
    assert!(!dev.changed);
    assert_eq!(dev.ids.0.len(), 9);
    list.assert_async().await;
    let global = sync(&client, Scope::Global, &definitions()).await.unwrap();
    assert!(global.changed);
    stale.assert_async().await;
    put.assert_async().await;
}

#[tokio::test]
async fn sync_failures_are_errors() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/applications/42/commands");
            then.status(401)
                .json_body(json!({"code": 0, "message": "401: Unauthorized"}));
        })
        .await;
    let client = client(&server.base_url());
    let error = sync(&client, Scope::Global, &definitions())
        .await
        .unwrap_err();
    assert!(matches!(error, SyncError::List(ref why) if why.contains("http_401")));
}
