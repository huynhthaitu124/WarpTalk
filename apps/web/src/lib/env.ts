/**
 * The single place that reads build-time environment. Components import from here, never from
 * `import.meta.env` / `process.env` directly, so the bundler can change without touching UI code.
 */
export const IS_DEV: boolean = import.meta.env.DEV;
