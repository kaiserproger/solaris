//! Connection-local settlement labels, planned from
//! [`mc_core::boundary::WorldSnapshot::settlement_labels`].
//!
//! Core owns the label data (anchor/position/text); this module owns only the
//! virtual wire image per viewer: negative entity ids that never alias
//! gameplay positives, an [`AddEntity`] + full metadata on label enter,
//! metadata-only [`ClientboundSetEntityData`] on text change, and
//! [`RemoveEntities`] on exit/removal/dimension switch. Labels never enter the
//! core store and are never targetable/pushable (`text_display` has no
//! hitbox).
//!
//! The spawn type comes from the canonical entity contract: 26.3 text_display
//! is 135, not the old 26.1.2 id 131 (now Stray). The legacy registry-name map
//! translates 135 back to 131 for protocol 775. Metadata indices are identical in
//! the local 26.3 and 26.1.2 `javap` oracles:
//! `Entity` defines 0..7 (shared flags 0, custom name 2, visible 3),
//! `Display` defines 8..22 (billboard 15, view range 17, shadow 18..19, width
//! 20, height 21), `Display$TextDisplay` defines 23..27 (text 23, line width
//! 24, background 25, opacity 26, style flags 27). The `COMPONENT` serializer
//! is id 5 (verified serial order BYTE, INT, LONG, FLOAT, STRING, COMPONENT,
//! OPTIONAL_COMPONENT, ITEM_STACK, ... in
//! `EntityDataSerializers.<clinit>`); `OptionalComponent` is 6. Billboard
//! ordinals are FIXED=0, VERTICAL=1, HORIZONTAL=2, CENTER=3
//! (`Display$BillboardConstraints.<clinit>`).

use std::collections::BTreeMap;

use mc_core::boundary::WorldSnapshot;
use mc_protocol::packets::play::{
    AddEntity, ClientboundSetEntityData, EntityDataValue as WireEntityDataValue, EntityVec3,
    RemoveEntities,
};

use crate::plan::{PlannedPacket, ViewerState, push_reliable};

/// Cached canonical type, never a version-specific duplicate of the registry.
static TEXT_DISPLAY_TYPE_ID: std::sync::LazyLock<i32> = std::sync::LazyLock::new(|| {
    mc_data::entity_types::entity_type_contract_26_1_2_by_name("minecraft:text_display")
        .expect("canonical text_display entity contract")
        .protocol_id as i32
});

/// `Display.DATA_BILLBOARD_RENDER_CONSTRAINTS_ID`: index 15, BYTE serializer.
/// CENTER (3) faces the viewer, matching in-world sign readability.
const BILLBOARD_INDEX: u8 = 15;
const BILLBOARD_CENTER: i8 = 3;

/// `Display$TextDisplay.DATA_TEXT_ID`: index 23, COMPONENT serializer (id 5).
const TEXT_INDEX: u8 = 23;

/// `Display.DATA_VIEW_RANGE_ID`: index 17, FLOAT serializer. Vanilla default
/// is 1.0; multiplied by 64 blocks client-side.
const VIEW_RANGE_INDEX: u8 = 17;
/// `Display.DATA_SHADOW_RADIUS_ID`: index 18, FLOAT.
const SHADOW_RADIUS_INDEX: u8 = 18;
/// `Display.DATA_SHADOW_STRENGTH_ID`: index 19, FLOAT.
const SHADOW_STRENGTH_INDEX: u8 = 19;

/// Vanilla text-display view-range default (1.0 = 64 blocks), bounded by the
/// edge's own label visibility cap below.
const LABEL_VIEW_RANGE: f32 = 1.0;
/// Labels must not out-render normal mobs: shadow hidden, no darkening.
const LABEL_SHADOW_RADIUS: f32 = 0.0;
const LABEL_SHADOW_STRENGTH: f32 = 1.0;

/// Viewer-local label visibility, in blocks (squared comparison). Labels past
/// this range despawn connection-locally even while retained in the snapshot.
const LABEL_VISIBILITY_RANGE: f64 = 48.0;
const LABEL_VISIBILITY_RANGE_SQ: f64 = LABEL_VISIBILITY_RANGE * LABEL_VISIBILITY_RANGE;

