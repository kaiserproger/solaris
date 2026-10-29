//! Masked eight-column DAG evaluation. Branches stay lazy per lane; chunk
//! lattice/flat caches are shared with the scalar sampler, not duplicated.
use super::*;

/// One DAG query shares immutable coordinates and a generation across nodes.
/// Masks permit a shared node to be visited lazily by disjoint branch lanes.
#[derive(Clone, Copy, Default)]
pub(super) struct BatchMemo {
    epoch: u64,
    mask: u8,
    values: [f32; 8],
}

impl DensitySampler<'_> {
    pub(crate) fn sample_terrain_batch(&mut self, field: Id, positions: [[i32; 3]; 8]) -> [f32; 8] {
        if self.batch_values.is_empty() {
            self.batch_values
                .resize(self.runtime.nodes.len(), BatchMemo::default());
        }
        self.eval_new_batch(
            field,
            &positions.map(|[x, y, z]| Position {
                x,
                y,
                z,
                mode: Mode::Terrain,
            }),
            u8::MAX,
        )
    }

    pub(super) fn eval_new_batch(
        &mut self,
        id: Id,
        positions: &[Position; 8],
        active: u8,
    ) -> [f32; 8] {
        self.batch_epoch += 1;
        self.eval_batch(id, positions, active, self.batch_epoch)
    }

    fn eval_batch(
        &mut self,
        id: Id,
        positions: &[Position; 8],
        mut active: u8,
        epoch: u64,
    ) -> [f32; 8] {
        if active == 0 {
            return [0.0; 8];
        }
        let previous = self.batch_values[id];
        let (mut result, cached) = if previous.epoch == epoch {
            (previous.values, previous.mask)
        } else {
            ([0.0; 8], 0)
        };
        let requested = active | cached;
        active &= !cached;
        if active == 0 {
            return result;
        }
        let shared = positions[0].mode != Mode::Raw
            && self
                .chunk
                .as_ref()
                .is_some_and(|chunk| chunk.flat.slots[id] != usize::MAX);
        if shared {
            for lane in 0..8 {
                if active & (1 << lane) != 0
                    && let Some(value) = self.get(id, positions[lane])
                {
                    result[lane] = value;
                    active &= !(1 << lane);
                }
            }
            if active == 0 {
                self.batch_values[id] = BatchMemo {
                    epoch,
                    mask: requested,
                    values: result,
                };
                return result;
            }
        }
        let runtime = self.runtime;
        let values = match &runtime.nodes[id] {
            Node::Constant(value) => [*value as f32; 8],
            Node::Binary(kind, a, b) => {
                let a = self.eval_batch(*a, positions, active, epoch);
                let bound = match kind {
                    Binary::Min | Binary::Max => self.node_bound(*b),
                    _ => None,
                };
                let mut needed = active;
                for (lane, value) in a.iter().enumerate() {
                    let skip = match kind {
                        Binary::Mul => *value == 0.0,
                        Binary::Min => bound.is_some_and(|b| *value < b[0]),
                        Binary::Max => bound.is_some_and(|b| *value > b[1]),
                        Binary::Add => false,
                    };
                    if skip {
                        needed &= !(1 << lane);
                    }
                }
                let b = self.eval_batch(*b, positions, needed, epoch);
                std::array::from_fn(|lane| match kind {
                    Binary::Add => a[lane] + b[lane],
                    Binary::Mul => {
                        if a[lane] == 0.0 {
                            0.0
                        } else {
                            a[lane] * b[lane]
                        }
                    }
                    Binary::Min => {
                        if needed & (1 << lane) == 0 {
                            a[lane]
                        } else {
                            a[lane].min(b[lane])
                        }
                    }
                    Binary::Max => {
                        if needed & (1 << lane) == 0 {
                            a[lane]
                        } else {
                            a[lane].max(b[lane])
                        }
                    }
                })
            }
            Node::Mapped(kind, input) => {
                let input = self.eval_batch(*input, positions, active, epoch);
                input.map(|value| match kind {
                    Mapped::Abs => value.abs(),
                    Mapped::Square => value * value,
                    Mapped::Cube => value * value * value,
                    Mapped::HalfNegative => {
                        if value > 0.0 {
                            value
                        } else {
                            value * 0.5
                        }
                    }
                    Mapped::QuarterNegative => {
                        if value > 0.0 {
                            value
                        } else {
                            value * 0.25
                        }
                    }
                    Mapped::Invert => 1.0 / value,
                    Mapped::Squeeze => {
                        let value = value.clamp(-1.0, 1.0);
                        value / 2.0 - value * value * value / 24.0
                    }
                })
            }
            Node::Clamp(input, min, max) => self
                .eval_batch(*input, positions, active, epoch)
                .map(|value| value.clamp(*min as f32, *max as f32)),
            Node::Gradient(from_y, to_y, from, to) => positions.map(|p| {
                lerp(
                    ((p.y as f32 - *from_y as f32) / (*to_y - *from_y) as f32).clamp(0.0, 1.0),
                    *from as f32,
                    *to as f32,
                )
            }),
            Node::Noise(noise, xz, y) => sample_noise(
                runtime,
                *noise,
                positions.map(|p| [f64::from(p.x) * xz, f64::from(p.y) * y, f64::from(p.z) * xz]),
                active,
            ),
            Node::Shift(noise, shift) => {
                let positions = positions.map(|p| {
                    let p = match shift {
                        Shift::All => [p.x, p.y, p.z],
                        Shift::A => [p.x, 0, p.z],
                        Shift::B => [p.z, p.x, 0],
                    };
                    p.map(|v| f64::from(v) * 0.25)
                });
                sample_noise(runtime, *noise, positions, active).map(|v| v * 4.0)
            }
            Node::Shifted(noise, sx, sy, sz, xz, y) => {
                let sx = self.eval_batch(*sx, positions, active, epoch);
                let sy = self.eval_batch(*sy, positions, active, epoch);
                let sz = self.eval_batch(*sz, positions, active, epoch);
                sample_noise(
                    runtime,
                    *noise,
                    std::array::from_fn(|lane| {
                        [
                            f64::from(positions[lane].x) * xz + f64::from(sx[lane]),
                            f64::from(positions[lane].y) * y + f64::from(sy[lane]),
                            f64::from(positions[lane].z) * xz + f64::from(sz[lane]),
                        ]
                    }),
                    active,
                )
            }
            Node::Weird(noise, input, two_dimensional) => {
                let input = self.eval_batch(*input, positions, active, epoch);
                let rarity = input.map(|value| {
                    if *two_dimensional {
                        if value < -0.75 {
                            0.5
                        } else if value < -0.5 {
                            0.75
                        } else if value < 0.5 {
                            1.0
                        } else if value < 0.75 {
                            2.0
                        } else {
                            3.0
                        }
                    } else if value < -0.5 {
                        0.75
                    } else if value < 0.0 {
                        1.0
                    } else if value < 0.5 {
                        1.5
                    } else {
                        2.0
                    }
                });
                let sampled = sample_noise(
                    runtime,
                    *noise,
                    std::array::from_fn(|lane| {
                        [
                            f64::from(positions[lane].x) / f64::from(rarity[lane]),
                            f64::from(positions[lane].y) / f64::from(rarity[lane]),
                            f64::from(positions[lane].z) / f64::from(rarity[lane]),
                        ]
                    }),
                    active,
                );
                std::array::from_fn(|lane| rarity[lane] * sampled[lane].abs())
            }
            Node::Range(input, min, max, inside, outside) => {
                let input = self.eval_batch(*input, positions, active, epoch);
                let mut selected = 0;
                for (lane, value) in input.iter().enumerate() {
                    if *value >= *min as f32 && *value < *max as f32 {
                        selected |= 1 << lane;
                    }
                }
                let inside = self.eval_batch(*inside, positions, active & selected, epoch);
                let outside = self.eval_batch(*outside, positions, active & !selected, epoch);
                std::array::from_fn(|lane| {
                    if selected & (1 << lane) != 0 {
                        inside[lane]
                    } else {
                        outside[lane]
                    }
                })
            }
            Node::Marker(Marker::Interpolated, input) if positions[0].mode == Mode::Terrain => {
                self.interpolate_batch(id, *input, positions, active)
            }
            Node::Marker(Marker::FlatCache, input) if positions[0].mode != Mode::Raw => self
                .eval_new_batch(
                    *input,
                    &positions.map(|p| Position {
                        x: p.x.div_euclid(4) * 4,
                        y: 0,
                        z: p.z.div_euclid(4) * 4,
                        mode: Mode::Lattice,
                    }),
                    active,
                ),
            Node::Marker(Marker::Cache2D, input)
                if positions[0].mode != Mode::Raw && self.node_y_free(*input) =>
            {
                self.eval_new_batch(
                    *input,
                    &positions.map(|p| Position {
                        y: 0,
                        mode: Mode::Lattice,
                        ..p
                    }),
                    active,
                )
            }
            Node::Marker(_, input) => self.eval_batch(*input, positions, active, epoch),
            // Irregular spline/search control flow keeps the existing scalar
            // semantics. Its noise results still populate shared flat caches.
            Node::Spline(_) | Node::FindTop(..) | Node::Blended(_) => std::array::from_fn(|lane| {
                if active & (1 << lane) != 0 {
                    eval(runtime, self, id, positions[lane])
                } else {
                    0.0
                }
            }),
        };
        for lane in 0..8 {
            if active & (1 << lane) != 0 {
                result[lane] = values[lane];
                if shared {
                    self.put(id, positions[lane], values[lane]);
                }
            }
        }
        // Scalar spline/search delegates have a single-position memo. Publishing
        // just the last computed lane is equivalent to eight sequential puts.
        let last = 7 - active.leading_zeros() as usize;
        self.values[id] = Some(Cached {
            position: positions[last],
            value: values[last],
        });
        self.batch_values[id] = BatchMemo {
            epoch,
            mask: requested,
            values: result,
        };
        result
    }

    fn interpolate_batch(
        &mut self,
        id: Id,
        input: Id,
        positions: &[Position; 8],
        active: u8,
    ) -> [f32; 8] {
        let width = self.runtime.cell_width;
        let height = self.runtime.cell_height;
        let mut corners = [[0.0; 8]; 8];
        let mut fractions = [[0.0; 8]; 3];
        let mut previous: Option<Cell> = None;
        for lane in 0..8 {
            if active & (1 << lane) == 0 {
                continue;
            }
            let p = positions[lane];
            // The production 4x2 tile fits one interpolation cell. Reuse
            // its integer origin and corners instead of dividing and probing
            // the node cache eight times; arbitrary batches still cross cells.
            let cell = previous.filter(|cell| {
                (p.x.wrapping_sub(cell.x) as u32) < width as u32
                    && (p.y.wrapping_sub(cell.y) as u32) < height as u32
                    && (p.z.wrapping_sub(cell.z) as u32) < width as u32
            });
            let (x, y, z) = cell.map_or_else(
                || {
                    (
                        p.x.div_euclid(width) * width,
                        p.y.div_euclid(height) * height,
                        p.z.div_euclid(width) * width,
                    )
                },
                |cell| (cell.x, cell.y, cell.z),
            );
            let values = if let Some(values) = cell
                .map(|cell| cell.corners)
                .or_else(|| self.cell(id, x, y, z))
            {
                values
            } else {
                let points = std::array::from_fn(|index| Position {
                    x: x + (index as i32 & 1) * width,
                    y: y + ((index as i32 >> 1) & 1) * height,
                    z: z + ((index as i32 >> 2) & 1) * width,
                    mode: Mode::Lattice,
                });
                let mut missing = 0;
                let mut values = std::array::from_fn(|index| {
                    self.corner(id, points[index]).unwrap_or_else(|| {
                        missing |= 1 << index;
                        0.0
                    })
                });
                let computed = self.eval_new_batch(input, &points, missing);
                for index in 0..8 {
                    if missing & (1 << index) != 0 {
                        values[index] = computed[index];
                        self.put_corner(id, points[index], computed[index]);
                    }
                }
                self.put_cell(
                    id,
                    Cell {
                        x,
                        y,
                        z,
                        corners: values,
                    },
                );
                values
            };
            previous = Some(Cell {
                x,
                y,
                z,
                corners: values,
            });
            fractions[0][lane] = (p.x - x) as f32 / width as f32;
            fractions[1][lane] = (p.y - y) as f32 / height as f32;
            fractions[2][lane] = (p.z - z) as f32 / width as f32;
            for corner in 0..8 {
                corners[corner][lane] = values[corner];
            }
        }
        interpolate_lanes(&corners, &fractions)
    }
}

