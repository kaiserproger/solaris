//! Behaviour tests for deterministic settlement site selection and layout.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use mc_data::Identifier;
use mc_data::blocks::{BlockReport, BlockStateReport};
use mc_world::BlockRegistry;

use crate::settlement_catalog::{BlueprintCatalog, BlueprintInstance, PoiKind, QuarterTurn};
use crate::settlement_sites::{
    MAX_ROAD_WAYPOINTS, MAX_SITES_PER_PAGE, ROAD_NEIGHBOUR_CELLS, SITE_CELL_BLOCKS,
    SettlementSelector, SiteCandidate, SiteError, SiteLayout, SiteVariant,
};

fn stone_registry() -> BlockRegistry {
    BlockRegistry::from_report(&[BlockReport {
        id: Identifier::parse("minecraft:stone").unwrap(),
        properties: BTreeMap::new(),
        states: vec![BlockStateReport {
            id: 0,
            default: true,
            properties: BTreeMap::new(),
        }],
    }])
    .unwrap()
}

/// A 4x4x4 building with one block. When `role` is set it also carries one POI
/// of that kind, plus a street connection so a home POI has an entrance.
fn building_toml(name: &str, role: Option<(&str, u16)>) -> String {
    let mut text = format!(
        "id = \"solaris:{name}\"\nrevision = 1\n\
         [footprint]\nsize = [4, 4, 4]\nanchor = [0, 0, 0]\n\
         [[palette]]\nindex = 0\nblock = \"minecraft:stone\"\nproperties = {{}}\n\
         [[blocks]]\nx = 0\ny = 0\nz = 0\npalette = 0\n"
    );
    if let Some((kind, capacity)) = role {
        text.push_str(&format!(
            "[[poi]]\nid = \"{kind}\"\nkind = \"{kind}\"\nat = [1, 1, 1]\ncapacity = {capacity}\n\
             [[street_connection]]\nat = [0, 0, 0]\nfacing = \"north\"\n"
        ));
    }
    text
}

fn oversized_home_toml() -> String {
    "id = \"solaris:home\"\nrevision = 1\n\
     [footprint]\nsize = [64, 4, 64]\nanchor = [0, 0, 0]\n\
     [[palette]]\nindex = 0\nblock = \"minecraft:stone\"\nproperties = {}\n\
     [[blocks]]\nx = 0\ny = 0\nz = 0\npalette = 0\n\
     [[poi]]\nid = \"home\"\nkind = \"home\"\nat = [1, 1, 1]\ncapacity = 8\n\
     [[street_connection]]\nat = [0, 0, 0]\nfacing = \"north\"\n"
        .to_owned()
}

/// Roles of a standard test settlement, in catalog order.
const ROLES: [(&str, u16); 4] = [("home", 8), ("hall", 0), ("forge", 0), ("tower", 0)];

fn role_kind(name: &str) -> &str {
    match name {
        "home" => "home",
        "hall" => "meeting",
        "forge" => "work",
        _ => "guard",
    }
}

fn catalog(registry: &BlockRegistry, roles: &[(&str, u16)]) -> BlueprintCatalog {
    let files: Vec<(String, String)> = roles
        .iter()
        .map(|(name, capacity)| {
            (
                format!("structures/{name}.toml"),
                building_toml(name, Some((role_kind(name), *capacity))),
            )
        })
        .collect();
    BlueprintCatalog::from_files(registry, "solaris", &files).unwrap()
}

fn first_candidate(selector: &SettlementSelector, variant: Option<SiteVariant>) -> SiteCandidate {
    for x in 0..64 {
        for z in 0..64 {
            if let Some(candidate) = selector.candidate([x, z])
                && variant.is_none_or(|wanted| candidate.variant == wanted)
            {
                return candidate;
            }
        }
    }
    panic!("deterministic candidate search found nothing");
}

fn neighbour_pair(selector: &SettlementSelector) -> (SiteCandidate, SiteCandidate) {
    for x in -16..64 {
        for z in -16..64 {
            if let (Some(a), Some(b)) = (selector.candidate([x, z]), selector.candidate([x + 1, z]))
            {
                return (a, b);
            }
        }
    }
    panic!("deterministic neighbour search found nothing");
}

