import createClient, { type Middleware } from "openapi-fetch";

import type { components, paths } from "./schema.gen";

/** FieldError is one form field the API refused, with a sentence to show next to it. */
export type FieldError = components["schemas"]["FieldError"];

/** Problem is one thing to fix before a module can be turned on. */
export type Problem = components["schemas"]["Problem"];

/**
 * ApiError is a failed API call. The backend wraps every failure in the same
 * envelope (design §14.6) and its message is always a sentence meant for the
 * reader, so pages show it as is.
 */
export class ApiError extends Error {
  readonly status: number;
  /** The envelope's machine-readable code, or "network_error" when Quack couldn't be reached. */
  readonly code: string;
  /** Form fields the API refused, keyed by their path ("levels[1].from_case_count"). */
  readonly fields: FieldError[];
  /** Module problems (409 module_not_ready). */
  readonly problems: Problem[];
  /** The trace to quote when reporting an unexpected error. */
  readonly traceId: string | null;
  /** Seconds to wait after a 429. */
  readonly retryAfter: number | null;

  constructor(
    status: number,
    code: string,
    message: string,
    extra: {
      fields?: FieldError[];
      problems?: Problem[];
      traceId?: string | null;
      retryAfter?: number | null;
    } = {},
  ) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
    this.fields = extra.fields ?? [];
    this.problems = extra.problems ?? [];
    this.traceId = extra.traceId ?? null;
    this.retryAfter = extra.retryAfter ?? null;
  }

  /** Whether the session is gone and the reader has to sign in again. */
  get signedOut(): boolean {
    return this.status === 401;
  }

  /**
   * Whether this is a failure the reader didn't cause and can't fix, so the
   * page should show the trace ID for a report.
   */
  get unexpected(): boolean {
    return this.status === 0 || this.status >= 500;
  }

  /** field returns the message for one form field, matching its path or any path under it. */
  field(path: string): string | undefined {
    return this.fields.find(
      (f) => f.field === path || f.field.startsWith(`${path}.`) || f.field.startsWith(`${path}[`),
    )?.message;
  }
}

let csrfToken = "";

/**
 * setCsrfToken stores the token from GET /api/auth/me. Every write must echo
 * it in X-CSRF-Token (design §14.5).
 */
export function setCsrfToken(token: string) {
  csrfToken = token;
}

const writeMethods = new Set(["POST", "PUT", "PATCH", "DELETE"]);

const writeHeaders: Middleware = {
  onRequest({ request }) {
    if (!writeMethods.has(request.method)) return request;
    if (csrfToken) request.headers.set("X-CSRF-Token", csrfToken);
    // write() sets the key so automatic retries reuse it; this only covers
    // a write that bypassed it.
    if (!request.headers.has("Idempotency-Key")) {
      request.headers.set("Idempotency-Key", newKey());
    }
    return request;
  },
};

/** api is the typed client for the Quack HTTP API. Paths already start with /api. */
export const api = createClient<paths>({
  baseUrl: "",
  credentials: "same-origin",
  headers: { Accept: "application/json" },
});
api.use(writeHeaders);

type Result<T> = { data?: T; error?: unknown; response: Response };

/**
 * unwrap turns an openapi-fetch result into its data, throwing ApiError for
 * any failure so TanStack Query sees a rejected promise.
 */
export async function unwrap<T>(pending: Promise<Result<T>>): Promise<T> {
  let result: Result<T>;
  try {
    result = await pending;
  } catch {
    throw new ApiError(
      0,
      "network_error",
      "Quack can't be reached right now. Check your connection and try again.",
    );
  }
  const { data, error, response } = result;
  if (response.ok) return data as T;
  throw toApiError(response, error);
}

/** toApiError reads the error envelope of a failed response. */
export function toApiError(response: Response, body: unknown): ApiError {
  const detail = (body as { error?: Partial<components["schemas"]["ErrorBody"]> } | undefined)
    ?.error;
  const retryHeader = Number.parseInt(response.headers.get("Retry-After") ?? "", 10);
  return new ApiError(
    response.status,
    detail?.code ?? (response.status >= 500 ? "internal_error" : "unknown"),
    detail?.message?.trim() || fallbackMessage(response.status),
    {
      fields: detail?.fields,
      problems: detail?.problems,
      traceId: detail?.trace_id ?? null,
      retryAfter:
        detail?.retry_after_seconds ?? (Number.isFinite(retryHeader) ? retryHeader : null),
    },
  );
}

