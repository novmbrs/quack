import type { components } from "./schema.gen";

type S = components["schemas"];

// Names for the wire types (design §14.3), so pages don't spell out the
// generated paths.

export type Me = S["Me"];
export type Capabilities = S["Capabilities"];
export type GuildSummary = S["GuildSummary"];
export type GuildMe = S["GuildMe"];
export type Health = S["Health"];
export type Permission = S["Permission"];
export type Statistics = S["Statistics"];
export type Bucket = S["Bucket"];

export type DirectoryMember = S["DirectoryMember"];
export type DirectoryChannel = S["DirectoryChannel"];
export type DirectoryRole = S["DirectoryRole"];
export type ChannelKind = S["ChannelKind"];

export type Rule = S["Rule"];
export type RuleSummary = S["RuleSummary"];
export type RuleDefinition = S["RuleDefinition"];
export type RuleVersion = S["RuleVersion"];
export type RuleFile = S["RuleFile"];
export type Level = S["Level"];
export type LevelAction = S["LevelAction"];
export type Field = S["Field"];
export type FieldKind = S["FieldKind"];

export type CaseSummary = S["CaseSummary"];
export type Case = S["Case"];
export type CaseVoided = S["CaseVoided"];
export type EvidenceAdded = S["EvidenceAdded"];
export type EvidenceEdited = S["EvidenceEdited"];
export type ImportedCase = S["ImportedCase"];
export type CasePreview = S["CasePreview"];
export type CaseSource = S["CaseSource"];
export type CaseLogEntry = S["CaseLogEntry"];
export type ContextInput = S["ContextInput"];
export type ContextValue = S["ContextValue"];
export type Action = S["Action"];
export type ActionListItem = S["ActionListItem"];
export type ActionKind = S["ActionKind"];
export type ActionState = S["ActionState"];
export type Dm = S["Dm"];
export type DmState = S["DmState"];
export type Evidence = S["Evidence"];
export type EvidenceFile = S["EvidenceFile"];
export type EvidenceKind = S["EvidenceKind"];
export type SurroundingMessage = S["SurroundingMessage"];

export type AppealSummary = S["AppealSummary"];
export type Appeal = S["Appeal"];
export type AppealState = S["AppealState"];
export type AppealEvent = S["AppealEvent"];
export type AppealEventKind = S["AppealEventKind"];
export type Decision = S["Decision"];

export type MemberCaseSummary = S["MemberCaseSummary"];
export type MemberCase = S["MemberCase"];
export type MemberSummary = S["MemberSummary"];

export type ModLogEntry = S["ModLogEntry"];
export type ModLogKind = S["ModLogKind"];
export type ModLogResult = S["ModLogResult"];
export type ModLogSource = S["ModLogSource"];

export type Settings = S["Settings"];
export type SettingsChange = S["SettingsChange"];
export type ModuleState = S["ModuleState"];

export type TicketSettings = S["TicketSettings"];
export type TicketsModule = S["TicketsModule"];
export type Ticket = S["Ticket"];
export type TicketState = S["TicketState"];
export type ActivityLogSettings = S["ActivityLogSettings"];
export type ActivityLogModule = S["ActivityLogModule"];
export type HoneypotSettings = S["HoneypotSettings"];
export type HoneypotModule = S["HoneypotModule"];

/** ModuleName is a module as it appears in JSON; its URL segment uses a hyphen. */
export type ModuleName = "tickets" | "activity_log" | "honeypot";

/** moduleSegment is a module's segment in API and dashboard URLs. */
export const moduleSegment = {
  tickets: "tickets",
  activity_log: "activity-log",
  honeypot: "honeypot",
} as const satisfies Record<ModuleName, string>;
