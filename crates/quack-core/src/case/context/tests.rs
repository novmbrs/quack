use serde_json::json;

use super::*;

const G: u64 = 123_456_789_012_345_678;

fn guild() -> GuildId {
    GuildId::new(G).unwrap()
}

fn field(key: &str, kind: FieldKind, required: bool) -> Field {
    Field {
        key: FieldKey(key.to_owned()),
        label: key.to_owned(),
        kind,
        required,
    }
}

fn link() -> String {
    format!("https://discord.com/channels/{G}/223456789012345678/323456789012345678")
}

fn one(kind: FieldKind, input: ContextInput) -> Result<Option<ContextValue>, ContextProblemCode> {
    let fields = [field("f", kind, false)];
    validate_context(&fields, guild(), &[("f".to_owned(), input)])
        .map(|v| v.get(&FieldKey("f".into())).cloned())
        .map_err(|p| p[0].code)
}

fn text(s: &str) -> ContextInput {
    ContextInput::Text(s.to_owned())
}

fn api(v: serde_json::Value) -> ContextInput {
    ContextInput::Json(v)
}

#[test]
fn text_fields() {
    use ContextProblemCode as P;
    assert_eq!(
        one(FieldKind::ShortText, text("  hi  ")),
        Ok(Some(ContextValue::Text("hi".into())))
    );
    assert_eq!(
        one(FieldKind::ShortText, api(json!("x".repeat(500)))).map(|v| v.is_some()),
        Ok(true)
    );
    assert_eq!(
        one(FieldKind::ShortText, api(json!("x".repeat(501)))),
        Err(P::TooLong)
    );
    assert_eq!(
        one(FieldKind::LongText, text(&"x".repeat(4000))).map(|v| v.is_some()),
        Ok(true)
    );
    assert_eq!(
        one(FieldKind::LongText, text(&"x".repeat(4001))),
        Err(P::TooLong)
    );
    assert_eq!(one(FieldKind::ShortText, api(json!(5))), Err(P::NotText));
}

#[test]
fn booleans() {
    use ContextProblemCode as P;
    assert_eq!(
        one(FieldKind::Boolean, api(json!(false))),
        Ok(Some(ContextValue::Boolean(false)))
    );
    assert_eq!(
        one(FieldKind::Boolean, text("yes")),
        Ok(Some(ContextValue::Boolean(true)))
    );
    assert_eq!(
        one(FieldKind::Boolean, text("no")),
        Ok(Some(ContextValue::Boolean(false)))
    );
    for bad in [text("true"), text("Yes"), api(json!("yes")), api(json!(1))] {
        assert_eq!(
            one(FieldKind::Boolean, bad.clone()),
            Err(P::NotYesNo),
            "{bad:?}"
        );
    }
}

fn raw(s: &str) -> ContextInput {
    ContextInput::from_json_text(s).unwrap()
}

#[test]
fn numbers() {
    use ContextProblemCode as P;
    let num = |s: &str| Ok(Some(ContextValue::Number(Decimal::parse(s).unwrap())));
    assert_eq!(one(FieldKind::Number, api(json!(0))), num("0"));
    assert_eq!(one(FieldKind::Number, api(json!(-7))), num("-7"));
    assert_eq!(one(FieldKind::Number, api(json!(1.5))), num("1.5"));
    assert_eq!(one(FieldKind::Number, raw("2.50")), num("2.5"));
    assert_eq!(one(FieldKind::Number, raw(" 0.000001 ")), num("0.000001"));
    assert_eq!(
        one(FieldKind::Number, raw("123456789012345.123456")),
        num("123456789012345.123456")
    );
    assert_eq!(one(FieldKind::Number, api(json!("5"))), num("5"));
    assert_eq!(one(FieldKind::Number, api(json!(" -1.25 "))), num("-1.25"));
    assert_eq!(
        one(FieldKind::Number, text("-12.345678")),
        num("-12.345678")
    );
    assert_eq!(one(FieldKind::Number, text("5.0")), num("5"));
    assert_eq!(
        one(FieldKind::Number, text("1000000000000000")),
        num("1000000000000000")
    );
    for bad in [
        text("abc"),
        text("1e5"),
        text("+1"),
        text(".5"),
        text("5."),
        text("1,5"),
        text("--1"),
        text("0x10"),
        text("1.2.3"),
        text("1 5"),
        raw("1e5"),
        raw("1E-3"),
        api(json!("2e3")),
        api(json!(true)),
        api(json!([1])),
    ] {
        assert_eq!(
            one(FieldKind::Number, bad.clone()),
            Err(P::NotANumber),
            "{bad:?}"
        );
    }
    for bad in [
        text("1000000000000001"),
        text("0.1234567"),
        raw("1000000000000000.5"),
        raw("-0.0000001"),
        api(json!(u64::MAX)),
        api(json!(i64::MIN)),
    ] {
        assert_eq!(
            one(FieldKind::Number, bad.clone()),
            Err(P::NumberOutOfRange),
            "{bad:?}"
        );
    }
}

