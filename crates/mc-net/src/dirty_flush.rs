pub(crate) async fn write_dirty_flush_blocking(
    flush_plan: mc_world::storage::DirtyFlushPlan,
) -> Result<mc_world::storage::DirtyFlushCommit, String> {
    crate::blocking::spawn_result_blocking(move || flush_plan.write()).await
}