// Keep arithmetic separate from the irregular corner gather. Inlining this
// into the gather makes LLVM pack corners within a column instead of columns.
#[inline(never)]
fn interpolate_lanes(corners: &[[f32; 8]; 8], fractions: &[[f32; 8]; 3]) -> [f32; 8] {
    let [tx, ty, tz] = fractions;
    let mut values = [0.0; 8];
    for lane in 0..8 {
        let y00 = lerp(ty[lane], corners[0][lane], corners[2][lane]);
        let y10 = lerp(ty[lane], corners[1][lane], corners[3][lane]);
        let y01 = lerp(ty[lane], corners[4][lane], corners[6][lane]);
        let y11 = lerp(ty[lane], corners[5][lane], corners[7][lane]);
        let x0 = lerp(tx[lane], y00, y10);
        let x1 = lerp(tx[lane], y01, y11);
        values[lane] = lerp(tz[lane], x0, x1);
    }
    values
}

fn sample_noise(
    runtime: &DensityRuntime,
    noise: usize,
    positions: [[f64; 3]; 8],
    active: u8,
) -> [f32; 8] {
    if active.count_ones() <= 2 {
        std::array::from_fn(|lane| {
            if active & (1 << lane) != 0 {
                let [x, y, z] = positions[lane];
                runtime.noises[noise].sample(x, y, z)
            } else {
                0.0
            }
        })
    } else {
        runtime.noises[noise].sample_batch(positions)
    }
}

#[cfg(test)]
#[path = "batch_tests.rs"]
mod tests;
