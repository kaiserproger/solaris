//! Native evaluation of the locally exported 26.1.2 overworld density graph.
//! This is noise terrain, not a complete chunk generator: structure beardifier,
//! aquifer block decisions, surface rules and configured carvers are separate stages.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::vanilla_climate::noise::NormalNoise;

mod blended;
mod compile;
mod evaluate;
pub mod program;
#[cfg(feature = "gpu")]
pub mod gpu;

use blended::BlendedNoise;
pub(crate) use evaluate::DensitySampler;
pub(crate) use evaluate::column::{RawColumnSampler, raw_y_free};

type Id = usize;

pub(crate) struct DensityRuntime {
    nodes: Vec<Node>,
    noises: Vec<NormalNoise>,
    blended: Vec<BlendedNoise>,
    fields: HashMap<String, Id>,
    min_y: i32,
    height: i32,
    sea_level: i32,
    cell_width: i32,
    cell_height: i32,
}

enum Node {
    Constant(f64),
    Binary(Binary, Id, Id),
    Mapped(Mapped, Id),
    Clamp(Id, f64, f64),
    Gradient(i32, i32, f64, f64),
    Noise(usize, f64, f64),
    Shift(usize, Shift),
    Shifted(usize, Id, Id, Id, f64, f64),
    Weird(usize, Id, bool),
    Range(Id, f64, f64, Id, Id),
    Spline(Spline),
    Marker(Marker, Id),
    Blended(usize),
    FindTop(Id, Id, i32, i32),
}

#[derive(Clone, Copy)]
enum Binary {
    Add,
    Mul,
    Min,
    Max,
}
#[derive(Clone, Copy)]
enum Mapped {
    Abs,
    Square,
    Cube,
    HalfNegative,
    QuarterNegative,
    Invert,
    Squeeze,
}
#[derive(Clone, Copy)]
enum Shift {
    All,
    A,
    B,
}
#[derive(Clone, Copy)]
enum Marker {
    Interpolated,
    FlatCache,
    Cache2D,
    CacheOnce,
    CacheAllInCell,
    Pass,
}

enum Spline {
    Constant(f32),
    Multipoint {
        coordinate: Id,
        points: Vec<SplinePoint>,
    },
}
struct SplinePoint {
    location: f32,
    value: Spline,
    derivative: f32,
}

impl DensityRuntime {
    pub(crate) fn new(seed: u64, data_root: impl AsRef<Path>) -> Result<Self, String> {
        compile::load(seed, data_root.as_ref())
    }

    pub(crate) fn field(&self, name: &str) -> Result<Id, String> {
        self.fields
            .get(name)
            .copied()
            .ok_or_else(|| format!("Missing overworld noise router field {name}"))
    }

    pub(crate) fn sampler(&self) -> DensitySampler<'_> {
        DensitySampler::new(self)
    }

    /// Evaluates raw RandomState/Climate router fields together without allocating.
    /// Marker wrappers stay transparent; shared values retain their exact bits.
    pub(crate) fn sample_fields<const N: usize>(
        &self,
        fields: [Id; N],
        x: i32,
        y: i32,
        z: i32,
    ) -> [f32; N] {
        evaluate::direct_fields(self, fields, x, y, z)
    }
}

#[derive(Clone)]
pub struct VanillaDensity {
    runtime: Arc<DensityRuntime>,
    final_density: Id,
    sloped_cheese: Id,
    preliminary_surface: Id,
    lod_surface: Option<Id>,
}

/// One bounded interpolation lattice shared by a chunk's lazy halo columns.
pub(crate) struct SurfaceHeightSampler<'a> {
    sampler: DensitySampler<'a>,
    field: Id,
    min_y: i32,
    max_y: i32,
}

impl SurfaceHeightSampler<'_> {
    pub(crate) fn surface_height(&mut self, x: i32, z: i32) -> i32 {
        for y in (self.min_y..self.max_y).rev() {
            if self.sampler.sample_terrain(self.field, x, y, z) > 0.0 {
                return y;
            }
        }
        self.min_y - 1
    }

    /// Terrain density at one block through the region's shared lattice
    /// corners: the same value [`VanillaDensity::density`] computes.
    pub(crate) fn density(&mut self, x: i32, y: i32, z: i32) -> f32 {
        self.sampler.sample_terrain(self.field, x, y, z)
    }
}

impl VanillaDensity {
    pub fn new(seed: u64, data_root: impl AsRef<Path>) -> Result<Self, String> {
        Self::from_runtime(Arc::new(DensityRuntime::new(seed, data_root)?))
    }

    pub(crate) fn from_runtime(runtime: Arc<DensityRuntime>) -> Result<Self, String> {
        Ok(Self {
            final_density: runtime.field("final_density")?,
            sloped_cheese: runtime.field("sloped_cheese")?,
            preliminary_surface: runtime.field("preliminary_surface_level")?,
            lod_surface: runtime.field("lod_surface").ok(),
            runtime,
        })
    }

