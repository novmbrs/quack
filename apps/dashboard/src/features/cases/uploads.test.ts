import { describe, expect, it } from "vite-plus/test";

import {
  MAX_FILE_BYTES,
  copyable,
  fileProgress,
  noteLength,
  refusal,
  sameFile,
  uploadProblem,
} from "./uploads";

const file = (name: string, size: number, type: string) => ({ name, size, type });

describe("refusal", () => {
  it("takes the types and sizes the server copies", () => {
    expect(refusal(file("a.png", 10, "image/png"))).toBeNull();
    expect(refusal(file("a.mp4", MAX_FILE_BYTES, "video/mp4"))).toBeNull();
    expect(refusal(file("a.txt", 1, "text/plain;charset=utf-8"))).toBeNull();
    expect(refusal(file("a.pdf", 1, "application/pdf"))).toBeNull();
    expect(refusal(file("a.ogg", 1, "Audio/Ogg"))).toBeNull();
  });
  it("refuses what the server refuses, size first", () => {
    expect(refusal(file("a.png", MAX_FILE_BYTES + 1, "image/png"))).toBe("too_large");
    expect(refusal(file("a.exe", MAX_FILE_BYTES + 1, "application/x-msdownload"))).toBe(
      "too_large",
    );
    expect(refusal(file("a.html", 1, "text/html"))).toBe("unsupported_type");
    // Browsers send no type for files they don't know; the server sees
    // application/octet-stream and refuses it.
    expect(refusal(file("a.log", 1, ""))).toBe("unsupported_type");
    expect(copyable("application/pdf; x=1")).toBe(true);
  });
});

describe("uploadProblem", () => {
  const png = file("a.png", 1, "image/png");
  it("needs 1–10 files Quack copies and a short note", () => {
    expect(uploadProblem([png], "")).toBeNull();
    expect(uploadProblem([], "")).toMatch(/at least one file/);
    expect(
      uploadProblem(
        Array.from({ length: 11 }, () => png),
        "",
      ),
    ).toMatch(/at most 10/);
    expect(uploadProblem([png, file("a.zip", 1, "application/zip")], "")).toMatch(/can't save/);
    expect(uploadProblem([png], "n".repeat(501))).toMatch(/too long/);
    expect(uploadProblem([png], ` ${"n".repeat(500)} `)).toBeNull();
  });
  it("counts a note as the server does", () => {
    expect(noteLength("  🦆🦆  ")).toBe(2);
  });
});

describe("fileProgress", () => {
  it("fills files in the order they're sent", () => {
    expect(fileProgress([100, 300], 0, 400)).toEqual([0, 0]);
    expect(fileProgress([100, 300], 200, 400)).toEqual([1, 1 / 3]);
    expect(fileProgress([100, 300], 400, 400)).toEqual([1, 1]);
  });
  it("counts the multipart framing against the files", () => {
    // 500 bytes sent of 500 with 100 bytes of framing: everything is sent.
    expect(fileProgress([200, 200], 500, 500)).toEqual([1, 1]);
    expect(fileProgress([0], 10, 10)).toEqual([1]);
    expect(fileProgress([5], 0, 0)).toEqual([0]);
  });
});

describe("sameFile", () => {
  it("spots a file staged twice", () => {
    const a = new File(["x"], "a.png", { type: "image/png", lastModified: 1 });
    const b = new File(["x"], "a.png", { type: "image/png", lastModified: 1 });
    const c = new File(["xy"], "a.png", { type: "image/png", lastModified: 1 });
    expect(sameFile(a, b)).toBe(true);
    expect(sameFile(a, c)).toBe(false);
  });
});
