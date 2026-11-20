//! Every engine error mapped to its API error, with exhaustive matches so
//! a new engine variant can't silently become a 500.

use quack_core::DomainError;
use quack_core::action::ActionKind;
use quack_core::case::{ContextProblem, ContextProblemCode, LinkProblem};
use quack_core::modules::{ModuleName, ModuleProblem};
use quack_core::permissions::ChannelProblem;
use quack_core::rule::{Field, RuleFileError, RuleProblem};
use quack_db::DbError;
use quack_engine::PrincipalError;
use quack_engine::actions::ActionError;
use quack_engine::appeals::AppealError;
use quack_engine::cases::{ApplyError, CaseError};
use quack_engine::rules::RuleError;
use quack_engine::settings::{FieldProblem, SettingsError};
use quack_engine::stats::StatsError;
use quack_text::ChannelRef;
use quack_text::api::ApiCode;
use quack_text::error::{ErrorFacts, Scene};
use quack_text::render::plain;

use super::{ApiError, FieldError, Problem};

/// What a handler knows about the thing an error is about, so the message
/// can name it (§14.6 "Where codes come from").
#[derive(Clone, Debug, Default)]
pub(crate) struct Facts {
    /// Where the error happened.
    pub scene: Scene,
    /// The case's number.
    pub case_number: Option<u64>,
    /// The rule's name.
    pub rule: Option<String>,
    /// The action's kind.
    pub action: Option<ActionKind>,
    /// The rule's context fields, for context problem labels.
    pub fields: Vec<Field>,
}

impl Facts {
    /// Facts for a scene with nothing else known.
    pub(crate) fn scene(scene: Scene) -> Self {
        Self {
            scene,
            ..Self::default()
        }
    }

    fn error_facts(&self) -> ErrorFacts<'_> {
        ErrorFacts {
            scene: self.scene,
            case_number: self.case_number,
            rule: self.rule.as_deref(),
            action: self.action,
            member: None,
        }
    }

    /// A domain refusal worded with these facts.
    pub(crate) fn domain(&self, err: DomainError) -> ApiError {
        ApiError::domain(&err, &self.error_facts())
    }
}

/// A database failure: unreachable or contended is `503 unavailable`
/// ("nothing changed" holds: the transaction rolled back); a missing row
/// is `404 not_found`; anything else is `500 internal_error`.
pub(crate) fn from_db(error: &DbError) -> ApiError {
    match error {
        DbError::Unavailable(..) | DbError::Contention(..) => ApiError::api(ApiCode::Unavailable),
        DbError::NotFound => ApiError::not_found(),
        DbError::Conflict(..)
        | DbError::Stale
        | DbError::Rejected { .. }
        | DbError::Invalid(_)
        | DbError::Decode(_)
        | DbError::Migrate(_)
        | DbError::Other(_) => {
            tracing::error!(class = ?error.class(), "database error");
            ApiError::internal()
        }
    }
}

/// A reason (void, appeal decision) that is empty or longer than `max`.
pub(crate) fn reason_problem(field: &str, code: &str, text: &str, max: usize) -> ApiError {
    let message = if text.trim().is_empty() {
        quack_text::error::reason_empty()
    } else {
        quack_text::error::reason_too_long(max)
    };
    ApiError::field(field, code, plain(&message))
}

/// A member's appeal text that is empty or longer than `max`.
fn appeal_text_problem(field: &str, text: &str, max: usize) -> ApiError {
    let message = if text.trim().is_empty() {
        quack_text::error::appeal_text_empty()
    } else {
        quack_text::error::appeal_text_too_long(max)
    };
    ApiError::field(field, "text_length", plain(&message))
}

fn context_fields(problems: &[ContextProblem], fields: &[Field]) -> Vec<FieldError> {
    problems
        .iter()
        .map(|p| {
            let field = fields.iter().find(|f| f.key.as_str() == p.key);
            FieldError {
                field: p.path(),
                code: p.code.as_str().to_owned(),
                message: plain(&quack_text::case::context_problem(
                    p,
                    field.map(|f| f.label.as_str()),
                    field.map(|f| f.kind),
                )),
            }
        })
        .collect()
}

fn link_fields(prefix: &str, problems: &[(usize, LinkProblem)]) -> Vec<FieldError> {
    problems
        .iter()
        .map(|(index, problem)| {
            let code = match problem {
                LinkProblem::NotAMessageLink => ContextProblemCode::NotAMessageLink,
                LinkProblem::OtherServer => ContextProblemCode::OtherServer,
            };
            let as_context = ContextProblem {
                key: format!("{prefix}[{index}]"),
                code,
            };
            FieldError {
                field: as_context.key.clone(),
                code: code.as_str().to_owned(),
                message: plain(&quack_text::case::context_problem(&as_context, None, None)),
            }
        })
        .collect()
}

