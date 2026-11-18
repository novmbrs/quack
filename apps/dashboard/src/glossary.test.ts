import { readdirSync, readFileSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";

import ts from "typescript";
import { describe, expect, it } from "vite-plus/test";

// The glossary lint. Quack says one thing one way on every surface (charter
// §3, design §1.3), so words the glossary retired must not come back in
// anything a person reads on the dashboard. This scans every string literal,
// template, and JSX text in src and fails on a banned word. Code identifiers,
// comments, types, and comparisons aren't scanned.

const root = fileURLToPath(new URL(".", import.meta.url));

/** banned maps each retired word to the glossary word to use instead. */
const banned: [RegExp, string][] = [
  [/\btemplates?\b/i, "rule"],
  [/\bpolic(y|ies)\b/i, "rule (or rule file)"],
  [/\binfractions?\b/i, "case"],
  [/\bpunish(ment|ments|ed|es)?\b/i, "action"],
  [/\benforce(ment|ments|d|s)?\b/i, "action"],
  [/\bexecutions?\b/i, "action or attempt"],
  [/\bnotifications?\b/i, "DM"],
  [/\bnotices?\b/i, "DM or reminder"],
  [/\baudit[- ]log\b/i, "moderation log"],
  [/\baudit mirror\b/i, "log channel"],
  [/\bgeneral logging\b/i, "server activity log"],
  [/\boffenders?\b/i, "member"],
  [/\binfringe/i, "case"],
  [/\busers?\b/i, "member (or Discord account)"],
];

/**
 * allowed lists reviewed uses: Discord's own UI names, which Quack quotes
 * so people can find them, and words that mean something else there.
 * Each entry matches the whole string (trimmed) in one file.
 */
const allowed: { file: string; text: RegExp; why: string }[] = [
  {
    file: "*",
    text: /User Settings/,
    why: "Discord's settings menu (User Settings → My Account) is quoted by name.",
  },
  {
    file: "*",
    text: /Server Settings → Audit Log|View Audit Log/,
    why: "Discord's own Audit Log and its View Audit Log permission, not Quack's moderation log.",
  },
];

/** files lists the source files people's strings live in. */
function files(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return files(path);
    if (!/\.tsx?$/.test(entry.name)) return [];
    if (/\.(test|gen)\.tsx?$/.test(entry.name)) return [];
    return [path];
  });
}

/** skip reports nodes whose strings are code, not text: imports, types, comparisons, keys. */
function skip(node: ts.Node): boolean {
  const parent = node.parent;
  if (!parent) return false;
  if (ts.isImportDeclaration(parent) || ts.isExportDeclaration(parent)) return true;
  if (ts.isLiteralTypeNode(parent)) return true;
  if (ts.isPropertyAssignment(parent) && parent.name === node) return true;
  if (ts.isPropertySignature(parent) && parent.name === node) return true;
  if (ts.isElementAccessExpression(parent)) return true;
  if (ts.isCaseClause(parent)) return true;
  if (
    ts.isBinaryExpression(parent) &&
    [
      ts.SyntaxKind.EqualsEqualsEqualsToken,
      ts.SyntaxKind.ExclamationEqualsEqualsToken,
      ts.SyntaxKind.EqualsEqualsToken,
      ts.SyntaxKind.ExclamationEqualsToken,
    ].includes(parent.operatorToken.kind)
  )
    return true;
  // Class names, keys, and paths in JSX attributes aren't read by people.
  if (ts.isJsxAttribute(parent)) {
    const name = parent.name.getText();
    return !["title", "label", "aria-label", "placeholder", "alt", "description", "topic", "hint"].includes(
      name,
    );
  }
  return false;
}

type Hit = { file: string; line: number; text: string; word: string; use: string };

function scan(path: string): Hit[] {
  const source = ts.createSourceFile(
    path,
    readFileSync(path, "utf8"),
    ts.ScriptTarget.Latest,
    true,
    path.endsWith("x") ? ts.ScriptKind.TSX : ts.ScriptKind.TS,
  );
  const file = relative(root, path);
  const hits: Hit[] = [];
  const check = (node: ts.Node, text: string) => {
    const trimmed = text.replace(/\s+/g, " ").trim();
    // API paths are the contract's names, not text.
    if (!trimmed || trimmed.startsWith("/api/")) return;
    for (const [pattern, use] of banned) {
      const match = pattern.exec(trimmed);
      if (!match) continue;
      const ok = allowed.some(
        (a) => (a.file === "*" || a.file === file) && a.text.test(trimmed),
      );
      if (ok) continue;
      const { line } = source.getLineAndCharacterOfPosition(node.getStart());
      hits.push({ file, line: line + 1, text: trimmed, word: match[0], use });
    }
  };
  const visit = (node: ts.Node) => {
    if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) {
      if (!skip(node)) check(node, node.text);
    } else if (ts.isTemplateExpression(node)) {
      if (!skip(node)) {
        check(node, [node.head.text, ...node.templateSpans.map((s) => s.literal.text)].join(" … "));
      }
    } else if (ts.isJsxText(node)) {
      check(node, node.text);
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
  return hits;
}

describe("glossary", () => {
  it("keeps retired words out of everything people read", () => {
    const hits = files(root).flatMap(scan);
    const report = hits.map(
      (h) => `${h.file}:${h.line} says "${h.word}" (use ${h.use}): ${h.text.slice(0, 120)}`,
    );
    expect(report).toEqual([]);
  });

  it("catches a retired word, and only in text", () => {
    const sample = join(root, "glossary.sample.tsx");
    const source = ts.createSourceFile(
      sample,
      `import x from "template";
       type T = "policy";
       if (k === "template") {}
       const a = <p className="notification">Apply the template to the user.</p>;
       const b = "Open User Settings → My Account.";`,
      ts.ScriptTarget.Latest,
      true,
      ts.ScriptKind.TSX,
    );
    const words: string[] = [];
    const visit = (node: ts.Node) => {
      if (ts.isJsxText(node) || ((ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) && !skip(node))) {
        const text = ts.isJsxText(node) ? node.text : node.text;
        for (const [pattern] of banned) {
          const m = pattern.exec(text);
          if (m && !allowed.some((a) => a.text.test(text))) words.push(m[0].toLowerCase());
        }
      }
      ts.forEachChild(node, visit);
    };
    visit(source);
    expect(words.sort()).toEqual(["template", "user"]);
  });
});
