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
Local services for integration tests: Postgres `postgres://postgres@localhost:5432/warptalk` (trust auth), Redis `redis://localhost:6379`. If unreachable, run `scripts/dev-services.sh` (never edit pg_hba.conf, never start/stop other clusters).

## Report format (keep it short)
List: files created/changed, public API added (signatures only), tests run with pass/fail counts, any deviation or blocker. Do not paste file contents.

## Module public surface (every `crates/modules/*`)
```rust
pub const MIGRATIONS: warptalk_db::ModuleMigrations;          // module name = schema owner, e.g. "identity"
pub struct <Name>Module { /* Arc'd services */ }
impl <Name>Module {
    pub fn new(deps: <Name>Deps) -> Self;                      // Deps = struct of pool, JwtKeys, Arc<dyn Clock>, Arc<dyn Port>...
    pub fn router(&self) -> axum::Router;                      // state already applied; paths relative, api mounts under /api/v1
}
```
- No cross-schema foreign keys (ids from other modules are plain uuid columns).
- Auth: handlers take `warptalk_web::AuthUser`; module state implements `FromRef` for `JwtKeys`.
- Integration tests build the router and call it with `tower::ServiceExt::oneshot` against the local Postgres; each test uses fresh random emails/names so tests can run repeatedly.

## Upstream source (read-only reference)
Shallow clones of the original system: `/tmp/claude-0/-home-user-WarpTalk/d227b7da-7786-5638-ae3d-034b3c9aa83f/scratchpad/upstream/{warptalk-ai,warptalk-backend}`. Spec files cite `path:line` there. Read only the files your task names; never copy secrets or hostnames. Port behaviour, not code style; improvements marked "(proposal)" in the spec are wanted.
