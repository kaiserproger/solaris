//! GPU worldgen experiment (feature `gpu`): `final_density` of whole chunks
//! on the GPU (`density.wgsl`, encoder `program_gpu.rs`), returned in the
//! [`VanillaDensity::fill_density_chunk`] layout.
use std::sync::mpsc;
use std::time::Duration;

use wgpu::util::DeviceExt;

use super::VanillaDensity;
use super::program::gpu::{ChunkWords, chunk_words};

const WORKGROUP: u32 = 64;

pub struct GpuDensity {
    device: wgpu::Device,
    queue: wgpu::Queue,
    passes: [wgpu::ComputePipeline; 3],
    layout: wgpu::BindGroupLayout,
    data: wgpu::Buffer,
    program: ChunkWords,
    timestamps: bool,
}

/// Chunk densities, chunk after chunk, and the GPU time of the flat, corner
/// and block passes when the adapter reports timestamps.
pub struct ChunkDensities {
    pub values: Vec<f32>,
    pub pass_ms: Option<[f64; 3]>,
}

impl GpuDensity {
    pub fn new(density: &VanillaDensity) -> Result<Self, String> {
        let program = chunk_words(density)?;
        if program.min_y.rem_euclid(program.cell_height) != 0
            || program.height % program.cell_height != 0
            || 16 % program.cell_width != 0
        {
            return Err("GPU chunk density needs a cell-aligned world".into());
        }
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .map_err(|e| format!("gpu adapter unavailable: {e}"))?;
        let timestamps = adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY);
        let supported = adapter.limits();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("solaris worldgen"),
            required_features: if timestamps {
                wgpu::Features::TIMESTAMP_QUERY
            } else {
                wgpu::Features::empty()
            },
            required_limits: wgpu::Limits {
                max_storage_buffer_binding_size: supported.max_storage_buffer_binding_size,
                max_buffer_size: supported.max_buffer_size,
                max_storage_buffers_per_shader_stage: 5,
                ..wgpu::Limits::default()
            },
            ..Default::default()
        }))
        .map_err(|e| format!("gpu device unavailable: {e}"))?;
        let source = shader_source(program.registers);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chunk density"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let buffer = |ty| wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let storage = |read_only| buffer(wgpu::BufferBindingType::Storage { read_only });
        let entries: Vec<_> = [
            buffer(wgpu::BufferBindingType::Uniform),
            storage(true),
            storage(true),
            storage(false),
            storage(false),
            storage(false),
        ]
        .into_iter()
        .enumerate()
        .map(|(binding, ty)| wgpu::BindGroupLayoutEntry {
            binding: binding as u32,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty,
            count: None,
        })
        .collect();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chunk density"),
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("chunk density"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pass = |entry_point| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry_point),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some(entry_point),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let passes = [pass("flat_pass"), pass("corner_pass"), pass("block_pass")];
        let data = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("chunk density program"),
            contents: &words(&program.words),
            usage: wgpu::BufferUsages::STORAGE,
        });
        Ok(Self {
            device,
            queue,
            passes,
            layout,
            data,
            program,
            timestamps,
        })
    }

    /// Densities of the chunks at block `(min_x, min_z)`.
    pub fn fill_chunks(&self, chunks: &[(i32, i32)]) -> Result<ChunkDensities, String> {
        let p = &self.program;
        let count = u32::try_from(chunks.len()).map_err(|_| "too many chunks")?;
        let side = (16 / p.cell_width + 1) as u32;
        let columns = side * side;
        let levels = (p.height / p.cell_height + 1) as u32;
        let blocks = 256 * p.height as u64;
        let jobs = [
            count * columns,
            count * columns * levels,
            count * 256 * (levels - 1),
        ];
        if jobs.iter().any(|jobs| jobs.div_ceil(WORKGROUP) > 65_535) {
            return Err(format!("{count} chunks exceed one dispatch"));
        }
        let params = [
            p.min_y as u32,
            p.height as u32,
            p.cell_width as u32,
            p.cell_height as u32,
            p.corner[0],
            p.corner[1],
            p.corner[2],
            p.corner[3],
            p.block[0],
            p.block[1],
            p.block[2],
            p.block[3],
            p.tables[0],
            p.tables[1],
            p.tables[2],
            p.tables[3],
            p.slots,
            count,
            side,
            levels,
        ];
        let device = &self.device;
        let init = |label, contents: &[u32], usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: &words(contents),
                usage,
            })
        };
        let scratch = |label, size: u64, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size.max(4),
                usage,
                mapped_at_creation: false,
            })
        };
        let uniform = init("chunk density params", &params, wgpu::BufferUsages::UNIFORM);
        let origins: Vec<u32> = chunks
            .iter()
            .flat_map(|&(x, z)| [x as u32, z as u32])
            .collect();
        let origins = init("chunk origins", &origins, wgpu::BufferUsages::STORAGE);
        let storage = wgpu::BufferUsages::STORAGE;
        let flat = scratch(
            "corner columns",
            u64::from(count * columns * p.corner[3]) * 4,
            storage,
        );
        let corners = scratch(
            "lattice corners",
            u64::from(count * p.slots * levels * columns) * 4,
            storage,
        );
        let density_size = u64::from(count) * blocks * 4;
        let density = scratch(
            "chunk density",
            density_size,
            storage | wgpu::BufferUsages::COPY_SRC,
        );
        let readback = scratch(
            "chunk density readback",
            density_size,
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("chunk density"),
            layout: &self.layout,
            entries: &[&uniform, &self.data, &origins, &flat, &corners, &density]
                .into_iter()
                .enumerate()
                .map(|(binding, buffer)| wgpu::BindGroupEntry {
                    binding: binding as u32,
                    resource: buffer.as_entire_binding(),
                })
                .collect::<Vec<_>>(),
        });
        let queries = self.timestamps.then(|| {
            (
                device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("chunk density passes"),
                    ty: wgpu::QueryType::Timestamp,
                    count: 6,
                }),
                scratch(
                    "pass timestamps",
                    48,
                    wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                ),
                scratch(
                    "pass timestamps readback",
                    48,
                    wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                ),
            )
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        for (index, (pipeline, jobs)) in self.passes.iter().zip(jobs).enumerate() {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("chunk density"),
                timestamp_writes: queries.as_ref().map(|(set, ..)| {
                    wgpu::ComputePassTimestampWrites {
                        query_set: set,
                        beginning_of_pass_write_index: Some(index as u32 * 2),
                        end_of_pass_write_index: Some(index as u32 * 2 + 1),
                    }
                }),
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(jobs.div_ceil(WORKGROUP), 1, 1);
        }
        encoder.copy_buffer_to_buffer(&density, 0, &readback, 0, density_size);
        if let Some((set, resolve, read)) = &queries {
            encoder.resolve_query_set(set, 0..6, resolve, 0);
            encoder.copy_buffer_to_buffer(resolve, 0, read, 0, 48);
        }
        let submission = self.queue.submit([encoder.finish()]);
        let (tx, rx) = mpsc::channel();
        let mut targets = vec![&readback];
        if let Some((_, _, read)) = &queries {
            targets.push(read);
        }
        for target in &targets {
            let tx = tx.clone();
            target
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let _ = tx.send(result);
                });
        }
        device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(Duration::from_secs(60)),
            })
            .map_err(|e| format!("gpu chunk density failed: {e}"))?;
        for _ in &targets {
            rx.recv()
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
        }
        let values = floats(&readback.slice(..).get_mapped_range());
        readback.unmap();
        let pass_ms = queries.map(|(_, _, read)| {
            let ticks: Vec<u64> = read
                .slice(..)
                .get_mapped_range()
                .chunks_exact(8)
                .map(|b| u64::from_le_bytes(b.try_into().expect("8 bytes")))
                .collect();
            let period = f64::from(self.queue.get_timestamp_period());
            std::array::from_fn(|pass| {
                ticks[pass * 2 + 1].saturating_sub(ticks[pass * 2]) as f64 * period / 1e6
            })
        });
        Ok(ChunkDensities { values, pass_ms })
    }
}

/// `density.wgsl` with its private register file sized for `registers`.
fn shader_source(registers: u32) -> String {
    include_str!("density.wgsl").replace("REGISTER_COUNTu", &format!("{registers}u"))
}

fn words(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

fn floats(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().expect("4 bytes")))
        .collect()
}

#[cfg(test)]
#[path = "gpu_tests.rs"]
mod tests;
