#!/usr/bin/env python3
# Generates data.sql and broken.sql: python3 gen_fixture.py <this directory>
"""Generates crates/quack-import/tests/fixtures/{data,broken}.sql.

Every ID is a readable ULID: 01JA + 2-letter kind + zero padding + number.
"""
import json
import sys
from datetime import datetime, timedelta

OUT = sys.argv[1]
BASE = datetime(2026, 1, 1)
NOW = "NOW"


def ulid(kind, n):
    # The number sits high in the random part: ulid_to_uuid drops the low
    # 6 bits, so IDs that differ only at the end would collide.
    s = "01JA" + kind + str(n).rjust(4, "0") + "0" * 16
    assert len(s) == 26 and not set(s) & set("ILOU"), s
    return s


def t(minutes):
    return (BASE + timedelta(minutes=minutes)).strftime("%Y-%m-%d %H:%M:%S.%f")[:-3]


def q(v):
    if v is None:
        return "NULL"
    if isinstance(v, bool):
        return "1" if v else "0"
    if isinstance(v, int):
        return str(v)
    if isinstance(v, (dict, list)):
        v = json.dumps(v, separators=(",", ":"))
    return "'" + v.replace("\\", "\\\\").replace("'", "\\'") + "'"


DEFAULTS = {
    "appeal_events": dict(actor_type="member", body="", metadata_json={}),
    "appeal_notifications": dict(body="", status="sent"),
    "appeals": dict(statement="please", metadata_json={}),
    "audit_log_entries": dict(source="discord", resource_type="case", resource_id="unknown", result="success", metadata_json={}),
    "audit_mirror_deliveries": dict(status="delivered"),
    "case_action_attempts": dict(request_payload_json={}, response_payload_json={}),
    "case_action_executions": dict(position=1, config_snapshot_json={}, safe_for_retry=True),
    "case_events": dict(body="", metadata_json={}),
    "case_evidence_attachments": dict(content_type="image/png", size_bytes=100, preserved_url="", preserved_message_discord_id="", preserved_attachment_discord_id="", copy_outcome="metadata_only", warning=""),
    "case_evidence_snapshots": dict(message_url="", content="", embeds_json=[], capture_outcome="captured", capture_warning=""),
    "case_notifications": dict(rendered_message="dm text", last_error=""),
    "case_publications": dict(presentation_json='{"view":"case_receipt"}', refresh_requested=False),
    "cases": dict(reason="Reason", source="discord", metadata_json={}, context_values_json=[]),
    "case_template_context_fields": dict(required=False),
    "case_template_level_actions": dict(config_json={}),
    "case_template_levels": dict(name="Level", is_default=False, trigger_case_count=0, notify_user=True),
    "case_templates": dict(description="", appealable=True, version=1, case_decay_days=0, created_by_discord_user_id="300000000000000001", updated_by_discord_user_id="300000000000000001"),
    "guild_settings": dict(notification_introduction="", notification_footer="", starter_policy_notice_pending=False),
    "guilds": dict(icon_url="", is_active=True),
    "honeypot_message_cleanups": dict(attempt_count=1),
    "honeypot_triggers": dict(),
    "module_configurations": dict(enabled=True),
    "staff_members": dict(),
    "ticket_events": dict(body="", metadata_json={}),
    "ticket_member_states": dict(open_count=1),
    "ticket_message_journal": dict(body="hi"),
    "tickets": dict(metadata_json={}),
}

STATEMENTS = []
COUNTER = [0]


def ins(table, **cols):
    COUNTER[0] += 1
    row = dict(DEFAULTS.get(table, {}))
    row.update(cols)
    none = ("honeypot_warning_refreshes", "ticket_message_journal", "ticket_transcripts", "quack_schema_migrations")
    created_only = ("honeypot_message_cleanups", "v4_import_batches", "v4_import_sources")
    for k in ("created_at", "updated_at"):
        if k not in row and table not in none and not (k == "updated_at" and table in created_only):
            row[k] = t(COUNTER[0])
    names = ", ".join(f"`{k}`" for k in row)
    values = ", ".join(q(v) for v in row.values())
    STATEMENTS.append(f"INSERT INTO `{table}` ({names}) VALUES ({values})")


def snapshot(template_id, name, version, trigger, matched, *, default=False, notify=True, actions=(), position=1, reason="Reason", decay=0, fields=()):
    return {
        "template": {"id": template_id, "slug": name.lower(), "name": name, "version": version,
                     "reason_template": reason, "case_decay_days": decay, "appealable": True},
        "selected_level": {"id": "x", "name": "Level", "position": position, "is_default": default,
                           "trigger_case_count": trigger, "notify_user": notify, "matched_case_count": matched},
        "actions": list(actions),
        "context_fields": list(fields),
        "context_values": [],
    }


