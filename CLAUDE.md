# WarpTalk (Rust) – agent conventions

Read `docs/architecture/README.md` §3–4 once for the big picture. Spec lives in `docs/spec/` (read only the sections your task names; `05a-data-dictionary.md` is the schema source of truth).

## Hard rules
- Edit ONLY the crate(s) your task names. Never edit root `Cargo.toml`; if you need a dependency that is not in `[workspace.dependencies]`, stop and report it.
- Add deps as `name = { workspace = true }`. Internal crates also via `{ workspace = true }`.
- Crate dependency direction: `apps -> modules|pipeline|providers|shared`; `modules -> shared|providers` (never module -> module); `shared/* -> shared/kernel` only.
- Module layers: `domain/` (no tokio/sqlx/redis, pure), `application/` (use cases + port traits), `infrastructure/` (sqlx/redis adapters), `http/` (axum handlers + DTOs).
- Cross-module needs: define a port trait in the consumer's `application/ports.rs`; `apps/api` wires the adapter.
- Errors: `thiserror` in libs, `anyhow` only in `apps/*`. HTTP errors map through `warptalk_kernel::AppError` -> JSON `{ "code": "...", "message": "..." }`.
- SQL: use runtime `sqlx::query` / `query_as` (NOT the `query!` macros: no DB at compile time). Always qualify schema (`auth.users`). Every tenant query filters by `workspace_id`.
- Migrations: `crates/modules/<m>/migrations/NNNN_name.sql`, forward-only, first file creates the schema (`CREATE SCHEMA IF NOT EXISTS ...`). Column names/types follow `05a`.
- Async traits used as `Arc<dyn Trait>`: `#[async_trait]`.
- No `unwrap()`/`expect()` outside tests (test modules: `#[allow(clippy::unwrap_used)]`). No secrets in code; config comes from env via `warptalk-config`.
- IDs: `uuid` v7 for new rows. Time: `chrono::DateTime<Utc>`.

## Verify before reporting done
```
cargo fmt --all
cargo clippy -p <your-crate> --all-targets -- -D warnings
cargo test -p <your-crate>
```
Local services for integration tests: Postgres `postgres://postgres@localhost:5432/warptalk` (trust auth), Redis `redis://localhost:6379`. Integration tests must skip (return early with a printed note) when `DATABASE_URL` / `REDIS_URL` env vars are unset.

## Report format (keep it short)
List: files created/changed, public API added (signatures only), tests run with pass/fail counts, any deviation or blocker. Do not paste file contents.