#[test]
fn variants_match_the_frozen_table() {
    assert_eq!(SiteVariant::Hamlet.name(), "hamlet");
    assert_eq!(SiteVariant::Village.name(), "village");
    assert_eq!(SiteVariant::Town.name(), "town");
    assert_eq!(SiteVariant::Hamlet.weight(), 65);
    assert_eq!(SiteVariant::Village.weight(), 30);
    assert_eq!(SiteVariant::Town.weight(), 5);
    assert_eq!(SiteVariant::Hamlet.houses(), (4, 5));
    assert_eq!(SiteVariant::Village.houses(), (9, 12));
    assert_eq!(SiteVariant::Town.houses(), (20, 28));
    assert_eq!(SiteVariant::Hamlet.residents(), (8, 16));
    assert_eq!(SiteVariant::Village.residents(), (24, 40));
    assert_eq!(SiteVariant::Town.residents(), (50, 80));
    for variant in [SiteVariant::Hamlet, SiteVariant::Village, SiteVariant::Town] {
        let footprint = variant.footprint();
        assert!(footprint[0] > 0 && footprint[1] > 0 && footprint[2] > 0);
        assert!(footprint[0] <= SITE_CELL_BLOCKS);
        assert!(footprint[2] <= SITE_CELL_BLOCKS);
        assert!(variant.houses().0 <= variant.houses().1);
        assert!(variant.residents().0 <= variant.residents().1);
    }
}

#[test]
fn selection_is_a_pure_function_of_seed_revision_and_cell() {
    let selector = SettlementSelector::new(0x5eed, 7);
    assert_eq!(selector.seed(), 0x5eed);
    assert_eq!(selector.revision(), 7);

    let cells: Vec<[i32; 2]> = (0..256).map(|index| [index % 16, index / 16]).collect();
    let forward: Vec<Option<SiteCandidate>> =
        cells.iter().map(|cell| selector.candidate(*cell)).collect();
    let backward: Vec<Option<SiteCandidate>> = cells
        .iter()
        .rev()
        .map(|cell| selector.candidate(*cell))
        .collect();
    let mut reversed = backward;
    reversed.reverse();
    assert_eq!(forward, reversed);
    assert!(forward.iter().any(Option::is_some));

    // A fresh selector with identical inputs agrees.
    let twin = SettlementSelector::new(0x5eed, 7);
    assert_eq!(
        forward,
        cells
            .iter()
            .map(|cell| twin.candidate(*cell))
            .collect::<Vec<_>>()
    );
    // A different revision or seed changes occupancy.
    let other = SettlementSelector::new(0x5eed, 8);
    assert_ne!(
        forward,
        cells
            .iter()
            .map(|cell| other.candidate(*cell))
            .collect::<Vec<_>>()
    );
}

#[test]
fn discover_is_bounded_ordered_and_repeatable() {
    let selector = SettlementSelector::new(-77, 3);
    let discovered = selector.discover([0, 0], 256);
    assert!(!discovered.is_empty());
    assert!(discovered.len() <= 256);
    assert_eq!(discovered, selector.discover([0, 0], 256));

    // Discover walks cells row-major at MAX_SITES_PER_PAGE columns per row.
    let expected: Vec<SiteCandidate> = (0..256)
        .filter_map(|index| {
            selector.candidate([
                (index % MAX_SITES_PER_PAGE) as i32,
                (index / MAX_SITES_PER_PAGE) as i32,
            ])
        })
        .collect();
    assert_eq!(discovered, expected);

    // A prefix page is exactly the first cells of the longer page.
    let page = selector.discover([0, 0], 128);
    assert_eq!(page, discovered[..page.len()].to_vec());

    let mut cells: BTreeSet<[i32; 2]> = BTreeSet::new();
    for candidate in &discovered {
        assert!(cells.insert(candidate.cell), "one candidate per cell");
        assert_eq!(candidate.size, candidate.variant.footprint());
        assert_eq!(selector.candidate(candidate.cell).as_ref(), Some(candidate));
    }
}