# ---------------------------------------------------------------- servers
GA, GB = ulid("GD", 1), ulid("GD", 2)
SA, SB = "100000000000000001", "100000000000000002"
OWNER, MOD, MOD2 = "300000000000000001", "300000000000000002", "300000000000000003"
M = {i: f"2000000000000000{i:02d}" for i in range(1, 20)}
CH = {i: f"4000000000000000{i:02d}" for i in range(1, 20)}

ins("guilds", id=GA, discord_guild_id=SA, name="Guild A", icon_url=f"https://cdn.discordapp.com/icons/{SA}/a_abc123.png",
    owner_discord_user_id=OWNER, created_at=t(0), updated_at=t(1))
ins("guilds", id=GB, discord_guild_id=" " + SB, name="Guild B", owner_discord_user_id=OWNER, is_active=False)

T = {i: ulid("TP", i) for i in range(1, 12)}
ins("guild_settings", id=ulid("GS", 1), guild_id=GA, audit_mirror_channel_discord_id=CH[1],
    appeal_queue_channel_discord_id=CH[2], managed_evidence_channel_discord_id="",
    notification_introduction="Hello", notification_footer="", starter_policy_template_id=T[1],
    starter_policy_notice_acknowledged_at=t(5), appeal_rejoin_url="https://discord.com/invite/abcd",
    appeal_review_reason_required=True)
ins("staff_members", id=ulid("SF", 1), guild_id=GA, discord_user_id=MOD, last_seen_permission_bits=18446744073709551615)
ins("staff_members", id=ulid("SF", 2), guild_id=GA, discord_user_id=MOD2)

# ---------------------------------------------------------------- rules
def template(i, guild, name, *, version=1, archived=None, description="", reason="Reason", decay=0, created=10):
    ins("case_templates", id=T[i], guild_id=guild, slug=f"slug-{i}", name=name, version=version, archived_at=archived,
        description=description, reason_template=reason, case_decay_days=decay, created_at=t(created + i), updated_at=t(created + i + 1))


LV = [0]


def level(tpl, position, *, default=False, trigger=0, notify=True, action=None, config=None):
    LV[0] += 1
    lid = ulid("TV", LV[0])
    ins("case_template_levels", id=lid, template_id=tpl, position=position, is_default=default, trigger_case_count=trigger, notify_user=notify)
    if action:
        ins("case_template_level_actions", id=ulid("TA", LV[0]), level_id=lid, action_type=action, config_json=config or {}, max_retries=3)


template(1, GA, "Spam", decay=30, reason="No spam")
ins("case_template_context_fields", id=ulid("TF", 1), template_id=T[1], key="where", label="Where", field_type="discord_message_link", position=1, required=True)
ins("case_template_context_fields", id=ulid("TF", 2), template_id=T[1], key="note", label="Note", field_type="long_text", position=2)
level(T[1], 1, default=True)
level(T[1], 2, trigger=3, action="timeout_user", config={"duration_seconds": 600})
level(T[1], 3, trigger=5, action="ban_user", config={"delete_message_seconds": 3600})

template(2, GA, "Harassment", version=3, reason="Be kind")
ins("case_template_context_fields", id=ulid("TF", 3), template_id=T[2], key="count", label="How many", field_type="number", position=1)
ins("case_template_context_fields", id=ulid("TF", 4), template_id=T[2], key="flag", label="Repeat?", field_type="boolean", position=2)
level(T[2], 1, default=True)
level(T[2], 2, trigger=2, action="kick_user")

template(3, GA, "spam", created=40)                          # clashes with Spam
level(T[3], 1, default=True, notify=False)
template(4, GA, "Old rule", archived=t(30), description="d" * 600)  # archived, over-limit description
level(T[4], 1, default=True)
template(5, GA, "Default dropped")
level(T[5], 1, default=True)
level(T[5], 2, trigger=1, action="kick_user")
template(6, GA, "Spam (2)", archived=t(31))                  # forces "spam (3)"
level(T[6], 1, default=True)
template(7, GA, "Honeypot")
level(T[7], 1, default=True, action="ban_user")
template(8, GA, "L" * 120)                                   # over-limit name
level(T[8], 1, default=True, action="send_dm")
template(10, GB, "Spam")                                     # same name, other server
level(T[10], 1, default=True)

# ---------------------------------------------------------------- cases
C = {i: ulid("CS", i) for i in range(1, 30)}
TIMEOUT = {"id": "a", "action_type": "timeout_user", "timeout_duration_seconds": 600, "max_retries": 3}
BAN = {"id": "a", "action_type": "ban_user", "delete_message_seconds": 3600, "max_retries": 3}
KICK = {"id": "a", "action_type": "kick_user", "max_retries": 3}
link = f"https://discord.com/channels/{SA}/{CH[3]}/500000000000000001"


