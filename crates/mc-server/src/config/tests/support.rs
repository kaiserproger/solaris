use super::super::*;

pub(super) fn stub_blocks() -> Arc<BlockRegistry> {
    Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"))
}

pub(super) fn stub_tags() -> Arc<TagsData> {
    Arc::new(TagsData::default())
}