#[test]
fn layout_fills_the_variant_range_and_is_deterministic() {
    let registry = stone_registry();
    let catalog = catalog(&registry, &ROLES);
    let selector = SettlementSelector::new(1234, 5);

    let candidates: Vec<SiteCandidate> = (0..64)
        .flat_map(|x| (0..4).map(move |z| [x, z]))
        .filter_map(|cell| selector.candidate(cell))
        .collect();
    assert!(candidates.len() > 4);

    let mut forward: BTreeMap<String, SiteLayout> = BTreeMap::new();
    for candidate in &candidates {
        let layout = selector.layout(candidate, &catalog, &flat_ground).unwrap();
        forward.insert(candidate.site_id.clone(), layout);
    }
    let mut backward: BTreeMap<String, SiteLayout> = BTreeMap::new();
    for candidate in candidates.iter().rev() {
        let layout = selector.layout(candidate, &catalog, &flat_ground).unwrap();
        backward.insert(candidate.site_id.clone(), layout);
    }
    assert_eq!(forward, backward);

    for (site_id, layout) in &forward {
        assert_eq!(site_id, &layout.candidate.site_id);
        // The layout's candidate is the selected candidate with the resolved
        // base row: the flat 64 this test resolver reports, because every test
        // blueprint anchors on its own ground layer. Everything else is
        // untouched.
        let mut resolved = selector.candidate(layout.candidate.cell).unwrap();
        assert_eq!(layout.candidate.origin[1], 65);
        resolved.origin[1] = 65;
        assert_eq!(layout.candidate, resolved);
        let houses = layout
            .placements
            .iter()
            .filter(|placement| placement.blueprint_id == "solaris:home")
            .count() as u32;
        let (min, max) = layout.candidate.variant.houses();
        assert!((min..=max).contains(&houses), "{site_id} houses {houses}");
        for role in [
            "solaris:home",
            "solaris:hall",
            "solaris:forge",
            "solaris:tower",
        ] {
            assert!(
                layout
                    .placements
                    .iter()
                    .any(|placement| placement.blueprint_id == role),
                "{site_id} is missing {role}"
            );
        }
        let (min, max) = layout.candidate.variant.residents();
        assert!(
            (min..=max).contains(&layout.inhabitant_slots),
            "{site_id} residents {}",
            layout.inhabitant_slots
        );
        assert!(!layout.pois.is_empty());
        let kinds: BTreeSet<&str> = layout.pois.iter().map(|poi| poi.kind.as_str()).collect();
        for kind in [
            PoiKind::Home,
            PoiKind::Meeting,
            PoiKind::Work,
            PoiKind::Guard,
        ] {
            assert!(
                kinds.contains(kind.as_str()),
                "{site_id} exposes no {} POI",
                kind.as_str()
            );
        }
        let mut poi_ids: BTreeSet<&str> = BTreeSet::new();
        for poi in &layout.pois {
            assert!(
                poi_ids.insert(&poi.poi_id),
                "duplicate POI id {}",
                poi.poi_id
            );
            assert!(within_site(poi.at, &layout.candidate));
            let owner = layout
                .placements
                .iter()
                .find(|placement| {
                    let size = catalog.get(&placement.blueprint_id).unwrap().size();
                    let rotated = if placement.rotation == 90 || placement.rotation == 270 {
                        [size[2], size[1], size[0]]
                    } else {
                        size
                    };
                    contains(placement.origin, rotated, poi.at)
                })
                .expect("every POI belongs to a placed building");
            assert_eq!(owner.blueprint_id, poi.building);
        }
        for placement in &layout.placements {
            assert!(within_site(placement.origin, &layout.candidate));
            assert!([0, 90, 180, 270].contains(&placement.rotation));
        }
    }

    // Re-running with the same inputs reproduces the same layouts.
    let twin = SettlementSelector::new(1234, 5);
    for candidate in &candidates {
        assert_eq!(
            twin.layout(candidate, &catalog, &flat_ground).unwrap(),
            forward[&candidate.site_id]
        );
    }
    // And the same site in a different position of the query list is identical.
    assert_eq!(
        selector
            .layout(&candidates[3], &catalog, &flat_ground)
            .unwrap(),
        forward[&candidates[3].site_id]
    );
}

