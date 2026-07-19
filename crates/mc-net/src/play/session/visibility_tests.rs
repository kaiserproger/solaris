use mc_entity::Vec3;

use super::visibility::quantized_entity_delta;

#[test]
fn entity_relative_delta_uses_quantized_absolute_endpoints() {
    let previous = Vec3::new(0.000_12, 64.0, -0.000_12);
    let current = Vec3::new(0.000_13, 64.0, -0.000_13);

    let delta = quantized_entity_delta(current, previous);

    assert_eq!(delta.x, 1.0 / 4096.0);
    assert_eq!(delta.y, 0.0);
    assert_eq!(delta.z, -1.0 / 4096.0);
}

#[test]
fn entity_relative_delta_matches_java_round_at_negative_half_step() {
    let previous = Vec3::new(0.0, 0.0, 0.0);
    let current = Vec3::new(-1.0 / 8192.0, 0.0, 0.0);

    let delta = quantized_entity_delta(current, previous);

    assert_eq!(delta.x, 0.0);
}
