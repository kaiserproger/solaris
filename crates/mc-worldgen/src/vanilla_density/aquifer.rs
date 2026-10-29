//! Overworld noise aquifers, applied after either CPU or GPU density filling.
//!
//! Cell geometry, fluid levels and pressure follow the 26.3
//! `Aquifer.NoiseBasedAquifer` implementation. The solid grid contains only
//! density signs, so barrier comparisons resample the density on the CPU.
use std::collections::HashMap;

use super::{DensitySampler, Id, VanillaDensity};
use crate::vanilla_climate::noise::NoiseFactory;

const OFFSETS: [(i32, i32); 13] = [
    (0, 0),
    (-2, -1),
    (-1, -1),
    (0, -1),
    (1, -1),
    (-3, 0),
    (-2, 0),
    (-1, 0),
    (1, 0),
    (-2, 1),
    (-1, 1),
    (0, 1),
    (1, 1),
];
const DRY: i32 = -32512;

type Point = (i32, i32, i32);
#[derive(Clone, Copy)]
struct Status {
    level: i32,
    lava: bool,
}
impl Status {
    fn at(self, y: i32) -> Fluid {
        if y >= self.level {
            Fluid::Air
        } else if self.lava {
            Fluid::Lava
        } else {
            Fluid::Water
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fluid {
    Air,
    Water,
    Lava,
    Solid,
}

pub(crate) struct Aquifer<'a> {
    density: &'a VanillaDensity,
    sampler: DensitySampler<'a>,
    random: NoiseFactory,
    barrier: Id,
    floodedness: Id,
    spread: Id,
    lava: Id,
    erosion: Id,
    depth: Id,
    cells: HashMap<Point, (Point, Option<Status>)>,
    surfaces: HashMap<(i32, i32), i32>,
}
impl<'a> Aquifer<'a> {
    pub(crate) fn new(density: &'a VanillaDensity, seed: u64) -> Self {
        let runtime = &density.runtime;
        Self {
            density,
            sampler: runtime.sampler(),
            random: NoiseFactory::new(seed)
                .random("minecraft:aquifer")
                .positional(),
            barrier: runtime.field("barrier").expect("overworld barrier"),
            floodedness: runtime
                .field("fluid_level_floodedness")
                .expect("overworld floodedness"),
            spread: runtime
                .field("fluid_level_spread")
                .expect("overworld spread"),
            lava: runtime.field("lava").expect("overworld lava"),
            erosion: runtime.field("erosion").expect("overworld erosion"),
            depth: runtime.field("depth").expect("overworld depth"),
            cells: HashMap::new(),
            surfaces: HashMap::new(),
        }
    }
    fn global(&self, y: i32) -> Status {
        if y < -54 {
            Status {
                level: -54,
                lava: true,
            }
        } else {
            Status {
                level: self.density.sea_level(),
                lava: false,
            }
        }
    }
    fn center(&mut self, cell: Point) -> Point {
        if let Some(&(point, _)) = self.cells.get(&cell) {
            return point;
        }
        let mut r = self.random.at(cell.0, cell.1, cell.2);
        let point = (
            cell.0 * 16 + r.next_int_bounded(10),
            cell.1 * 12 + r.next_int_bounded(9),
            cell.2 * 16 + r.next_int_bounded(10),
        );
        self.cells.insert(cell, (point, None));
        point
    }
    fn sample(&mut self, id: Id, x: i32, y: i32, z: i32) -> f64 {
        f64::from(self.sampler.sample_terrain(id, x, y, z))
    }
    fn surface(&mut self, x: i32, z: i32) -> i32 {
        let key = (x.div_euclid(4) * 4, z.div_euclid(4) * 4);
        if let Some(&value) = self.surfaces.get(&key) {
            return value;
        }
        let value = self
            .sample(self.density.preliminary_surface, key.0, 0, key.1)
            .floor() as i32;
        self.surfaces.insert(key, value);
        value
    }
    fn status(&mut self, cell: Point) -> Status {
        let (point, cached) = self.cells[&cell];
        if let Some(status) = cached {
            return status;
        }
        let (x, y, z) = point;
        let global = self.global(y);
        let mut lowest = i32::MAX;
        let mut under_ocean = false;
        for (dx, dz) in OFFSETS {
            let surface = self.surface(x + dx * 16, z + dz * 16);
            let adjusted = surface + 8;
            let center = dx == 0 && dz == 0;
            if center && y - 12 > adjusted {
                self.cells.get_mut(&cell).unwrap().1 = Some(global);
                return global;
            }
            let above = y + 12 > adjusted;
            if (above || center) && adjusted < self.global(adjusted).level {
                if center {
                    under_ocean = true;
                }
                if above {
                    let status = self.global(adjusted);
                    self.cells.get_mut(&cell).unwrap().1 = Some(status);
                    return status;
                }
            }
            lowest = lowest.min(surface);
        }
        let factor = if under_ocean {
            (1.0 - f64::from(lowest + 8 - y) / 64.0).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let flood = self.sample(self.floodedness, x, y, z).clamp(-1.0, 1.0);
        // 26.3 aquifer exclusion: min(-0.225 - erosion, max(depth - 0.9, 0)).
        let excluded =
            self.sample(self.erosion, x, y, z) < -0.225 && self.sample(self.depth, x, y, z) > 0.9;
        let level = if excluded {
            DRY
        } else if flood > 0.8 - 1.1 * factor {
            global.level
        } else if flood > 0.4 - 1.2 * factor {
            let (cx, cy, cz) = (x.div_euclid(16), y.div_euclid(40), z.div_euclid(16));
            let spread = self.sample(self.spread, cx, cy, cz) * 10.0;
            lowest.min(cy * 40 + 20 + (spread / 3.0).floor() as i32 * 3)
        } else {
            DRY
        };
        let lava = global.lava
            || (level <= -10
                && level != DRY
                && self
                    .sample(
                        self.lava,
                        x.div_euclid(64),
                        y.div_euclid(40),
                        z.div_euclid(64),
                    )
                    .abs()
                    > 0.3);
        let status = Status { level, lava };
        self.cells.get_mut(&cell).unwrap().1 = Some(status);
        status
    }
    fn pressure(&mut self, x: i32, y: i32, z: i32, a: Status, b: Status) -> f64 {
        let (fa, fb) = (a.at(y), b.at(y));
        if (fa == Fluid::Water && fb == Fluid::Lava) || (fa == Fluid::Lava && fb == Fluid::Water) {
            return 2.0;
        }
        let difference = (a.level - b.level).abs();
        if difference == 0 {
            return 0.0;
        }
        let above = f64::from(y) + 0.5 - f64::from(a.level + b.level) * 0.5;
        let edge = f64::from(difference) / 2.0 - above.abs();
        let gradient = if above > 0.0 {
            if edge > 0.0 { edge / 1.5 } else { edge / 2.5 }
        } else {
            let v = 3.0 + edge;
            if v > 0.0 { v / 3.0 } else { v / 10.0 }
        };
        let noise = if (-2.0..=2.0).contains(&gradient) {
            self.sample(self.barrier, x, y, z)
        } else {
            0.0
        };
        2.0 * (noise + gradient)
    }
    pub(crate) fn substance(&mut self, x: i32, y: i32, z: i32) -> Fluid {
        if y < -54 {
            return Fluid::Lava;
        }
        let (gx, gy, gz) = (
            (x - 5).div_euclid(16),
            (y + 1).div_euclid(12),
            (z - 5).div_euclid(16),
        );
        let mut nearest = [(i32::MAX, (0, 0, 0)); 3];
        for dx in 0..=1 {
            for dy in -1..=1 {
                for dz in 0..=1 {
                    let cell = (gx + dx, gy + dy, gz + dz);
                    let p = self.center(cell);
                    let dist = (p.0 - x).pow(2) + (p.1 - y).pow(2) + (p.2 - z).pow(2);
                    for i in 0..3 {
                        if dist <= nearest[i].0 {
                            for j in (i + 1..3).rev() {
                                nearest[j] = nearest[j - 1];
                            }
                            nearest[i] = (dist, cell);
                            break;
                        }
                    }
                }
            }
        }
        let first = self.status(nearest[0].1);
        let similarity = |a: i32, b: i32| 1.0 - f64::from(b - a) / 25.0;
        let sim12 = similarity(nearest[0].0, nearest[1].0);
        if sim12 <= 0.0 {
            return first.at(y);
        }
        if first.at(y) == Fluid::Water && y - 1 < -54 {
            return Fluid::Water;
        }
        let second = self.status(nearest[1].1);
        let mut pressure = sim12 * self.pressure(x, y, z, first, second);
        let sim13 = similarity(nearest[0].0, nearest[2].0);
        let sim23 = similarity(nearest[1].0, nearest[2].0);
        if sim13 > 0.0 || sim23 > 0.0 {
            let third = self.status(nearest[2].1);
            if sim13 > 0.0 {
                pressure = pressure.max(sim12 * sim13 * self.pressure(x, y, z, first, third));
            }
            if sim23 > 0.0 {
                pressure = pressure.max(sim12 * sim23 * self.pressure(x, y, z, second, third));
            }
        }
        if pressure > 0.0
            && f64::from(
                self.sampler
                    .sample_terrain(self.density.final_density, x, y, z),
            ) + pressure
                > 0.0
        {
            Fluid::Solid
        } else {
            first.at(y)
        }
    }
}
