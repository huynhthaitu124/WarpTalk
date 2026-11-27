/**
 * The macOS app serves the static export from `warptalk://app/`. WebKit keeps no cookies for a
 * custom scheme: writes to `document.cookie` are dropped and reads come back empty. The session
 * lives in cookies (access_token, warptalk_session), so login "succeeded" and the route gate then
 * found no session and sent the user straight back to /login.
 *
 * Outside http(s) this swaps `document.cookie` for a localStorage-backed jar. It understands
 * `expires` / `max-age` (a past date or max-age<=0 deletes) and ignores path/domain/secure/samesite,
 * since the page has exactly one origin. On http(s) nothing changes.
 */
const STORE_KEY = "warptalk.cookie-jar";

type Jar = Record<string, { v: string; exp: number | null }>;

function load(): Jar {
  try {
    const raw = localStorage.getItem(STORE_KEY);
    return raw ? (JSON.parse(raw) as Jar) : {};
  } catch {
    return {};
  }
}

function save(jar: Jar) {
  try {
    localStorage.setItem(STORE_KEY, JSON.stringify(jar));
  } catch {
    // Blocked storage: the session just will not persist, same as before the shim.
  }
}

function live(jar: Jar, now: number): Jar {
  const out: Jar = {};
  for (const [k, c] of Object.entries(jar)) if (c.exp === null || c.exp > now) out[k] = c;
  return out;
}

export function parseCookieAssignment(assignment: string, now: number): { name: string; value: string; exp: number | null } | null {
  const [pair, ...attrs] = assignment.split(";");
  const eq = pair.indexOf("=");
  if (eq <= 0) return null;
  const name = pair.slice(0, eq).trim();
  const value = pair.slice(eq + 1).trim();
  let exp: number | null = null;
  for (const attr of attrs) {
    const [rawKey, ...rest] = attr.split("=");
    const key = rawKey.trim().toLowerCase();
    const val = rest.join("=").trim();
    if (key === "max-age") {
      const secs = Number(val);
      if (Number.isFinite(secs)) {
        exp = now + secs * 1000;
        break; // max-age wins over expires
      }
    } else if (key === "expires") {
      const t = Date.parse(val);
      if (!Number.isNaN(t)) exp = t;
    }
  }
  return { name, value, exp };
}

function install() {
  if (typeof window === "undefined" || typeof document === "undefined") return;
  if (/^https?:$/.test(window.location.protocol)) return;
  Object.defineProperty(document, "cookie", {
    configurable: true,
    get() {
      const jar = live(load(), Date.now());
      return Object.entries(jar)
        .map(([k, c]) => `${k}=${c.v}`)
        .join("; ");
    },
    set(assignment: string) {
      const now = Date.now();
      const parsed = parseCookieAssignment(String(assignment), now);
      if (!parsed) return;
      const jar = live(load(), now);
      if (parsed.exp !== null && parsed.exp <= now) delete jar[parsed.name];
      else jar[parsed.name] = { v: parsed.value, exp: parsed.exp };
      save(jar);
    },
  });
}

install();