fn contains(origin: [i32; 3], size: [i32; 3], point: [i32; 3]) -> bool {
    point[0] >= origin[0]
        && point[0] < origin[0] + size[0]
        && point[1] >= origin[1]
        && point[1] < origin[1] + size[1]
        && point[2] >= origin[2]
        && point[2] < origin[2] + size[2]
}

fn within_site(point: [i32; 3], site: &SiteCandidate) -> bool {
    point[0] >= site.origin[0]
        && point[0] < site.origin[0] + site.size[0]
        && point[2] >= site.origin[2]
        && point[2] < site.origin[2] + site.size[2]
}

#[test]
fn layout_reports_missing_roles_and_impossible_variants() {
    let registry = stone_registry();
    let selector = SettlementSelector::new(99, 2);
    let candidate = first_candidate(&selector, Some(SiteVariant::Hamlet));

    let homes_only = catalog(&registry, &[("home", 8)]);
    assert!(matches!(
        selector
            .layout(&candidate, &homes_only, &flat_ground)
            .unwrap_err(),
        SiteError::MissingRole { role: "meeting" }
    ));

    let empty_files: Vec<(String, String)> = Vec::new();
    let empty = BlueprintCatalog::from_files(&registry, "solaris", &empty_files).unwrap();
    assert!(matches!(
        selector
            .layout(&candidate, &empty, &flat_ground)
            .unwrap_err(),
        SiteError::MissingRole { role: "home" }
    ));

    // Home POIs cannot hold a hamlet: capacity 1 x at most 5 houses < 8.
    let poor = catalog(
        &registry,
        &[("home", 1), ("hall", 0), ("forge", 0), ("tower", 0)],
    );
    let error = selector
        .layout(&candidate, &poor, &flat_ground)
        .unwrap_err();
    assert!(matches!(
        error,
        SiteError::ExcessInhabitants { requested, capacity }
            if requested >= 8 && capacity < requested
    ));

    // A 64-wide home blueprint cannot be packed into a 128-block hamlet.
    let files = vec![
        ("structures/home.toml".to_owned(), oversized_home_toml()),
        (
            "structures/hall.toml".to_owned(),
            building_toml("hall", Some(("meeting", 0))),
        ),
        (
            "structures/forge.toml".to_owned(),
            building_toml("forge", Some(("work", 0))),
        ),
        (
            "structures/tower.toml".to_owned(),
            building_toml("tower", Some(("guard", 0))),
        ),
    ];
    let oversized = BlueprintCatalog::from_files(&registry, "solaris", &files).unwrap();
    let error = selector
        .layout(&candidate, &oversized, &flat_ground)
        .unwrap_err();
    assert!(matches!(error, SiteError::SiteOutOfBounds { .. }));
}

#[test]
fn site_ids_round_trip_through_the_grid_cell() {
    let selector = SettlementSelector::new(-90210, 11);
    let discovered = selector.discover([-4, -3], 512);
    assert!(!discovered.is_empty());
    for candidate in &discovered {
        assert_eq!(
            selector.cell_from_site_id(&candidate.site_id),
            Some(candidate.cell)
        );
        assert_eq!(
            selector.site_id_for_cell(candidate.cell).as_deref(),
            Some(candidate.site_id.as_str())
        );
    }

    let parts: Vec<&str> = discovered[0].site_id.split('_').collect();
    assert_eq!(parts.len(), 4);
    assert_eq!(parts[0], "site");
    assert_eq!(parts[1].parse::<i32>().unwrap(), discovered[0].cell[0]);
    assert_eq!(parts[2].parse::<i32>().unwrap(), discovered[0].cell[1]);
    assert_eq!(parts[3].len(), 8);
    assert!(
        parts[3]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );

    // A corrupted digest, a forged cell, and a foreign selector all fail.
    let original = discovered[0].site_id.clone();
    let mut chars: Vec<char> = original.chars().collect();
    let last = chars.len() - 1;
    chars[last] = if chars[last] == 'a' { 'b' } else { 'a' };
    let corrupted: String = chars.into_iter().collect();
    assert_eq!(selector.cell_from_site_id(&corrupted), None);
    assert_eq!(selector.cell_from_site_id(""), None);
    assert_eq!(selector.cell_from_site_id("site_1_2"), None);
    assert_eq!(selector.cell_from_site_id("site_1_2_three"), None);
    assert_eq!(selector.cell_from_site_id("site_1_2_deadbeef_extra"), None);
    assert_eq!(selector.cell_from_site_id("other_1_2_deadbeef"), None);
    let foreign = SettlementSelector::new(-90211, 11);
    assert!(
        discovered
            .iter()
            .all(|candidate| foreign.cell_from_site_id(&candidate.site_id).is_none())
    );
    let empty_cell = (0..64)
        .map(|index| [index, 40])
        .find(|cell| selector.candidate(*cell).is_none())
        .unwrap();
    assert_eq!(selector.site_id_for_cell(empty_cell), None);
}