    /// Far-terrain LOD height: the topmost block where the cave-free
    /// interpolated surface density (`lod_surface`) is positive, on the
    /// column's lattice corner (`x`, `z` rounded down to the cell grid).
    ///
    /// One density evaluation per lattice level, top down, so it costs about
    /// a cell column instead of a chunk. No caves, aquifers, surface rules or
    /// structures. `None` when the router lacks the LOD surface shape.
    pub fn lod_surface_height(&self, x: i32, z: i32) -> Option<i32> {
        let field = self.lod_surface?;
        let (width, height) = (self.runtime.cell_width, self.runtime.cell_height);
        let (x, z) = (x.div_euclid(width) * width, z.div_euclid(width) * width);
        let (min_y, max_y) = (self.min_y(), self.min_y() + self.height());
        let mut sampler = self.runtime.sampler();
        let mut upper = None;
        let mut y = ((max_y - 1).div_euclid(height) + 1) * height;
        while y >= min_y.div_euclid(height) * height {
            let value = sampler.sample_terrain(field, x, y, z);
            if value > 0.0 {
                let Some(upper) = upper else {
                    return Some(max_y - 1);
                };
                // The vanilla Y lerp between this corner and the one above.
                let dy = (0..height)
                    .rev()
                    .find(|dy| value + *dy as f32 / height as f32 * (upper - value) > 0.0)
                    .unwrap_or(0);
                return Some((y + dy).min(max_y - 1));
            }
            upper = Some(value);
            y -= height;
        }
        Some(min_y - 1)
    }

    pub fn min_y(&self) -> i32 {
        self.runtime.min_y
    }
    pub fn height(&self) -> i32 {
        self.runtime.height
    }
    pub fn sea_level(&self) -> i32 {
        self.runtime.sea_level
    }

    pub(crate) fn surface_height_sampler(
        &self,
        min_x: i32,
        min_z: i32,
        max_x: i32,
        max_z: i32,
    ) -> SurfaceHeightSampler<'_> {
        SurfaceHeightSampler {
            sampler: DensitySampler::for_region(&self.runtime, min_x, min_z, max_x, max_z),
            field: self.final_density,
            min_y: self.min_y(),
            max_y: self.min_y() + self.height(),
        }
    }

    /// Top solid block, before aquifers, surfaces, structures and carvers.
    /// Unlike a first-free heightmap this returns the solid block's Y itself.
    pub fn surface_height(&self, x: i32, z: i32) -> i32 {
        let mut sampler = self.runtime.sampler();
        for y in (self.min_y()..self.min_y() + self.height()).rev() {
            if sampler.sample_terrain(self.final_density, x, y, z) > 0.0 {
                return y;
            }
        }
        self.min_y() - 1
    }

    pub fn density(&self, x: i32, y: i32, z: i32) -> f32 {
        evaluate::direct_terrain(&self.runtime, self.final_density, x, y, z)
    }

    /// The terrain's sloped cheese before cave subtraction and vertical slides.
    pub fn initial_density(&self, x: i32, y: i32, z: i32) -> f32 {
        evaluate::direct_terrain(&self.runtime, self.sloped_cheese, x, y, z)
    }

    pub fn preliminary_surface_height(&self, x: i32, z: i32) -> i32 {
        self.runtime
            .sampler()
            .sample_terrain(
                self.preliminary_surface,
                x.div_euclid(4) * 4,
                0,
                z.div_euclid(4) * 4,
            )
            .floor() as i32
    }

    /// Reuses interpolation corners and compiled-node caches across a whole column.
    /// `output[0]` is min_y. No allocation occurs per block.
    pub fn fill_density_column(&self, x: i32, z: i32, output: &mut [f32]) {
        assert_eq!(output.len(), self.height() as usize);
        let mut sampler = self.runtime.sampler();
        for (offset, density) in output.iter_mut().enumerate() {
            *density =
                sampler.sample_terrain(self.final_density, x, self.min_y() + offset as i32, z);
        }
    }

    /// Chunk output is Y-major, then Z, then X: `(y-min_y)*256 + z*16+x`.
    /// Each interpolated node retains the chunk's shared lattice corners;
    /// flat-cache nodes retain quart-column values across all vertical cells.
    /// Eight neighbouring columns (4×2) traverse the DAG together. Branch masks
    /// retain scalar laziness; lane arithmetic preserves scalar fp32 bits.
    pub fn fill_density_chunk(&self, min_x: i32, min_z: i32, output: &mut [f32]) {
        assert_eq!(output.len(), self.height() as usize * 256);
        let mut sampler = DensitySampler::for_chunk(&self.runtime, min_x, min_z);
        let max_y = self.min_y() + self.height();
        let height = self.runtime.cell_height;
        for cell_y in (self.min_y().div_euclid(height)..(max_y - 1).div_euclid(height) + 1).rev() {
            // Keep a quart-cell's vertical work together so the scalar and
            // batched evaluators reuse the same hot interpolation corners.
            for tile_z in (min_z..min_z + 16).step_by(4) {
                for tile_x in (min_x..min_x + 16).step_by(4) {
                    for y in (cell_y * height).max(self.min_y())..((cell_y + 1) * height).min(max_y)
                    {
                        for z in [tile_z, tile_z + 2] {
                            let positions = std::array::from_fn(|lane| {
                                [tile_x + (lane % 4) as i32, y, z + (lane / 4) as i32]
                            });
                            let values =
                                sampler.sample_terrain_batch(self.final_density, positions);
                            let index = (y - self.min_y()) as usize * 256
                                + (z - min_z) as usize * 16
                                + (tile_x - min_x) as usize;
                            output[index..index + 4].copy_from_slice(&values[..4]);
                            output[index + 16..index + 20].copy_from_slice(&values[4..]);
                        }
                    }
                }
            }
        }
    }
}