def case(i, guild, number, member, *, tpl=None, version=1, snap=None, source="discord", moderator=MOD, validity="valid",
         voided_at=None, voided_reason=None, voided_by="", replaces=None, key=None, context=None, metadata=None, reason="Reason", minute=None):
    ins("cases", id=C[i], guild_id=guild, case_number=number, template_id=tpl, template_version=version,
        template_snapshot_json=snap if snap is not None else {}, target_discord_user_id=member, moderator_discord_user_id=moderator,
        reason=reason, validity=validity, source=source, voided_at=voided_at, voided_reason=voided_reason,
        voided_by_discord_user_id=voided_by, replaces_case_id=replaces, idempotency_key=key,
        context_values_json=context if context is not None else [], metadata_json=metadata if metadata is not None else {},
        created_at=t(100 + (minute or i)), updated_at=t(101 + (minute or i)))


case(1, GA, 1, M[1], tpl=T[1], snap=snapshot(T[1], "Spam", 1, 0, 1, default=True), key="discord:111",
     context=[{"key": "where", "label": "Where", "type": "discord_message_link", "required": True, "value": link},
              {"key": "note", "label": "Note", "type": "long_text", "required": False, "value": "context note"},
              {"key": "empty", "value": ""}])
case(2, GA, 2, M[1], tpl=T[1], snap=snapshot(T[1], "Spam", 1, 3, 3, actions=[TIMEOUT], position=2), source="dashboard")
case(3, GA, 3, M[2], tpl=T[1], snap=snapshot(T[1], "Spam", 1, 5, 5, actions=[BAN], position=3))
# Harassment v1 and v2 exist only in snapshots.
v1_fields = [{"id": "f", "key": "count", "label": "How many", "type": "number", "position": 1, "required": False}]
case(4, GA, 4, M[3], tpl=T[2], version=1, snap=snapshot(T[2], "Harassment", 1, 2, 2, actions=[dict(TIMEOUT, timeout_duration_seconds=60)], position=2, reason="Be nice", fields=v1_fields),
     context=[{"key": "count", "value": 2.50}])
case(5, GA, 5, M[4], tpl=T[2], version=2, snap=snapshot(T[2], "Harassment", 2, 0, 1, default=True, reason="Be kinder"),
     validity="voided", voided_at=t(200), voided_reason="", voided_by=MOD)
case(6, GA, 6, M[3], tpl=T[2], version=1, snap=snapshot(T[2], "Harassment (old)", 1, 1, 1, default=True, reason="Be nice", fields=v1_fields), minute=3,
     context=[{"key": "count", "value": 7}, {"key": "flag", "value": True}])
case(7, GA, 7, M[5], tpl=T[5], snap=snapshot(T[5], "Default dropped", 1, 1, 1, actions=[KICK], position=2))
case(8, GA, 8, M[4], tpl=T[2], version=3, snap=snapshot(T[2], "Harassment", 3, 0, 1, default=True), replaces=C[5])
case(9, GA, 9, M[4], tpl=T[2], version=3, snap=snapshot(T[2], "Harassment", 3, 0, 1, default=True), replaces=C[5])
case(10, GA, 10, M[6], source="v4_import", moderator="", reason="old reason", snap={"historical": True, "v4_action_type": "ban"},
     version=0, metadata={"historical": True, "v4": {"source_name": "v4", "source_id": "abc", "case_number": 7, "action_type": "ban",
                                                     "moderator_display_name": "OldMod", "target_departed": False, "target_missing": False, "action_expires_at": None}})
case(11, GA, 11, M[7], tpl=T[7], source="honeypot", moderator="quack-system", snap=snapshot(T[7], "Honeypot", 1, 0, 1, default=True, actions=[dict(BAN, delete_message_seconds=0)]),
     key=f"honeypot:{GA}:500000000000000099")
case(12, GA, 12, M[8], tpl=T[1], snap=snapshot(T[1], "Spam", 1, 3, 3, actions=[TIMEOUT], position=2))
case(13, GA, 13, M[9], tpl=T[1], snap=snapshot(T[1], "Spam", 1, 0, 1, default=True))
case(14, GA, 14, "not-a-member", tpl=T[1], snap=snapshot(T[1], "Spam", 1, 0, 1, default=True))
case(15, GA, 15, M[10], tpl=T[1], snap=snapshot(T[1], "Spam", 1, 3, 3, actions=[TIMEOUT], position=2), validity="voided", voided_reason=None)
case(17, GA, 17, M[11], tpl=T[1], snap=snapshot(T[1], "Spam", 1, 5, 5, actions=[BAN], position=3), validity="voided", voided_at=t(300), voided_reason="mistake", voided_by=MOD)
GHOST = ulid("TP", 99)
case(18, GA, 18, M[12], tpl=GHOST, version=4, snap=snapshot(GHOST, "Ghost rule", 4, 0, 1, default=True))
case(19, GA, 19, M[13], tpl=T[10], snap=snapshot(T[10], "Spam", 1, 0, 1, default=True))
case(20, GA, 9007199254740991, M[14], tpl=T[1], snap=snapshot(T[1], "Spam", 1, 0, 1, default=True))
case(21, GB, 1, M[1], tpl=T[10], snap=snapshot(T[10], "Spam", 1, 0, 1, default=True))
case(22, GA, 22, M[15], tpl=T[8], snap=snapshot(T[8], "L" * 120, 1, 0, 1, default=True))
case(23, GA, 23, M[16], tpl=T[3], snap=snapshot(T[3], "spam", 1, 0, 1, default=True, notify=False))

