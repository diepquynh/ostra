import { api } from "./api";

/** Read `#token=...` from a URL hash. */
export function tokenFromHash(hash: string): string | null {
  const m = /(?:^#|&)token=([^&]+)/.exec(hash);
  return m ? decodeURIComponent(m[1]) : null;
}

/** A token from a pasted sign-in link, or a bare token. */
export function tokenFromInput(input: string): string | null {
  const text = input.trim();
  const hash = text.indexOf("#");
  if (hash >= 0) return tokenFromHash(text.slice(hash));
  return /^[0-9a-f]{16,}$/i.test(text) ? text : null;
}

export async function exchangeToken(token: string): Promise<string | null> {
  try {
    await api.exchange(token);
    return null;
  } catch (e) {
    return (e as Error).message;
  }
}

/**
 * Exchange the one-time token from the URL `ostra` printed for a session cookie, then strip it
 * from the address bar so it is not left in history or copied along with a link.
 */
export async function exchangeTokenFromUrl(): Promise<string | null> {
  const token = tokenFromHash(location.hash);
  if (!token) return null;
  // A browser may load the page in the background (prerender) while the URL is still being
  // typed or pasted. Spending the one-time token there would leave the real visit with a used
  // link, so wait until the page is shown.
  const doc = document as Document & { prerendering?: boolean };
  if (doc.prerendering) {
    await new Promise((resolve) => document.addEventListener("prerenderingchange", resolve, { once: true }));
  }
  if (exchanged.has(token)) return null;
  exchanged.add(token);
  history.replaceState(null, "", location.pathname + location.search);
  return exchangeToken(token);
}

const exchanged = new Set<string>();

/**
 * Pasting a new sign-in link into an open tab changes only the hash, which does not reload the
 * page, so the boot-time exchange never sees it. Reload so it does.
 */
export function watchForTokens(): void {
  window.addEventListener("hashchange", () => {
    if (tokenFromHash(location.hash)) location.reload();
  });
}
