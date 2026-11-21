//! Offline evaluation runner: replays a labelled fixture through [`KnowledgeService::search`].

use warptalk_kernel::AppResult;

use super::model::{AccessContext, SearchFilters};
use super::search::KnowledgeService;
use crate::domain::{EvalFixture, EvalReport, evaluate};

/// Which id of a hit is compared with the fixture's `relevant` list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelevanceKey {
    SourceId,
    ChunkId,
}

pub async fn run_eval(
    service: &KnowledgeService,
    ctx: &AccessContext,
    fixture: &EvalFixture,
    k: usize,
    key: RelevanceKey,
) -> AppResult<EvalReport> {
    let filters = SearchFilters {
        limit: Some(k.max(1)),
        ..SearchFilters::default()
    };
    let mut rankings = Vec::with_capacity(fixture.cases.len());
    for case in &fixture.cases {
        let r = service.search(ctx, &case.query, &filters).await?;
        rankings.push(
            r.chunks
                .iter()
                .map(|h| match key {
                    RelevanceKey::SourceId => h.payload.source_id.clone(),
                    RelevanceKey::ChunkId => h.chunk_id.to_string(),
                })
                .collect(),
        );
    }
    Ok(evaluate(&fixture.cases, &rankings, k))
}