/// Connection-local virtual ids: negative, never aliasing gameplay positives.
/// One label per stable anchor key; ids are assigned in anchor order and never
/// reused after a label leaves, so a removed slot cannot alias a later label.
#[derive(Debug, Default)]
pub(crate) struct LabelBaseline {
    /// Anchor -> (virtual id, position, last sent text).
    by_anchor: BTreeMap<[i32; 3], (i32, [f64; 3], String)>,
    /// Last allocated id; zero before the first allocation.
    next: i32,
}

impl LabelBaseline {
    pub(crate) fn alloc(&mut self) -> Option<i32> {
        self.next = self.next.checked_sub(1)?;
        Some(self.next)
    }
}

/// Whether the viewer holds the chunk carrying `position` and is close enough
/// for the label to stay visible.
pub(crate) fn label_visible(
    loaded: &impl Fn((i32, i32)) -> bool,
    viewer_position: [f64; 3],
    position: [f64; 3],
) -> bool {
    let chunk = (
        (position[0] / 16.0).floor() as i32,
        (position[2] / 16.0).floor() as i32,
    );
    if !loaded(chunk) {
        return false;
    }
    let dx = position[0] - viewer_position[0];
    let dy = position[1] - viewer_position[1];
    let dz = position[2] - viewer_position[2];
    dx * dx + dy * dy + dz * dz <= LABEL_VISIBILITY_RANGE_SQ
}

/// Text-only delta; constant display styling is sent once at spawn.
pub(crate) fn label_metadata(entity_id: i32, text: &str) -> ClientboundSetEntityData {
    ClientboundSetEntityData {
        entity_id,
        values: vec![WireEntityDataValue::Component {
            index: TEXT_INDEX,
            value: text.to_owned(),
        }],
    }
}

/// Spawn image: shared-flags invisible=false baseline, billboard, view range,
/// shadow, full text. Kept beside [`label_metadata`] so tests prove the
/// exact wire shape of both spawn and update paths.
pub(crate) fn label_spawn_metadata(text: &str) -> Vec<WireEntityDataValue> {
    vec![
        WireEntityDataValue::Byte { index: 0, value: 0 },
        WireEntityDataValue::Byte {
            index: BILLBOARD_INDEX,
            value: BILLBOARD_CENTER,
        },
        WireEntityDataValue::Float {
            index: VIEW_RANGE_INDEX,
            value: LABEL_VIEW_RANGE,
        },
        WireEntityDataValue::Float {
            index: SHADOW_RADIUS_INDEX,
            value: LABEL_SHADOW_RADIUS,
        },
        WireEntityDataValue::Float {
            index: SHADOW_STRENGTH_INDEX,
            value: LABEL_SHADOW_STRENGTH,
        },
        WireEntityDataValue::Component {
            index: TEXT_INDEX,
            value: text.to_owned(),
        },
    ]
}