# ---------------------------------------------------------------- actions
E = {i: ulid("EX", i) for i in range(1, 30)}
AT = [0]


def execution(i, case_id, action, status, *, code="", config=None, reversal=None, finished=None, dismissed=None, dismissed_by=""):
    ins("case_action_executions", id=E[i], case_id=case_id, action_type=action, status=status, idempotency_key=f"exec-{i}",
        config_snapshot_json=config or {}, last_error_code=code, finished_at=finished, reversal_of_execution_id=reversal,
        dismissed_at=dismissed, dismissed_by_discord_user_id=dismissed_by)


def attempt(execution_id, number, status, *, code="", response=None, started=500, finished=501):
    AT[0] += 1
    ins("case_action_attempts", id=ulid("AT", AT[0]), execution_id=execution_id, attempt_number=number, status=status,
        error_code=code, response_payload_json=response or {}, started_at=t(started), finished_at=None if finished is None else t(finished))


execution(1, C[2], "timeout_user", "succeeded", config={"duration_seconds": 600}, finished=t(502))
attempt(E[1], 1, "succeeded", response={"timeout_until": "2026-01-01T02:00:00Z"})
execution(2, C[2], "remove_timeout", "failed", code="reversal_ownership_conflict", reversal=E[1])
attempt(E[2], 1, "failed", code="reversal_ownership_conflict", response={"error": "x"})
execution(3, C[2], "remove_timeout", "succeeded", reversal=E[1], finished=t(510), config={"requested_by": MOD})
attempt(E[3], 1, "succeeded", response={"result": "timeout_removed", "reversal_noop": True})
execution(4, C[3], "ban_user", "running", config={"delete_message_seconds": 3600})
attempt(E[4], 1, "running", finished=None)
execution(5, C[4], "timeout_user", "pending", config={"duration_seconds": 60})
execution(6, C[6], "timeout_user", "retrying", config={"duration_seconds": 60})
attempt(E[6], 1, "failed", code="timeout_rate_limited")
execution(7, C[7], "kick_user", "failed", code="kick_permission_or_hierarchy_denied", dismissed=t(600), dismissed_by=MOD)
attempt(E[7], 1, "failed", code="kick_permission_or_hierarchy_denied")
execution(8, C[11], "ban_user", "succeeded", finished=t(520))
attempt(E[8], 1, "succeeded", response={"result": "banned", "delete_message_seconds": 0})
execution(9, C[11], "unban_user", "failed", code="reversal_provenance_unavailable", reversal=E[8])
execution(10, C[12], "timeout_user", "failed", code="timeout_network_error", config={"duration_seconds": 600})
attempt(E[10], 1, "failed", code="timeout_network_error")
execution(11, C[13], "send_dm", "succeeded")
attempt(E[11], 1, "succeeded")
execution(12, C[15], "timeout_user", "pending", config={"duration_seconds": 600})
execution(13, C[17], "ban_user", "succeeded", finished=t(530))
attempt(E[13], 1, "succeeded")
execution(14, C[17], "unban_user", "pending", reversal=E[13])
execution(15, C[14], "timeout_user", "pending")       # its case is skipped
attempt(ulid("EX", 98), 1, "failed")                  # orphan attempt

# ---------------------------------------------------------------- DMs
N = [0]


def notification(case_id, status, *, code="", channel="", message="", sent=None):
    N[0] += 1
    ins("case_notifications", id=ulid("NT", N[0]), case_id=case_id, status=status, last_error_code=code,
        prepared_channel_discord_id=channel, delivery_message_discord_id=message, sent_at=sent)


notification(C[1], "sent", channel=CH[10], message="700000000000000001", sent=t(700))
notification(C[2], "sent", channel=CH[10])
notification(C[3], "sending", channel=CH[10])
notification(C[4], "pending")
notification(C[5], "claimed")
notification(C[6], "failed", code="dm_permission_or_hierarchy_denied")
notification(C[7], "failed", code="case_voided")
notification(C[8], "prepared")
notification(C[12], "failed", code="dm_prepare_failed")
notification(C[13], "sent", message="700000000000000013", sent=t(713))  # Go never kept the DM channel
notification(C[14], "pending")                          # its case is skipped
notification(ulid("CS", 97), "pending")                 # orphan

