use warptalk_db::ModuleMigrations;

/// Migration sets of every module, in the order they must be applied.
///
/// Business modules append their `MIGRATIONS` const here in dependency order
/// (identity, workspace, room, meeting, transcript, assistant, billing, notification).
pub fn registry() -> Vec<ModuleMigrations> {
    vec![warptalk_db::OUTBOX_MIGRATIONS]
}
