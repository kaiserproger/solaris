//! Local-data overworld climate and multi-noise biome selection.
#[cfg(test)]
mod column_tests;
pub(crate) mod noise;
#[cfg(test)]
mod numeric_bench;
use crate::vanilla_density::{DensityRuntime, RawColumnSampler, raw_y_free};
use serde_json::Value;
use std::{path::Path, sync::Arc};

#[derive(Debug, Clone, Copy)]
pub struct ClimateSample<'a> {
    /// Temperature, humidity, continentalness, erosion, depth, weirdness.
    pub climate: [f32; 6],
    pub quantized: [i64; 6],
    pub biome: &'a str,
}
pub struct VanillaClimate {
    runtime: Arc<DensityRuntime>,
    y_free: Vec<bool>,
    fields: [usize; 6],
    biomes: Vec<String>,
    parameters: Vec<[[i64; 2]; 7]>,
    tree: Node,
}
#[derive(Debug, Clone)]
struct Node {
    bounds: [[i64; 2]; 7],
    leaf: Option<usize>,
    children: Vec<Node>,
}
impl Node {
    fn branch(children: Vec<Self>) -> Self {
        let mut bounds = [[i64::MAX, i64::MIN]; 7];
        for child in &children {
            for (b, c) in bounds.iter_mut().zip(child.bounds) {
                b[0] = b[0].min(c[0]);
                b[1] = b[1].max(c[1]);
            }
        }
        Self {
            bounds,
            leaf: None,
            children,
        }
    }
    /// Visits subtrees nearer than `best.1`; a leaf strictly nearer becomes the
    /// best, so equal distances keep the first leaf in tree order.
    fn search(&self, target: &[i64; 7], best: &mut (Option<usize>, i64)) {
        let mut distance = 0;
        for (bounds, target) in self.bounds.iter().zip(target) {
            let d = (target - bounds[1]).max(bounds[0] - target).max(0);
            distance += d * d;
            // Remaining terms cannot improve the bound.
            if distance >= best.1 {
                return;
            }
        }
        if let Some(id) = self.leaf {
            *best = (Some(id), distance);
        } else {
            for child in &self.children {
                child.search(target, best);
            }
        }
    }
    fn leaves(&self, out: &mut Vec<usize>) {
        match self.leaf {
            Some(id) => out.push(id),
            None => self.children.iter().for_each(|child| child.leaves(out)),
        }
    }
}
fn sort(nodes: &mut [Node], dimension: usize, absolute: bool) {
    nodes.sort_by(|a, b| {
        for n in 0..7 {
            let d = (dimension + n) % 7;
            let ac = (a.bounds[d][0] + a.bounds[d][1]) / 2;
            let bc = (b.bounds[d][0] + b.bounds[d][1]) / 2;
            let order = if absolute {
                ac.abs().cmp(&bc.abs())
            } else {
                ac.cmp(&bc)
            };
            if !order.is_eq() {
                return order;
            }
        }
        std::cmp::Ordering::Equal
    });
}
fn build(mut nodes: Vec<Node>) -> Node {
    if nodes.len() == 1 {
        return nodes.pop().expect("one node");
    }
    if nodes.len() <= 6 {
        nodes.sort_by_key(|n| {
            n.bounds
                .iter()
                .map(|b| ((b[0] + b[1]) / 2).abs())
                .sum::<i64>()
        });
        return Node::branch(nodes);
    }
    let size = 6f64.powf(((nodes.len() as f64 - 0.01).ln() / 6f64.ln()).floor()) as usize;
    let mut selected = Vec::new();
    let mut min_cost = i64::MAX;
    let mut dimension = 0;
    for d in 0..7 {
        sort(&mut nodes, d, false);
        let buckets: Vec<Node> = nodes
            .chunks(size)
            .map(|c| Node::branch(c.to_vec()))
            .collect();
        let cost = buckets
            .iter()
            .flat_map(|b| b.bounds)
            .map(|b| (b[1] - b[0]).abs())
            .sum();
        if cost < min_cost {
            min_cost = cost;
            dimension = d;
            selected = buckets;
        }
    }
    sort(&mut selected, dimension, true);
    Node::branch(selected.into_iter().map(|n| build(n.children)).collect())
}
fn quantize(value: f64) -> i64 {
    ((value as f32) * 10000.0f32) as i64
}
impl VanillaClimate {
    pub fn new(seed: u64, data_root: impl AsRef<Path>) -> Result<Self, String> {
        let supplied = data_root.as_ref();
        let root = if supplied.join("data/minecraft/worldgen").is_dir() {
            supplied.join("data")
        } else {
            supplied.to_path_buf()
        };
        Self::from_runtime(Arc::new(DensityRuntime::new(seed, &root)?), &root)
    }
    pub(crate) fn from_runtime(runtime: Arc<DensityRuntime>, root: &Path) -> Result<Self, String> {
        let names = [
            "temperature",
            "vegetation",
            "continents",
            "erosion",
            "depth",
            "ridges",
        ];
        let mut fields = [0; 6];
        for (field, name) in fields.iter_mut().zip(names) {
            *field = runtime.field(name)?;
        }
        let path = root.join(
            "minecraft/worldgen/multi_noise_biome_source_parameter_list/overworld_expanded.json",
        );
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let json: Value =
            serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        let rows = json["biomes"]
            .as_array()
            .ok_or("expanded overworld parameter list must contain biomes")?;
        let mut biomes = Vec::with_capacity(rows.len());
        let mut leaves = Vec::with_capacity(rows.len());
        let mut parameters = Vec::with_capacity(rows.len());
        for row in rows {
            let biome = row["biome"].as_str().ok_or("biome identifier missing")?;
            let mut bounds = [[0; 2]; 7];
            for (d, name) in [
                "temperature",
                "humidity",
                "continentalness",
                "erosion",
                "depth",
                "weirdness",
                "offset",
            ]
            .iter()
            .enumerate()
            {
                let v = &row["parameters"][name];
                let (min, max) = if let Some(v) = v.as_f64() {
                    (v, v)
                } else {
                    let a = v
                        .as_array()
                        .ok_or_else(|| format!("invalid climate parameter {name}"))?;
                    if a.len() != 2 {
                        return Err(format!("invalid climate interval {name}"));
                    }
                    (
                        a[0].as_f64().ok_or("invalid interval min")?,
                        a[1].as_f64().ok_or("invalid interval max")?,
                    )
                };
                bounds[d] = [quantize(min), quantize(max)];
            }
            parameters.push(bounds);
            leaves.push(Node {
                bounds,
                leaf: Some(biomes.len()),
                children: Vec::new(),
            });
            biomes.push(biome.to_owned());
        }
        if leaves.is_empty() {
            return Err("empty overworld biome parameter list".into());
        }
        let y_free = raw_y_free(&runtime);
        Ok(Self {
            runtime,
            y_free,
            fields,
            biomes,
            parameters,
            tree: build(leaves),
        })
    }
    /// Block coordinates are floored to their containing biome quart, including negatives.
    pub fn sample(&self, x: i32, y: i32, z: i32) -> ClimateSample<'_> {
        self.sample_quart(x.div_euclid(4), y.div_euclid(4), z.div_euclid(4))
    }
    pub fn sample_quart(&self, x: i32, y: i32, z: i32) -> ClimateSample<'_> {
        let climate = self.runtime.sample_fields(
            self.fields,
            x.wrapping_mul(4),
            y.wrapping_mul(4),
            z.wrapping_mul(4),
        );
        self.select(climate, None, false).0
    }
    /// Reuses only proven Y-independent raw density nodes. The previous leaf
    /// only bounds the search: the result is the first nearest leaf in tree
    /// order, exactly as [`Self::sample`] picks it.
    pub(crate) fn column_sampler(&self) -> ClimateColumnSampler<'_> {
        ClimateColumnSampler {
            climate: self,
            density: RawColumnSampler::new(&self.runtime, &self.y_free),
            last: None,
        }
    }
    /// `last` seeds the search with its distance. Kept as the result
    /// (`keep_last`), it wins ties like vanilla's warm start; otherwise it only
    /// prunes subtrees farther than it, and the search still returns the first
    /// nearest leaf in tree order, as without it.
    fn select(
        &self,
        climate: [f32; 6],
        last: Option<usize>,
        keep_last: bool,
    ) -> (ClimateSample<'_>, usize) {
        let quantized = climate.map(|value| (value * 10000.0) as i64);
        let mut target = [0; 7];
        target[..6].copy_from_slice(&quantized);
        let mut best = match last {
            Some(id) => {
                let distance = self.parameters[id]
                    .iter()
                    .zip(target)
                    .map(|(b, t)| {
                        let d = (t - b[1]).max(b[0] - t).max(0);
                        d * d
                    })
                    .sum();
                if keep_last {
                    (Some(id), distance)
                } else {
                    (None, distance + 1)
                }
            }
            None => (None, i64::MAX),
        };
        self.tree.search(&target, &mut best);
        let id = best.0.expect("nonempty biome tree");
        (
            ClimateSample {
                climate,
                quantized,
                biome: &self.biomes[id],
            },
            id,
        )
    }
    /// Retains vanilla's previous nearest leaf, including its strict tie behavior.
    pub fn sampler(&self) -> ClimateSampler<'_> {
        ClimateSampler {
            climate: self,
            density: self.runtime.sampler(),
            last: None,
        }
    }
    pub(crate) fn runtime(&self) -> Arc<DensityRuntime> {
        Arc::clone(&self.runtime)
    }
    /// Router field ids, then each parameter row with its biome in the order
    /// the search tree visits leaves: a scan keeping the first strictly
    /// smaller distance picks the same biome as [`Self::sample`].
    pub(crate) fn program_parts(&self) -> ([usize; 6], Vec<ParameterRow<'_>>) {
        let mut order = Vec::with_capacity(self.parameters.len());
        self.tree.leaves(&mut order);
        let rows = order
            .into_iter()
            .map(|id| (self.parameters[id], self.biomes[id].as_str()))
            .collect();
        (self.fields, rows)
    }
}