# ---------------------------------------------------------------- evidence
S = {i: ulid("SN", i) for i in range(1, 12)}
msg = "500000000000000001"
embeds = [{"type": "link", "title": f"t{i}", "url": "https://example.com"} for i in range(12)]
ins("case_evidence_snapshots", id=S[1], case_id=C[2], guild_id=GA, channel_discord_id=CH[3], message_discord_id=msg,
    author_discord_user_id=M[1], content="x" * 4100, message_created_at=t(50), embeds_json=embeds)
ins("case_evidence_snapshots", id=S[2], case_id=C[2], guild_id=GA, channel_discord_id=CH[3], message_discord_id=msg,
    author_discord_user_id=M[1], content="dup", message_created_at=t(50))
ins("case_evidence_snapshots", id=S[3], case_id=C[2], guild_id=GA, channel_discord_id=CH[4], message_discord_id="800000000000000001",
    author_discord_user_id=MOD, content="", message_created_at=t(51), capture_outcome="uploaded")
ins("case_evidence_snapshots", id=S[4], case_id=C[3], guild_id=GA, channel_discord_id=CH[3], message_discord_id="500000000000000002",
    author_discord_user_id=M[2], content="", message_created_at=t(52), capture_outcome="deleted")
ins("case_evidence_snapshots", id=S[5], case_id=C[3], guild_id=GA, channel_discord_id=CH[3], message_discord_id="500000000000000003",
    author_discord_user_id="", content="", message_created_at="1000-01-01 00:00:00.000", capture_outcome="unavailable")
ins("case_evidence_snapshots", id=S[6], case_id=C[3], guild_id=GA, channel_discord_id=CH[3], message_discord_id="500000000000000004",
    author_discord_user_id=M[2], content="ctrl\x07char", message_created_at=t(53), message_edited_at=t(54), capture_outcome="inaccessible")
ins("case_evidence_snapshots", id=S[7], case_id=C[14], guild_id=GA, channel_discord_id=CH[3], message_discord_id="500000000000000005",
    author_discord_user_id=M[2], message_created_at=t(55))
ins("case_evidence_snapshots", id=S[8], case_id=C[3], guild_id=GA, channel_discord_id="", message_discord_id="500000000000000006",
    author_discord_user_id=M[2], message_created_at=t(56))
A = [0]


def attachment(evidence, url_attachment, *, outcome="metadata_only", preserved=None, minute=None):
    A[0] += 1
    url = f"https://cdn.discordapp.com/attachments/{CH[3]}/{url_attachment}/f{A[0]}.png?ex=1"
    p_url, p_msg, p_att = "", "", ""
    if preserved:
        # Go stored the copy's jump link, not a CDN URL.
        p_chan, p_msg, p_att, link_msg = preserved
        p_url = f"https://discord.com/channels/{SA}/{p_chan}/{link_msg}"
    ins("case_evidence_attachments", id=ulid("AA", A[0]), evidence_id=evidence, filename=f"f{A[0]}.png", original_url=url,
        preserved_url=p_url, preserved_message_discord_id=p_msg, preserved_attachment_discord_id=p_att, copy_outcome=outcome,
        created_at=t(900 + A[0]))


attachment(S[1], "900000000000000001", outcome="preserved", preserved=(CH[5], "910000000000000001", "920000000000000001", "910000000000000001"))
attachment(S[1], "900000000000000002")
attachment(S[1], "900000000000000003", outcome="preserved", preserved=(CH[5], "910000000000000002", "920000000000000002", "910000000000000099"))
attachment(S[2], "900000000000000004")                  # merged into S1
attachment(S[2], "900000000000000001")                  # same attachment as the first: duplicate
for i in range(11):
    attachment(S[3], f"9300000000000000{i:02d}")
attachment(ulid("SN", 97), "900000000000000009")        # orphan
# A file a moderator uploaded to the command: no source channel, an
# ephemeral CDN URL, and the file's ID as the message ID (as Go wrote it).
ins("case_evidence_snapshots", id=S[9], case_id=C[4], guild_id=GA, channel_discord_id="", message_discord_id="940000000000000001",
    author_discord_user_id=MOD, content="", message_created_at=t(57), capture_outcome="uploaded")
A[0] += 1
ins("case_evidence_attachments", id=ulid("AA", A[0]), evidence_id=S[9], filename="upload.png",
    original_url=f"https://cdn.discordapp.com/ephemeral-attachments/{CH[13]}/940000000000000001/upload.png?ex=1",
    preserved_url=f"https://discord.com/channels/{SA}/{CH[5]}/910000000000000009", preserved_message_discord_id="910000000000000009",
    preserved_attachment_discord_id="920000000000000009", copy_outcome="preserved", created_at=t(990))

