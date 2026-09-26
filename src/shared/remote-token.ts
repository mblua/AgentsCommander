// #2646 - the web remote token is untrusted URL/session input until it is
// checked. The server only ever creates it as uuid::Uuid::new_v4().to_string()
// (lowercase, hyphenated), so anything else is dropped before it can reach
// sessionStorage or the WebSocket URL.

const REMOTE_TOKEN_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

export const REMOTE_TOKEN_KEY = "remoteToken";

export function isRemoteToken(value: string | null | undefined): value is string {
  return typeof value === "string" && REMOTE_TOKEN_RE.test(value);
}

/**
 * The token for the WS URL. Precedence is unchanged (URL param || stored): a
 * present URL token wins even when invalid, and then yields "" without falling
 * back to the stored one. An absent or empty URL param uses the stored token,
 * which must also be valid.
 */
export function resolveRemoteToken(
  params: URLSearchParams,
  storage: Pick<Storage, "getItem">,
): string {
  const fromUrl = params.get(REMOTE_TOKEN_KEY);
  if (fromUrl) return isRemoteToken(fromUrl) ? fromUrl : "";
  const stored = storage.getItem(REMOTE_TOKEN_KEY);
  return isRemoteToken(stored) ? stored : "";
}

/** Keeps a valid URL token for later page loads; an invalid one is not stored. */
export function storeRemoteTokenFromUrl(
  params: URLSearchParams,
  storage: Pick<Storage, "setItem">,
): void {
  const fromUrl = params.get(REMOTE_TOKEN_KEY);
  if (isRemoteToken(fromUrl)) storage.setItem(REMOTE_TOKEN_KEY, fromUrl);
}
