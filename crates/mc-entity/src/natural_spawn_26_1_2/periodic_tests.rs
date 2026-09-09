use super::overworld_sky_darken_26_1_2;

#[test]
fn overworld_sky_darken_follows_26_1_2_day_timeline() {
    assert_eq!(overworld_sky_darken_26_1_2(6_000), 0);
    assert_eq!(overworld_sky_darken_26_1_2(13_000), 6);
    assert_eq!(overworld_sky_darken_26_1_2(13_670), 11);
    assert_eq!(overworld_sky_darken_26_1_2(18_000), 11);
    assert_eq!(overworld_sky_darken_26_1_2(23_000), 6);
}