#[test]
fn json_text_keeps_numbers_exact() {
    assert_eq!(raw("2.50"), ContextInput::JsonNumber("2.50".into()));
    assert_eq!(raw(" true "), api(json!(true)));
    assert_eq!(raw("\"x\""), api(json!("x")));
    assert!(ContextInput::from_json_text("{").is_err());
}

#[test]
fn message_links() {
    use ContextProblemCode as P;
    let messy = format!(
        "  {}?foo#bar ",
        link().replace("discord.com", "ptb.discordapp.com")
    );
    assert_eq!(
        one(FieldKind::MessageLink, text(&messy)),
        Ok(Some(ContextValue::Text(link())))
    );
    assert_eq!(
        one(FieldKind::MessageLink, text("https://example.com")),
        Err(P::NotAMessageLink)
    );
    assert_eq!(
        one(FieldKind::MessageLink, api(json!(1))),
        Err(P::NotAMessageLink)
    );
    let other = link().replace(&G.to_string(), "999999999999999999");
    assert_eq!(
        one(FieldKind::MessageLink, text(&other)),
        Err(P::OtherServer)
    );
}

#[test]
fn absence_required_unknown_and_duplicates() {
    let fields = [
        field("a", FieldKind::ShortText, true),
        field("b", FieldKind::Boolean, false),
        field("c", FieldKind::Number, true),
    ];
    let submitted = vec![
        ("a".to_owned(), text("   ")),
        ("b".to_owned(), api(serde_json::Value::Null)),
        ("zzz".to_owned(), text("x")),
        ("b".to_owned(), text("yes")),
    ];
    let problems = validate_context(&fields, guild(), &submitted).unwrap_err();
    let got: Vec<_> = problems
        .iter()
        .map(|p| (p.key.as_str(), p.code.as_str(), p.path()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("zzz", "unknown_field", "context.zzz".to_owned()),
            ("b", "duplicate_field", "context.b".to_owned()),
            ("a", "required", "context.a".to_owned()),
            ("c", "required", "context.c".to_owned()),
        ]
    );

    let ok = validate_context(
        &fields,
        guild(),
        &[("a".into(), text("x")), ("c".into(), raw("0.50"))],
    )
    .unwrap();
    assert_eq!(ok.0.len(), 2);
    assert_eq!(ok.get(&FieldKey("b".into())), None);
    assert_eq!(
        serde_json::to_value(&ok).unwrap(),
        json!({ "a": "x", "c": "0.5" })
    );
}