ins("case_events", id=ulid("CE", 1), case_id=C[1], guild_id=GA, event_type="case_created", actor_type="staff")
ins("case_events", id=ulid("CE", 2), case_id=C[2], guild_id=GA, event_type="action_succeeded")
ins("case_events", id=ulid("CE", 3), case_id=C[5], guild_id=GA, event_type="case_voided", metadata_json={"reason": "", "replacement_case_id": None})

# ---------------------------------------------------------------- receipts
ins("case_publications", message_id="610000000000000001", case_id=C[1], channel_id=CH[6], created_at=t(1000), retry_at=t(1000))
ins("case_publications", message_id="610000000000000002", case_id=C[1], channel_id=CH[6], created_at=t(1001), retry_at=t(1001),
    presentation_json=json.dumps({"view": "case_receipt", "discord_guild_id": SA}))
ins("case_publications", message_id="610000000000000003", case_id=C[2], channel_id=CH[6], refresh_requested=True, retry_at=t(1002))
ins("case_publications", message_id="610000000000000004", case_id=C[14], channel_id=CH[6], retry_at=t(1003))
ins("case_publications", message_id="610000000000000005", case_id=C[3], channel_id=CH[6], retry_at=t(1004),
    presentation_json=json.dumps({"view": "case_receipt", "discord_guild_id": SB}))

# ---------------------------------------------------------------- appeals
AP = {i: ulid("AP", i) for i in range(1, 10)}
AE = {i: ulid("AE", i) for i in range(1, 20)}


def appeal(i, case_id, member, status, *, statement="please", reason=None, reviewed_by="", review_message="", minute=1100):
    ins("appeals", id=AP[i], guild_id=GA, case_id=case_id, target_discord_user_id=member, status=status, statement=statement,
        version=2, decision_reason=reason, reviewed_by_discord_user_id=reviewed_by, reviewed_at=t(minute + 5) if reviewed_by else None,
        review_message_discord_id=review_message, created_at=t(minute), updated_at=t(minute + 6))


def event(i, appeal_id, kind, *, actor="", actor_type="member", body="text", metadata=None, minute=1200):
    ins("appeal_events", id=AE[i], appeal_id=appeal_id, guild_id=GA, event_type=kind, actor_discord_user_id=actor,
        actor_type=actor_type, body=body, metadata_json=metadata or {}, created_at=t(minute + i))


AN = [0]


def appeal_notification(appeal_id, event_id, target, audience, status, *, code="", channel="", message="", refresh=False):
    AN[0] += 1
    ins("appeal_notifications", id=ulid("AN", AN[0]), appeal_id=appeal_id, event_id=event_id, guild_id=GA, target_discord_user_id=target,
        audience=audience, status=status, last_error_code=code, delivery_channel_id=channel, delivery_message_id=message,
        refresh_requested=refresh, decision_intent_json="" if audience == "staff" else '{"version":1}')


appeal(1, C[5], M[4], "accepted", reason="ok", reviewed_by=MOD)
event(1, AP[1], "submitted", actor=M[4])
event(2, AP[1], "accepted", actor=MOD, actor_type="staff", body="ok")
appeal_notification(AP[1], AE[2], M[4], "member", "sent", channel=CH[11], message="710000000000000001")
appeal_notification(AP[1], AE[1], M[4], "staff", "sent", channel=CH[2], message="720000000000000001")
appeal(2, C[2], M[1], "pending")
event(3, AP[2], "submitted", actor=M[1])
appeal_notification(AP[2], AE[3], M[1], "staff", "sent", channel=CH[2], message="720000000000000002")
event(12, AP[2], "information_requested", actor=MOD, actor_type="staff")
appeal_notification(AP[2], AE[12], M[1], "staff", "failed", code="delivery_deferred")  # never delivered
appeal(3, C[1], M[1], "needs_information")
event(4, AP[3], "submitted", actor=M[1])
event(5, AP[3], "information_requested", actor=MOD, actor_type="staff", body="more?")
event(6, AP[3], "information_submitted", actor=M[1], body="")
appeal_notification(AP[3], AE[5], M[1], "member", "failed", code="delivery_deferred")
appeal(4, None, M[1], "pending")
appeal(5, C[3], M[9], "pending")                       # not the case's member
appeal(6, C[6], M[3], "rejected", statement="s" * 4100, reason="no", reviewed_by=MOD, review_message="720000000000000006")
event(7, AP[6], "submitted", actor=M[3])
event(8, AP[6], "rejected", actor=MOD, actor_type="staff")
event(9, AP[6], "teleported")                          # unknown type
appeal(7, C[12], M[8], "closed", statement="")
event(10, AP[7], "closed", actor="", actor_type="staff", metadata={"cause": "case_voided"})
appeal_notification(AP[7], AE[10], M[8], "member", "sending")
event(11, AP[2], "reopened", actor=MOD, actor_type="staff")
appeal_notification(AP[2], AE[11], M[2], "member", "pending")  # wrong member

