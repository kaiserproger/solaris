use super::{PlayerPose, arrow_spawn_position, arrow_velocity};

#[test]
fn arrow_launch_uses_player_look_direction_and_draw_power() {
    let pose = PlayerPose {
        yaw: 90.0,
        pitch: -30.0,
        ..PlayerPose::new(1.0, 64.0, 2.0)
    };

    let spawn = arrow_spawn_position(pose);
    let velocity = arrow_velocity(pose, 0.5);

    assert!((spawn.x - 1.0).abs() < 0.000_001);
    assert!((spawn.y - 65.62).abs() < 0.000_001);
    assert!((spawn.z - 2.0).abs() < 0.000_001);
    assert!((velocity.x + 1.299_038_105_676_658).abs() < 0.000_001);
    assert!((velocity.y - 0.75).abs() < 0.000_001);
    assert!(velocity.z.abs() < 0.000_001);
}
