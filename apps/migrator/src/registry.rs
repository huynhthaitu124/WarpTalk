use warptalk_db::ModuleMigrations;

/// Migration sets of every module, in the order they must be applied.
///
/// Business modules append their `MIGRATIONS` const here in dependency order
/// (identity, workspace, room, meeting, transcript, knowledge, assistant, billing, notification).
pub fn registry() -> Vec<ModuleMigrations> {
    vec![
        warptalk_db::OUTBOX_MIGRATIONS,
        warptalk_settings::MIGRATIONS,
        warptalk_identity::MIGRATIONS,
        warptalk_workspace::MIGRATIONS,
        warptalk_room::MIGRATIONS,
        warptalk_meeting::MIGRATIONS,
        warptalk_transcript::MIGRATIONS,
        warptalk_knowledge::MIGRATIONS,
        warptalk_assistant::MIGRATIONS,
    ]
}