/// Files Quack won't copy (field `files`, code `too_large` or
/// `unsupported_type`), each with the `error.files_not_allowed` sentence
/// naming it.
fn file_fields(files: &[quack_core::evidence::RefusedFile]) -> Vec<FieldError> {
    files
        .iter()
        .map(|f| FieldError {
            field: "files".to_owned(),
            code: f.why.as_str().to_owned(),
            message: plain(&quack_text::error::files_not_allowed(std::slice::from_ref(
                f,
            ))),
        })
        .collect()
}

fn rule_fields(prefix: &str, problems: &[RuleProblem]) -> Vec<FieldError> {
    problems
        .iter()
        .map(|p| FieldError {
            field: format!("{prefix}{}", p.path()),
            code: p.code().as_str().to_owned(),
            message: plain(&quack_text::rules::problem(p.code())),
        })
        .collect()
}

/// Errors of applying a rule (route 21). `links` names the request's
/// evidence link field.
pub(crate) fn apply(error: ApplyError, facts: &Facts) -> ApiError {
    match error {
        ApplyError::Refused(err) => facts.domain(err),
        ApplyError::ContextInvalid(problems) => {
            ApiError::validation(context_fields(&problems, &facts.fields))
        }
        ApplyError::LinksInvalid(problems) => {
            ApiError::validation(link_fields("evidence_links", &problems))
        }
        ApplyError::FilesRefused(files) => ApiError::validation(file_fields(&files)),
        ApplyError::OperationKeyReused => ApiError::api(ApiCode::IdempotencyKeyReused),
        ApplyError::CaseNumbersExhausted => ApiError::api(ApiCode::CaseNumbersExhausted),
        // Only the honeypot flow can see this; the dashboard never does.
        ApplyError::HoneypotPolicyChanged | ApplyError::Unconfirmed => ApiError::internal(),
        ApplyError::DiscordUnavailable => ApiError::api(ApiCode::DiscordUnavailable),
        ApplyError::Deadline => ApiError::api(ApiCode::Unavailable),
        ApplyError::Db(e) => from_db(&e),
    }
}

/// Errors of voiding, editing context, and adding evidence.
pub(crate) fn case(error: CaseError, facts: &Facts, reason: &str) -> ApiError {
    match error {
        CaseError::Refused(err) => facts.domain(err),
        CaseError::ContextInvalid(problems) => {
            ApiError::validation(context_fields(&problems, &facts.fields))
        }
        CaseError::LinksInvalid(problems) => ApiError::validation(link_fields("links", &problems)),
        CaseError::ContextChanged => ApiError::api(ApiCode::ContextChanged),
        CaseError::NoLinks => {
            ApiError::field("links", "no_links", plain(&quack_text::error::no_links()))
        }
        CaseError::FilesRefused(files) => ApiError::validation(file_fields(&files)),
        CaseError::NoteLength => ApiError::field(
            "note",
            "note_too_long",
            plain(&quack_text::error::note_too_long()),
        ),
        CaseError::DiscordUnavailable => ApiError::api(ApiCode::DiscordUnavailable),
        CaseError::ReasonLength => reason_problem(
            "reason",
            "reason_length",
            reason,
            quack_core::limits::VOID_REASON.max,
        ),
        CaseError::OperationKeyReused => ApiError::api(ApiCode::IdempotencyKeyReused),
        CaseError::Db(e) => from_db(&e),
    }
}

/// Errors of retry, dismiss, and undo.
pub(crate) fn action(error: ActionError, facts: &Facts) -> ApiError {
    match error {
        ActionError::NotFound => ApiError::api(ApiCode::ActionNotFound),
        ActionError::Refused(err) => facts.domain(err),
        ActionError::OperationKeyReused => ApiError::api(ApiCode::IdempotencyKeyReused),
        ActionError::DiscordUnavailable => ApiError::api(ApiCode::DiscordUnavailable),
        ActionError::Db(e) => from_db(&e),
    }
}

/// Errors of appeals; `text_field` names the request's text field
/// (`statement`, `text`) and `text` its value.
pub(crate) fn appeal(error: AppealError, facts: &Facts, text_field: &str, text: &str) -> ApiError {
    match error {
        AppealError::Refused(err) => facts.domain(err),
        AppealError::TextLength => {
            appeal_text_problem(text_field, text, quack_core::limits::APPEAL_TEXT.max)
        }
        AppealError::ReasonLength => reason_problem(
            "reason",
            "reason_length",
            text,
            quack_core::limits::APPEAL_DECISION_REASON.max,
        ),
        AppealError::OperationKeyReused => ApiError::api(ApiCode::IdempotencyKeyReused),
        AppealError::Db(e) => from_db(&e),
    }
}