# ---------------------------------------------------------------- moderation log
L = [0]


def audit(action, rtype, rid, *, result="success", metadata=None, actor=MOD, failure=None, guild=GA):
    L[0] += 1
    ins("audit_log_entries", id=ulid("AD", L[0]), guild_id=guild, actor_discord_user_id=actor, action=action, resource_type=rtype,
        resource_id=rid, result=result, metadata_json=metadata or {}, failure_reason=failure, actor_permission_bits=18446744073709551615,
        correlation_id="c", request_id="r")
    return ulid("AD", L[0])


audit("case.create", "case", C[1], metadata={"case_id": C[1], "case_number": 1, "target_discord_user_id": M[1]})
audit("appeal.read", "appeal", AP[2], metadata={"case_id": C[1]})                       # case disagrees with the appeal
audit("case_action.succeeded", "case_action_execution", E[1], metadata={"case_id": C[2], "reversal_noop": False})
audit("case_template.update", "case_template", T[1])
audit("ticket.open", "ticket", ulid("TK", 1))
audit("member_case.read", "member", M[3], actor="")
audit("case.read", "case", "unknown", failure="Authorization: Bearer secret")
audit("case.void", "case", C[14], result="denied")                                    # case not imported
audit("case.read", "case", C[2], result="maybe")                                     # unknown result
audit("case.read", "case", C[2], metadata={"target_discord_user_id": M[9]})           # member disagrees
ins("audit_mirror_deliveries", audit_entry_id=ulid("AD", 1), guild_id=GA, status="delivered", next_attempt_at=t(2000))
ins("audit_mirror_deliveries", audit_entry_id=ulid("AD", 2), guild_id=GA, status="pending", next_attempt_at=t(2000))

# ---------------------------------------------------------------- modules
LEGACY_WARNING = ("# Warning!\nThis channel catches spam and scam accounts. Do not post here. "
                  "Posting here triggers this server's honeypot moderation rule.")
ins("module_configurations", id=ulid("MC", 1), guild_id=GA, module_id="tickets",
    config_json={"entry_channel_discord_id": CH[7], "queue_channel_discord_id": CH[8], "transcript_retention_days": 30,
                 "entry_panel_channel_id": CH[7], "entry_panel_message_id": "730000000000000001"})
ins("module_configurations", id=ulid("MC", 2), guild_id=GA, module_id="general_logging",
    config_json={"channels": {"message_edit": CH[9], "member_join": CH[9], "discord_ban": "bad", "mystery": CH[9]},
                 "include_message_content": False, "include_attachment_metadata": True, "include_embed_metadata": False,
                 "cache_entries_per_guild": 1000, "max_delivery_attempts": 3})
ins("module_configurations", id=ulid("MC", 3), guild_id=GA, module_id="honeypots",
    config_json={"channel_discord_id": CH[12], "template_id": T[7], "warning_text": LEGACY_WARNING, "warning_message_id": "740000000000000001"})
ins("module_configurations", id=ulid("MC", 4), guild_id=GB, module_id="tickets",
    config_json={"entry_channel_discord_id": CH[7], "queue_channel_discord_id": "", "transcript_retention_days": 900})
ins("module_configurations", id=ulid("MC", 5), guild_id=GB, module_id="honeypots",
    config_json={"channel_discord_id": CH[12], "template_id": T[10], "disabled_reason": "configured honeypot channel was deleted"})
ins("module_configurations", id=ulid("MC", 6), guild_id=GB, module_id="general_logging", config_json={"channels": None})

# ---------------------------------------------------------------- tickets
TK = {i: ulid("TK", i) for i in range(1, 10)}


def ticket(i, owner, status, *, thread=None, resolved_by=None, resolved=None, metadata=None, log=None, created=True, queue=None):
    ins("tickets", id=TK[i], guild_id=GA, owner_discord_user_id=owner, thread_discord_channel_id=thread, status=status,
        resolved_by_discord_user_id=resolved_by, resolved_at=resolved, metadata_json=metadata or {},
        log_channel_discord_id=log[0] if log else None, log_message_discord_id=log[1] if log else None,
        queue_delivery_attempt_id=queue, created_at=t(1500 + i) if created else None, updated_at=t(1510 + i))


ticket(1, M[1], "resolved", thread="810000000000000001", resolved_by=MOD, resolved=t(1520),
       metadata={"close_notice": {"state": "sent", "message_id": "750000000000000001"}}, log=(CH[8], "760000000000000001"))