#[test]
fn roads_are_canonical_bounded_and_local() {
    let selector = SettlementSelector::new(4242, 13);
    let (a, b) = neighbour_pair(&selector);
    let ab = selector
        .road(&a, &b)
        .expect("adjacent cells are neighbours");
    let ba = selector.road(&b, &a).expect("neighbourhood is symmetric");
    assert_eq!(ab, ba);
    assert_eq!(ab.edge_id, ba.edge_id);
    assert!(ab.from_site < ab.to_site);
    assert!(ab.from_site == a.site_id || ab.from_site == b.site_id);
    assert!(ab.waypoints.len() >= 2);
    assert!(ab.waypoints.len() <= MAX_ROAD_WAYPOINTS);

    let (from, to) = if ab.from_site == a.site_id {
        (&a, &b)
    } else {
        (&b, &a)
    };
    assert!(on_boundary(ab.waypoints[0], from));
    assert!(on_boundary(*ab.waypoints.last().unwrap(), to));

    // Same site, and anything beyond the neighbour radius, has no edge.
    assert!(selector.road(&a, &a).is_none());
    let far = SiteCandidate {
        cell: [a.cell[0] + ROAD_NEIGHBOUR_CELLS + 1, a.cell[1]],
        site_id: "site_999_999_deadbeef".to_owned(),
        variant: a.variant,
        origin: [a.origin[0] + SITE_CELL_BLOCKS, a.origin[1], a.origin[2]],
        size: a.size,
    };
    assert!(selector.road(&a, &far).is_none());
}

fn on_boundary(point: [i32; 3], site: &SiteCandidate) -> bool {
    let [size_x, _, size_z] = site.size;
    let inside_x = point[0] >= site.origin[0] && point[0] < site.origin[0] + size_x;
    let inside_z = point[2] >= site.origin[2] && point[2] < site.origin[2] + size_z;
    let on_edge_x = point[0] == site.origin[0] || point[0] == site.origin[0] + size_x - 1;
    let on_edge_z = point[2] == site.origin[2] || point[2] == site.origin[2] + size_z - 1;
    inside_x && inside_z && (on_edge_x || on_edge_z)
}

/// A sloped world anchors every building on the terrain under its own authored
/// anchor column, and reports the site base at the site's own origin column.
#[test]
fn each_building_anchors_on_the_terrain_under_its_own_anchor() {
    let registry = stone_registry();
    let catalog = catalog(&registry, &ROLES);
    let selector = SettlementSelector::new(1234, 5);
    let candidate = first_candidate(&selector, Some(SiteVariant::Village));

    // Terrain that rises one row per block along +x: a base row copied from the
    // site origin would sink most of the settlement into the hill.
    let slope = |x: i32, _z: i32| Some(60 + x.rem_euclid(24));
    let layout = selector.layout(&candidate, &catalog, &slope).unwrap();

    assert!(!layout.placements.is_empty());
    // The reported site origin is the first free row above the terrain at the
    // site's own origin column.
    assert_eq!(
        layout.candidate.origin[1],
        slope(layout.candidate.origin[0], layout.candidate.origin[2]).unwrap() + 1
    );

    for placement in &layout.placements {
        let blueprint = catalog
            .get(&placement.blueprint_id)
            .expect("a placement names a catalogued blueprint");
        let turn = QuarterTurn::from_degrees(placement.rotation)
            .expect("a layout only writes authored rotations");
        let anchor = BlueprintInstance::new(
            Arc::clone(blueprint),
            turn,
            [placement.origin[0], 0, placement.origin[2]],
        )
        .placed_anchor();
        let surface = slope(anchor[0], anchor[2]).unwrap();
        assert_eq!(
            placement.origin[1], surface,
            "building {:?} must stand on the terrain row under its anchor {anchor:?}",
            placement.origin
        );
    }
}