/// Errors of rule writes and reads. `prefix` is the definition's path in
/// the request body (`""`, `definition.`, or `file.rule.`).
pub(crate) fn rule(error: RuleError, facts: &Facts, prefix: &str) -> ApiError {
    match error {
        RuleError::Refused(err) => facts.domain(err),
        RuleError::Invalid(problems) => ApiError::validation(rule_fields(prefix, &problems)),
        RuleError::File(file) => match file {
            RuleFileError::WrongFormat => ApiError::field(
                "file.format",
                "wrong_format",
                plain(&quack_text::api::rule_file_wrong_format()),
            ),
            RuleFileError::UnsupportedVersion(_) => ApiError::field(
                "file.format_version",
                "unsupported_version",
                plain(&quack_text::api::rule_file_unsupported_version()),
            ),
            RuleFileError::Invalid(problems) => {
                ApiError::validation(rule_fields("file.rule.", &problems))
            }
        },
        RuleError::OperationKeyReused => ApiError::api(ApiCode::IdempotencyKeyReused),
        RuleError::Db(e) => from_db(&e),
    }
}

/// The character limit of a settings field, for `too_long`.
fn settings_max(field: &str) -> usize {
    match field {
        "dm_intro" => quack_core::limits::DM_INTRO.max,
        "dm_footer" => quack_core::limits::DM_FOOTER.max,
        "warning_text" => quack_core::limits::HONEYPOT_WARNING.max,
        _ => quack_core::limits::INVITE_CODE.max,
    }
}

/// The message of one module problem.
pub(crate) fn module_problem(p: &ModuleProblem) -> Problem {
    Problem {
        code: p.code().to_owned(),
        message: plain(&quack_text::setup::module_problem(p, None, None)),
    }
}

/// The message of one channel problem.
pub(crate) fn channel_problem(channel: quack_core::ids::ChannelId, p: &ChannelProblem) -> String {
    plain(&quack_text::setup::channel_problem(
        p,
        &ChannelRef {
            id: channel,
            name: None,
        },
        &[],
        &[],
    ))
}

/// Errors of settings, module switches, and health. `module` names the
/// module a `module_not_ready` is about; `channels` gives the channel each
/// submitted channel field named, for channel problem messages.
pub(crate) fn settings(
    error: SettingsError,
    facts: &Facts,
    module: Option<ModuleName>,
    channels: &[(&str, quack_core::ids::ChannelId)],
) -> ApiError {
    match error {
        SettingsError::Refused(err) => facts.domain(err),
        SettingsError::Invalid(problems) => {
            let mut fields = Vec::new();
            for (field, problem) in problems {
                match problem {
                    FieldProblem::Value(p) => fields.push(FieldError {
                        field: field.to_owned(),
                        code: p.as_str().to_owned(),
                        message: plain(&quack_text::setup::settings_problem(
                            p,
                            settings_max(field),
                        )),
                    }),
                    FieldProblem::Channel(channel_problems) => {
                        let channel = channels.iter().find(|(f, _)| *f == field).map(|c| c.1);
                        for p in channel_problems {
                            fields.push(FieldError {
                                field: field.to_owned(),
                                code: p.code().to_owned(),
                                message: channel.map_or_else(
                                    || plain(&quack_text::api::validation_failed()),
                                    |c| channel_problem(c, &p),
                                ),
                            });
                        }
                    }
                }
            }
            ApiError::validation(fields)
        }
        SettingsError::ModuleNotReady(problems) => match module {
            Some(m) => ApiError::module_not_ready(m, problems.iter().map(module_problem).collect()),
            None => ApiError::internal(),
        },
        SettingsError::OperationKeyReused => ApiError::api(ApiCode::IdempotencyKeyReused),
        SettingsError::DiscordUnavailable => ApiError::api(ApiCode::DiscordUnavailable),
        SettingsError::Db(e) => from_db(&e),
    }
}

/// Errors of statistics.
pub(crate) fn stats(error: StatsError, facts: &Facts) -> ApiError {
    match error {
        StatsError::Refused(err) => facts.domain(err),
        StatsError::RangeInvalid => ApiError::field(
            "from",
            "range",
            plain(&quack_text::api::range_invalid(
                u64::try_from(quack_core::limits::MAX_STATS_RANGE.whole_days()).unwrap_or(366),
            )),
        ),
        StatsError::Db(e) => from_db(&e),
    }
}

/// Errors of building a principal: Quack not in the server reads as an
/// unknown server (it doesn't say which, §12.14).
pub(crate) fn principal(error: PrincipalError) -> ApiError {
    match error {
        PrincipalError::DiscordUnavailable => ApiError::api(ApiCode::DiscordUnavailable),
        PrincipalError::QuackNotInServer => ApiError::api(ApiCode::ServerNotFound),
        PrincipalError::Db(e) => from_db(&e),
    }
}