function fallbackMessage(status: number): string {
  if (status === 401) return "Your session ended. Sign in again to continue.";
  if (status === 429) return "That was a lot of requests at once. Wait a moment and try again.";
  if (status >= 500) return "Quack couldn't finish this. Try again in a moment.";
  return "Quack couldn't do that. Reload the page and try again.";
}

/** newKey makes one Idempotency-Key: one per thing the reader asked for. */
export function newKey(): string {
  return crypto.randomUUID();
}

/** WriteInit is what write() passes to the openapi-fetch call it wraps. */
export type WriteInit = { headers: { "Idempotency-Key": string } };

/** Statuses worth retrying automatically: Quack or Discord was briefly unavailable. */
const retryable = (e: ApiError) => e.status === 0 || e.status === 502 || e.status === 503;

/**
 * write sends one write the reader asked for. It makes one Idempotency-Key
 * and reuses it on automatic retries (network errors, 502, 503), so a retry
 * never applies a change twice. Each call is a new intent with a new key.
 */
export async function write<T>(
  call: (init: WriteInit) => Promise<Result<T>>,
  options: { key?: string; retries?: number; delay?: (attempt: number) => number } = {},
): Promise<T> {
  const key = options.key ?? newKey();
  const retries = options.retries ?? 2;
  const delay = options.delay ?? ((attempt) => 400 * 2 ** attempt);
  for (let attempt = 0; ; attempt++) {
    try {
      return await unwrap(call({ headers: { "Idempotency-Key": key } }));
    } catch (error) {
      if (!(error instanceof ApiError) || !retryable(error) || attempt >= retries) throw error;
      await new Promise((resolve) => setTimeout(resolve, delay(attempt)));
    }
  }
}

/** UploadProgress is how much of an upload's body the browser has sent. */
export type UploadProgress = { loaded: number; total: number };

/**
 * postForm sends a multipart form the way openapi-fetch sends JSON (CSRF
 * token, Idempotency-Key, same origin), but over XMLHttpRequest, because
 * fetch can't report upload progress. It settles like openapi-fetch: a
 * network failure rejects, any answer resolves with its parsed body.
 */
function postForm<T>(
  url: string,
  form: FormData,
  init: WriteInit,
  onProgress?: (p: UploadProgress) => void,
): Promise<Result<T>> {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open("POST", url);
    xhr.withCredentials = true;
    xhr.setRequestHeader("Accept", "application/json");
    if (csrfToken) xhr.setRequestHeader("X-CSRF-Token", csrfToken);
    xhr.setRequestHeader("Idempotency-Key", init.headers["Idempotency-Key"]);
    if (onProgress) {
      xhr.upload.onprogress = (e) => {
        if (e.lengthComputable) onProgress({ loaded: e.loaded, total: e.total });
      };
    }
    xhr.onerror = () => reject(new TypeError("upload failed"));
    xhr.onabort = () => reject(new TypeError("upload aborted"));
    xhr.onload = () => {
      let body: unknown;
      try {
        body = xhr.responseText ? JSON.parse(xhr.responseText) : undefined;
      } catch {
        body = undefined;
      }
      const headers = new Headers();
      const retryAfter = xhr.getResponseHeader("Retry-After");
      if (retryAfter) headers.set("Retry-After", retryAfter);
      const response = new Response(null, { status: xhr.status, headers });
      resolve(response.ok ? { data: body as T, response } : { error: body, response });
    };
    xhr.send(form);
  });
}

/**
 * upload sends one multipart write the reader asked for, with write()'s
 * Idempotency-Key and retries, reporting how much of the body was sent.
 */
export function upload<T>(
  url: string,
  form: FormData,
  onProgress?: (p: UploadProgress) => void,
): Promise<T> {
  return write<T>((init) => postForm<T>(url, form, init, onProgress));
}

/**
 * loginUrl starts Discord sign-in and comes back to redirectTo afterwards.
 * The backend keeps only the path of redirectTo, so it can't leave the site.
 */
export function loginUrl(redirectTo = "/guilds"): string {
  return `/api/auth/login?${new URLSearchParams({ redirect_to: redirectTo })}`;
}