/// A blueprint whose anchor sits above its own base row is grounded on the same
/// terrain row as every other building: the anchor chooses the placement column,
/// never the base row. That keeps the placement compatible with the prepare-time
/// fit gate, which refuses any footprint whose terrain rises above the base row
/// (the authored deck case, `fishing_pier` with its anchor at local y=1, would
/// otherwise be refused everywhere).
#[test]
fn an_anchor_above_the_base_row_does_not_move_the_base_row() {
    let registry = stone_registry();
    let files = vec![
        (
            "structures/pier.toml".to_owned(),
            "id = \"solaris:pier\"\nrevision = 1\n\
             [footprint]\nsize = [4, 4, 2]\nanchor = [1, 1, 0]\n\
             [[palette]]\nindex = 0\nblock = \"minecraft:stone\"\nproperties = {}\n\
             [[blocks]]\nx = 0\ny = 0\nz = 0\npalette = 0\n\
             [[blocks]]\nx = 1\ny = 1\nz = 0\npalette = 0\n\
             [[poi]]\nid = \"deck\"\nkind = \"home\"\nat = [1, 1, 0]\ncapacity = 8\n\
             [[street_connection]]\nat = [1, 1, 0]\nfacing = \"north\"\n"
                .to_owned(),
        ),
        (
            "structures/hall.toml".to_owned(),
            building_toml("hall", Some(("meeting", 0))),
        ),
        (
            "structures/forge.toml".to_owned(),
            building_toml("forge", Some(("work", 0))),
        ),
        (
            "structures/tower.toml".to_owned(),
            building_toml("tower", Some(("guard", 0))),
        ),
    ];
    let catalog = BlueprintCatalog::from_files(&registry, "solaris", &files).unwrap();
    let selector = SettlementSelector::new(4242, 9);
    let candidate = first_candidate(&selector, Some(SiteVariant::Hamlet));

    let ground = |_x: i32, _z: i32| Some(70);
    let layout = selector.layout(&candidate, &catalog, &ground).unwrap();

    let mut checked = 0;
    for placement in &layout.placements {
        let blueprint = catalog.get(&placement.blueprint_id).unwrap();
        if blueprint.id() != "solaris:pier" {
            continue;
        }
        let turn = QuarterTurn::from_degrees(placement.rotation).unwrap();
        let anchor = BlueprintInstance::new(
            Arc::clone(blueprint),
            turn,
            [placement.origin[0], 0, placement.origin[2]],
        )
        .placed_anchor();
        assert_eq!(anchor[1], 1, "the authored anchor sits one row up");
        assert_eq!(placement.origin[1], 70);
        assert_eq!(placement.origin[1] + anchor[1], 71);
        checked += 1;
    }
    assert!(
        checked > 0,
        "the hamlet layout must place its home blueprint"
    );
}

/// A resolver that cannot answer a column fails the layout closed.
#[test]
fn an_unanswerable_column_fails_the_layout_closed() {
    let registry = stone_registry();
    let catalog = catalog(&registry, &ROLES);
    let selector = SettlementSelector::new(1234, 5);
    let candidate = first_candidate(&selector, Some(SiteVariant::Hamlet));

    let error = selector
        .layout(&candidate, &catalog, &|_, _| None)
        .unwrap_err();
    assert!(matches!(error, SiteError::Ungrounded { .. }));
}

/// Deterministic flat ground for layouts that only exercise placement rules.
fn flat_ground(_world_x: i32, _world_z: i32) -> Option<i32> {
    Some(64)
}
