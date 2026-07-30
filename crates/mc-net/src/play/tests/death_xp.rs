use super::{XpState, recoverable_death_xp};

#[test]
fn recoverable_death_xp_uses_level_cap() {
    let mut xp = XpState {
        total: 1_000,
        level: 40,
        ..XpState::default()
    };

    assert_eq!(recoverable_death_xp(&xp), 100);

    xp.level = 3;
    assert_eq!(recoverable_death_xp(&xp), 21);
}