ticket(2, M[2], "open", thread="810000000000000002")
ticket(3, M[2], "open", thread="810000000000000003")    # older live ticket of the same member
ticket(4, M[3], "open")                                # no thread
ticket(5, M[4], "cancelled", thread="810000000000000005", metadata={"close_notice": {"state": "rejected"}}, created=False)
ticket(6, M[5], "resolved", thread="810000000000000006", queue=ulid("QA", 1))
ins("ticket_events", id=ulid("TE", 1), ticket_id=TK[1], guild_id=GA, event_type="opened")
ins("ticket_events", id=ulid("TE", 2), ticket_id=TK[1], guild_id=GA, event_type="resolved")
ins("ticket_member_states", id=ulid("TM", 1), guild_id=GA, owner_discord_user_id=M[6], open_ticket_id=ulid("TK", 98), window_started_at=t(1600))
ins("ticket_message_journal", guild_id=GA, thread_discord_channel_id="810000000000000001", message_discord_id="820000000000000001",
    ticket_id=TK[1], author_discord_user_id=M[1], author_name=None, body="hello", sent_at=t(1501))
ins("ticket_message_journal", guild_id=GA, thread_discord_channel_id="810000000000000001", message_discord_id="820000000000000002",
    ticket_id=TK[1], author_discord_user_id=MOD, author_name="Mod", body="", sent_at=t(1502), expires_at=t(99999))
ins("ticket_message_journal", guild_id=GA, thread_discord_channel_id="899999999999999999", message_discord_id="820000000000000003",
    ticket_id=TK[1], author_discord_user_id=MOD, author_name="Mod", body="wrong thread", sent_at=t(1503))
ins("ticket_transcripts", ticket_id=TK[1], guild_id=GA, content="transcript", captured_at=t(1520), expires_at="2099-01-01 00:00:00.000")
ins("ticket_transcripts", ticket_id=TK[5], guild_id=GA, content="old", captured_at=t(1520), expires_at=t(1530))

# ---------------------------------------------------------------- honeypot
HT = {i: ulid("HT", i) for i in range(1, 10)}


def trigger(i, message, member, outcome, *, case_id=None, code=None):
    ins("honeypot_triggers", id=HT[i], guild_id=GA, channel_discord_id=CH[12], message_discord_id=message, target_discord_user_id=member,
        template_id=T[7], case_id=case_id, outcome=outcome, failure_code=code)


trigger(1, "500000000000000098", M[7], "created", case_id=C[11])
trigger(2, "500000000000000097", M[8], "failed", code="case_application_failed")
trigger(3, "500000000000000096", M[9], "exempt", code="")
trigger(4, "500000000000000099", M[7], "pending")
trigger(5, "500000000000000095", M[10], "pending")
ins("honeypot_message_cleanups", id=ulid("HC", 1), guild_id=GA, message_discord_id="500000000000000098", channel_discord_id=CH[12],
    target_discord_user_id=M[7], trigger_id=HT[1], next_attempt_at=t(3000), completed_at=None, created_at=t(3000))
ins("honeypot_message_cleanups", id=ulid("HC", 2), guild_id=GA, message_discord_id="500000000000000097", channel_discord_id=CH[12],
    target_discord_user_id=M[8], trigger_id=HT[2], next_attempt_at=t(3000), completed_at=t(3001), created_at=t(3000))
ins("honeypot_warning_refreshes", guild_id=GA, revision=ulid("HW", 1), pending=True, next_attempt_at=t(3000), send_identity="")

ins("v4_import_batches", id="b" * 64, guild_id=GA, source_name="v4", checksum="c" * 64, actor_discord_user_id=OWNER,
    record_count=1, created_count=1, already_imported_count=0, warning_count=0)
ins("v4_import_sources", id=ulid("VS", 1), batch_id="b" * 64, guild_id=GA, source_name="v4", source_id="abc",
    source_case_number=7, target_case_id=C[10], fingerprint="d" * 64)

with open(f"{OUT}/data.sql", "w") as f:
    f.write("-- Generated fixture: every Go table, enum value, and nasty case the importer handles.\n")
    f.write(";\n".join(STATEMENTS) + ";\n")

# ---------------------------------------------------------------- broken
STATEMENTS.clear()
GC = ulid("GD", 3)
ins("guilds", id=GC, discord_guild_id="not-a-snowflake", name="Broken", owner_discord_user_id=OWNER)
ins("cases", id=ulid("CS", 50), guild_id=GC, case_number=1, template_version=1, template_snapshot_json={}, target_discord_user_id=M[1],
    moderator_discord_user_id=MOD)
# An appeal of server B on a case of server A.
ins("appeals", id=ulid("AP", 50), guild_id=GB, case_id=C[13], target_discord_user_id=M[9], status="pending")
with open(f"{OUT}/broken.sql", "w") as f:
    f.write("-- Generated fixture: a server with an invalid ID and a cross-server link.\n")
    f.write(";\n".join(STATEMENTS) + ";\n")
