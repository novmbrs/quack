import { describe, expect, it } from "vite-plus/test";

import {
  decisionReason,
  decisionsFrom,
  defaultReason,
  filterOf,
  parseStateFilter,
  reasonRequired,
  tabOf,
} from "./decisions";

describe("decisionsFrom", () => {
  it("lets staff decide open appeals", () => {
    expect(decisionsFrom.pending).toEqual(["accept", "reject", "request_information", "close"]);
    expect(decisionsFrom.needs_information).toEqual(["accept", "reject", "close"]);
  });
  it("only reopens decided appeals", () => {
    expect(decisionsFrom.accepted).toEqual(["reopen"]);
    expect(decisionsFrom.rejected).toEqual(["reopen"]);
    expect(decisionsFrom.closed).toEqual(["reopen"]);
  });
});

describe("reasons", () => {
  it("needs written text for anything without a standard reason", () => {
    expect(reasonRequired("request_information")).toBe(true);
    expect(reasonRequired("reopen")).toBe(true);
    expect(reasonRequired("close")).toBe(true);
    expect(reasonRequired("accept")).toBe(false);
    expect(reasonRequired("reject")).toBe(false);
  });
  it("prefers what staff wrote", () => {
    expect(decisionReason("accept", "  Mistaken identity. ")).toBe("Mistaken identity.");
  });
  it("falls back to the reason Discord's buttons send", () => {
    expect(decisionReason("accept", "")).toBe(
      "We reviewed your appeal. The case no longer counts.",
    );
    expect(decisionReason("reject", " ")).toBe("We reviewed your appeal. The case still counts.");
    expect(defaultReason("close")).toBeUndefined();
  });
});

describe("state filters", () => {
  it("defaults to pending", () => {
    expect(parseStateFilter(undefined)).toBe("pending");
    expect(parseStateFilter("nonsense")).toBe("pending");
    expect(parseStateFilter("rejected")).toBe("rejected");
  });
  it("groups decided states under one tab", () => {
    expect(tabOf("accepted")).toBe("decided");
    expect(tabOf("closed")).toBe("decided");
    expect(tabOf("needs_information")).toBe("needs_information");
    expect(filterOf("decided")).toBe("accepted");
    expect(filterOf("all")).toBe("all");
  });
});