#[test]
fn stored_values_round_trip_through_json() {
    let fields = [
        field("t", FieldKind::ShortText, false),
        field("b", FieldKind::Boolean, false),
        field("n", FieldKind::Number, false),
    ];
    let values = validate_context(
        &fields,
        guild(),
        &[
            ("t".into(), text("hi")),
            ("b".into(), text("no")),
            ("n".into(), text("2.5")),
        ],
    )
    .unwrap();
    let json = serde_json::to_string(&values).unwrap();
    assert_eq!(json, r#"{"b":false,"n":"2.5","t":"hi"}"#);
    let read = serde_json::from_str::<ContextValues>(&json).unwrap();
    // Untyped, a stored number reads back as its canonical string.
    assert_eq!(
        read.get(&FieldKey("n".into())),
        Some(&ContextValue::Text("2.5".into()))
    );
    assert_eq!(read.with_kinds(&fields), values);
}

#[test]
fn fill_and_read_message_links() {
    let fields = [
        field("m", FieldKind::MessageLink, true),
        field("n", FieldKind::MessageLink, false),
        field("t", FieldKind::ShortText, false),
    ];
    let source = MessageLink::parse(&link()).unwrap();
    let mut submitted = vec![("n".to_owned(), text(" "))];
    fill_message_links(&fields, &mut submitted, &source);
    assert_eq!(submitted.len(), 2);
    let values = validate_context(&fields, guild(), &submitted).unwrap();
    assert_eq!(values.message_links(&fields), vec![source, source]);

    let own = link().replace("323456789012345678", "423456789012345678");
    let mut kept = vec![("m".to_owned(), text(&own))];
    fill_message_links(&fields, &mut kept, &source);
    assert_eq!(kept[0].1, text(&own));
}

#[test]
fn changed_keys() {
    let k = |s: &str| FieldKey(s.into());
    let old = ContextValues(
        [
            (k("a"), ContextValue::Boolean(true)),
            (k("b"), ContextValue::Text("x".into())),
        ]
        .into(),
    );
    let new = ContextValues(
        [
            (k("a"), ContextValue::Boolean(true)),
            (k("b"), ContextValue::Text("y".into())),
            (k("c"), ContextValue::Boolean(false)),
        ]
        .into(),
    );
    assert_eq!(old.changed_keys(&new), vec![k("b"), k("c")]);
    assert_eq!(
        new.changed_keys(&ContextValues::default()),
        vec![k("a"), k("b"), k("c")]
    );
}

#[test]
fn a_rule_without_fields_takes_free_text_details() {
    let def = crate::rule::RuleDefinition {
        fields: Vec::new(),
        ..crate::rule::starter_rule()
    };
    let fields = def.context_fields();
    let values = validate_context(
        &fields,
        guild(),
        &[("details".into(), text(" what happened "))],
    )
    .unwrap();
    assert_eq!(
        values.get(&FieldKey("details".into())),
        Some(&ContextValue::Text("what happened".into()))
    );
    assert_eq!(
        validate_context(&fields, guild(), &[]),
        Ok(ContextValues::default())
    );
    let too_long = validate_context(
        &fields,
        guild(),
        &[("details".into(), text(&"x".repeat(4001)))],
    );
    assert_eq!(too_long.unwrap_err()[0].code, ContextProblemCode::TooLong);
    let unknown = validate_context(&fields, guild(), &[("other".into(), text("x"))]);
    assert_eq!(
        unknown.unwrap_err()[0].code,
        ContextProblemCode::UnknownField
    );
}

#[test]
fn evidence_links_include_links_inside_text() {
    let fields = [
        field("m", FieldKind::MessageLink, false),
        field("t", FieldKind::LongText, false),
        field("s", FieldKind::ShortText, false),
    ];
    let other = "https://discord.com/channels/999/1/2";
    let second = link().replace("323456789012345678", "423456789012345678");
    let values = validate_context(
        &fields,
        guild(),
        &[
            ("m".into(), text(&link())),
            (
                "t".into(),
                text(&format!("see {second} and {other} and {}", link())),
            ),
            ("s".into(), text(&format!("<{second}>"))),
        ],
    )
    .unwrap();
    // The text is stored unchanged.
    assert!(
        matches!(values.get(&FieldKey("t".into())), Some(ContextValue::Text(t)) if t.contains(other))
    );
    let urls: Vec<String> = values
        .evidence_links(&fields, guild())
        .iter()
        .map(MessageLink::to_url)
        .collect();
    assert_eq!(urls, [link(), second]);
}