/// Deterministic per-label UUID: type-3 style MD5 of dimension + anchor, so
/// re-joins and re-spawns keep one stable virtual identity per label without
/// core storage.
pub(crate) fn label_uuid(
    dimension: mc_core::dimensions::Dimension,
    anchor: [i32; 3],
) -> uuid::Uuid {
    use md5::Digest as _;
    let mut hasher = md5::Md5::new();
    hasher.update(b"solaris:label");
    hasher.update([dimension as u8]);
    for coordinate in anchor {
        hasher.update(coordinate.to_be_bytes());
    }
    let digest = hasher.finalize();
    let mut raw = [0_u8; 16];
    raw.copy_from_slice(&digest);
    raw[6] = (raw[6] & 0x0f) | 0x30;
    raw[8] = (raw[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(raw)
}

/// Plan connection-local label packets for one viewer from one snapshot.
///
/// Removals, spawns and metadata updates are reliable: baseline text must not
/// advance past metadata discarded by the best-effort world queue. Unchanged
/// visibility and text produce no packets or scratch allocations.
pub(crate) fn plan_labels(
    snapshot: &WorldSnapshot,
    viewer: &mut ViewerState,
    view_distance: i32,
    out: &mut Vec<PlannedPacket>,
    credit: &mut usize,
) {
    if snapshot.dimension != viewer.dimension {
        remove_all_labels(viewer, out);
        viewer.label_dimension = None;
        return;
    }
    if viewer
        .label_dimension
        .is_some_and(|held| held != snapshot.dimension)
    {
        remove_all_labels(viewer, out);
    }
    viewer.label_dimension = Some(snapshot.dimension);
    let connection = viewer.connection;
    let position = viewer.position;
    let viewer_chunk = (
        (position[0] / 16.0).floor() as i32,
        (position[2] / 16.0).floor() as i32,
    );
    let held = &viewer.loaded_chunks;
    let visible = |label: &mc_core::settlements::presentation::SettlementLabel| {
        let chunk = (
            (label.position[0] / 16.0).floor() as i32,
            (label.position[2] / 16.0).floor() as i32,
        );
        chunk.0.abs_diff(viewer_chunk.0) <= view_distance.max(0) as u32
            && chunk.1.abs_diff(viewer_chunk.1) <= view_distance.max(0) as u32
            && label_visible(&|chunk| held.contains_key(&chunk), position, label.position)
    };
    let mut removed = Vec::new();
    viewer
        .labels
        .by_anchor
        .retain(|anchor, (id, old_position, _)| {
            let retained = snapshot
                .settlement_labels
                .binary_search_by_key(anchor, |label| label.anchor)
                .ok()
                .is_some_and(|index| {
                    let label = &snapshot.settlement_labels[index];
                    label.position == *old_position && visible(label)
                });
            if !retained {
                removed.push(*id);
            }
            retained
        });
    if !removed.is_empty() {
        push_reliable(
            out,
            connection,
            &RemoveEntities {
                entity_ids: removed,
            },
        );
    }
    for label in snapshot
        .settlement_labels
        .iter()
        .filter(|label| visible(label))
    {
        if let Some((id, _, text)) = viewer.labels.by_anchor.get_mut(&label.anchor) {
            if *text != label.text {
                push_reliable(out, connection, &label_metadata(*id, &label.text));
                text.clone_from(&label.text);
            }
            continue;
        }
        if *credit == 0 {
            continue;
        }
        let Some(id) = viewer.labels.alloc() else {
            continue;
        };
        push_reliable(
            out,
            connection,
            &AddEntity {
                entity_id: id,
                uuid: label_uuid(snapshot.dimension, label.anchor),
                entity_type_id: *TEXT_DISPLAY_TYPE_ID,
                x: label.position[0],
                y: label.position[1],
                z: label.position[2],
                movement: EntityVec3::ZERO,
                pitch: 0.0,
                yaw: 0.0,
                head_yaw: 0.0,
                data: 0,
            },
        );
        push_reliable(
            out,
            connection,
            &ClientboundSetEntityData {
                entity_id: id,
                values: label_spawn_metadata(&label.text),
            },
        );
        *credit -= 1;
        viewer
            .labels
            .by_anchor
            .insert(label.anchor, (id, label.position, label.text.clone()));
    }
}

/// Drop every retained virtual label id (dimension switch / forget pass).
/// Reliable: the client's fresh level must not keep stale nameplates.
fn remove_all_labels(viewer: &mut ViewerState, out: &mut Vec<PlannedPacket>) {
    if viewer.labels.by_anchor.is_empty() {
        return;
    }
    let mut ids: Vec<i32> = viewer
        .labels
        .by_anchor
        .values()
        .map(|(id, _, _)| *id)
        .collect();
    ids.sort_unstable();
    push_reliable(out, viewer.connection, &RemoveEntities { entity_ids: ids });
    viewer.labels.by_anchor.clear();
}

/// Forget-pass entry: same-dimension respawn clears label ids alongside
/// gameplay entities, then re-plans from the next snapshot.
pub(crate) fn forget_labels(viewer: &mut ViewerState, out: &mut Vec<PlannedPacket>) {
    remove_all_labels(viewer, out);
}

#[cfg(test)]
#[path = "settlement_labels_tests.rs"]
mod tests;