/// Quantized min/max per climate parameter (temperature, humidity,
/// continentalness, erosion, depth, weirdness, offset) and the biome.
pub(crate) type ParameterRow<'a> = ([[i64; 2]; 7], &'a str);

pub struct ClimateSampler<'a> {
    climate: &'a VanillaClimate,
    density: crate::vanilla_density::DensitySampler<'a>,
    last: Option<usize>,
}
impl<'a> ClimateSampler<'a> {
    pub fn sample(&mut self, x: i32, y: i32, z: i32) -> ClimateSample<'a> {
        self.sample_quart(x.div_euclid(4), y.div_euclid(4), z.div_euclid(4))
    }
    pub fn sample_quart(&mut self, x: i32, y: i32, z: i32) -> ClimateSample<'a> {
        let climate = self.climate.fields.map(|field| {
            self.density.sample(
                field,
                x.wrapping_mul(4),
                y.wrapping_mul(4),
                z.wrapping_mul(4),
            )
        });
        let (sample, id) = self.climate.select(climate, self.last, true);
        self.last = Some(id);
        sample
    }
}

pub(crate) struct ClimateColumnSampler<'a> {
    climate: &'a VanillaClimate,
    density: RawColumnSampler<'a>,
    last: Option<usize>,
}
impl<'a> ClimateColumnSampler<'a> {
    pub(crate) fn sample(&mut self, x: i32, y: i32, z: i32) -> ClimateSample<'a> {
        let climate = self.density.sample_fields(
            self.climate.fields,
            x.div_euclid(4).wrapping_mul(4),
            y.div_euclid(4).wrapping_mul(4),
            z.div_euclid(4).wrapping_mul(4),
        );
        let (sample, id) = self.climate.select(climate, self.last, false);
        self.last = Some(id);
        sample
    }
}
