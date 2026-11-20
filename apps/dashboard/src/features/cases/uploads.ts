// Uploading files as evidence (design §7.1, §7.3, §14.2 route 60). These
// checks mirror the server's so the case page can say what's wrong before
// sending anything; the server checks again and stays the authority.

/** MAX_FILES is the most files one upload takes (one evidence item). */
export const MAX_FILES = 10;

/** MAX_FILE_BYTES is the largest file Quack copies (10 MiB). */
export const MAX_FILE_BYTES = 10_485_760;

/** NOTE_MAX is the longest staff note, in characters. */
export const NOTE_MAX = 500;

/**
 * noteLength counts a note the way the server does: Unicode code points of
 * the trimmed text.
 */
export function noteLength(note: string): number {
  return Array.from(note.trim()).length;
}

/** ACCEPT is the file picker's filter: the types Quack copies. */
export const ACCEPT = "image/*,video/*,audio/*,text/plain,application/pdf";

/** Offered is what the checks need to know about one file. */
export type Offered = { name: string; size: number; type: string };

/** Refusal is why Quack won't take a file, as the server says it. */
export type Refusal = "too_large" | "unsupported_type";

/** copyable says whether Quack copies a file of this content type. */
export function copyable(type: string): boolean {
  const essence = type.split(";")[0]!.trim().toLowerCase();
  return (
    essence.startsWith("image/") ||
    essence.startsWith("video/") ||
    essence.startsWith("audio/") ||
    essence === "text/plain" ||
    essence === "application/pdf"
  );
}

/** refusal is why Quack would refuse one file, or null when it takes it. */
export function refusal(file: Offered): Refusal | null {
  if (file.size > MAX_FILE_BYTES) return "too_large";
  if (!copyable(file.type)) return "unsupported_type";
  return null;
}

/**
 * refusalText says why a file can't be uploaded, in the words of the
 * server's error.files_not_allowed.
 */
export function refusalText(why: Refusal): string {
  return why === "too_large" ? "Larger than 10 MB." : "A file type Quack doesn't copy.";
}

/** SUPPORTED says what Quack copies, for hints. */
export const SUPPORTED = "Images, videos, audio, plain text, and PDFs up to 10 MB each.";

/**
 * uploadProblem is what stops the staged files from being sent, or null:
 * nothing staged, more than 10, a file Quack won't copy, or a note that's
 * too long.
 */
export function uploadProblem(files: Offered[], note: string): string | null {
  if (files.length === 0) return "Add at least one file.";
  if (files.length > MAX_FILES) return `Add at most ${MAX_FILES} files at a time.`;
  if (files.some((f) => refusal(f) !== null))
    return "Remove the files Quack can't save, then upload.";
  if (noteLength(note) > NOTE_MAX)
    return `This note is too long. Shorten it to ${NOTE_MAX} characters or fewer.`;
  return null;
}

/**
 * fileProgress splits an upload's sent bytes across its files in the order
 * they're sent, as fractions from 0 to 1. The multipart framing is small,
 * so it's counted against the files.
 */
export function fileProgress(sizes: number[], loaded: number, total: number): number[] {
  const content = sizes.reduce((a, b) => a + b, 0);
  // Scale the bytes sent to the files' share of the body.
  let sent = total > 0 ? (loaded / total) * content : 0;
  return sizes.map((size) => {
    if (size === 0) return loaded >= total ? 1 : 0;
    const done = Math.min(Math.max(sent, 0), size);
    sent -= size;
    return done / size;
  });
}

/**
 * pastedFiles takes the files out of a paste, or nothing when the paste
 * is only text (so pasting into a text box works as usual).
 */
export function pastedFiles(data: DataTransfer | null): File[] {
  if (!data) return [];
  return Array.from(data.files ?? []);
}

/** hasFiles says whether a drag carries files (not text or a link). */
export function hasFiles(data: DataTransfer | null): boolean {
  return Boolean(data && Array.from(data.types ?? []).includes("Files"));
}
