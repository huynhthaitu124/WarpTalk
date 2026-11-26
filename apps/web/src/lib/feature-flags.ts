/**
 * Product switches for surfaces that are kept in the code but not shown.
 *
 * ADMIN_CONSOLE_ENABLED: the platform admin console (`/admin/*`, and the admin-only `/billing`
 * and `/notifications` screens). Off: those addresses forward to the user's normal landing page
 * and nothing links to them. The backend admin APIs are untouched.
 */
export const ADMIN_CONSOLE_ENABLED = false;

const ADMIN_ONLY_PATH = /^\/(?:admin|billing|notifications)(?:\/|$)/;

export function isHiddenAdminPath(pathname: string): boolean {
  return !ADMIN_CONSOLE_ENABLED && ADMIN_ONLY_PATH.test(pathname);
}

/**
 * WORKSPACE_UI_ENABLED: workspaces as something the user sees and manages. Off: every account
 * works inside one personal workspace that the shell creates silently on first sign-in, and the
 * switcher, the create/join/plans onboarding screens and invite prompts are not shown. The data
 * model is unchanged (rows still carry a workspace id), so turning this back on needs no migration.
 */
export const WORKSPACE_UI_ENABLED = false;

/** Name given to the workspace created silently for an account that has none. */
export const PERSONAL_WORKSPACE_NAME = "Personal";

const WORKSPACE_ONBOARDING_PATH = /^\/workspace(?:\/(?:create|join|plans))?\/?$/;

/** The workspace onboarding screens that are skipped while workspaces are hidden. */
export function isHiddenWorkspaceOnboarding(pathname: string): boolean {
  return !WORKSPACE_UI_ENABLED && WORKSPACE_ONBOARDING_PATH.test(pathname);
}

/**
 * Meeting recording (LiveKit egress) is not part of the product: the Rust backend never starts an
 * egress, so no recording artifact exists. Off hides the record page's player, its seek links and
 * the "recording failed / processing" notices instead of showing them for a file that never comes.
 */
export const RECORDING_ENABLED = false;
