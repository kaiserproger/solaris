// GPU worldgen experiment: `final_density` of whole chunks in three passes
// (corner-column caches, lattice corners, blocks). `program_gpu.rs` is the
// encoder and documents the word layout; `evaluate.rs` is the CPU reference.

struct Params {
    // min_y, height, cell_width, cell_height
    geometry: vec4<i32>,
    // corner code: instructions, count, points, flat_len
    corner: vec4<u32>,
    // block code: instructions, count, points, root register
    block: vec4<u32>,
    // octaves, noise record offsets, blended record offsets, slot registers
    tables: vec4<u32>,
    // slot count, chunk count, corner columns per side, corner levels
    shape: vec4<u32>,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> data: array<u32>;
// Chunk minimum block x, z.
@group(0) @binding(2) var<storage, read> chunks: array<vec2<i32>>;
// [chunk][corner column][flat register]
@group(0) @binding(3) var<storage, read_write> flat: array<f32>;
// [chunk][slot][level][corner column]
@group(0) @binding(4) var<storage, read_write> corners: array<f32>;
// [chunk][y - min_y][z][x]
@group(0) @binding(5) var<storage, read_write> density: array<f32>;

const REGISTERS: u32 = REGISTER_COUNTu;
var<private> regs: array<f32, REGISTERS>;
// Block pass: chunk, level, corner x, corner z of the block's cell.
var<private> cell: vec4<u32>;
var<private> cell_t: vec3<f32>;

var<private> GRADIENTS: array<vec3<f32>, 16> = array<vec3<f32>, 16>(
    vec3(1.0, 1.0, 0.0), vec3(-1.0, 1.0, 0.0), vec3(1.0, -1.0, 0.0), vec3(-1.0, -1.0, 0.0),
    vec3(1.0, 0.0, 1.0), vec3(-1.0, 0.0, 1.0), vec3(1.0, 0.0, -1.0), vec3(-1.0, 0.0, -1.0),
    vec3(0.0, 1.0, 1.0), vec3(0.0, -1.0, 1.0), vec3(0.0, 1.0, -1.0), vec3(0.0, -1.0, -1.0),
    vec3(1.0, 1.0, 0.0), vec3(0.0, -1.0, 1.0), vec3(-1.0, 1.0, 0.0), vec3(0.0, -1.0, -1.0),
);

fn f(index: u32) -> f32 {
    return bitcast<f32>(data[index]);
}

// 8.56 fixed point, (lo, hi), modulo 2^64: the integer part is the lattice
// cell modulo the permutation period.
fn mul_wide(a: u32, b: u32) -> vec2<u32> {
    let a0 = a & 0xffffu;
    let a1 = a >> 16u;
    let b0 = b & 0xffffu;
    let b1 = b >> 16u;
    let p00 = a0 * b0;
    let p01 = a0 * b1;
    let p10 = a1 * b0;
    let mid = p01 + p10;
    let mid_carry = select(0u, 0x10000u, mid < p01);
    let lo = p00 + (mid << 16u);
    let lo_carry = select(0u, 1u, lo < p00);
    return vec2(lo, a1 * b1 + (mid >> 16u) + mid_carry + lo_carry);
}

fn add64(a: vec2<u32>, b: vec2<u32>) -> vec2<u32> {
    let lo = a.x + b.x;
    return vec2(lo, a.y + b.y + select(0u, 1u, lo < a.x));
}

// Integer coordinate times the fixed multiplier at word `k`.
fn scaled(v: i32, k: u32) -> vec2<u32> {
    let u = bitcast<u32>(v);
    var r = mul_wide(u, data[k]);
    r.y += u * data[k + 1u];
    if v < 0 {
        r.y -= data[k];
    }
    return r;
}

// A small f32 (|v| < 128) as fixed point.
fn fixed_of(v: f32) -> vec2<u32> {
    let s = v * 16777216.0;
    let hi = floor(s);
    return vec2(u32((s - hi) * 4294967296.0), bitcast<u32>(i32(hi)));
}

fn perm(octave: u32, n: i32) -> i32 {
    let i = u32(n & 255);
    return i32((data[octave + (i >> 2u)] >> ((i & 3u) * 8u)) & 255u);
}

fn grad(hash: i32, x: f32, y: f32, z: f32) -> f32 {
    let g = GRADIENTS[hash & 15];
    return g.x * x + g.y * y + g.z * z;
}

fn fade(v: f32) -> f32 {
    return v * v * v * (v * (v * 6.0 - 15.0) + 10.0);
}

// Vanilla `Mth.lerp` operand order (not WGSL `mix`).
fn lerp(t: f32, a: f32, b: f32) -> f32 {
    return a + t * (b - a);
}

fn octave_offset(id: u32) -> u32 {
    return params.tables.x + id * 70u;
}

const NO_DELTA: u32 = 0xffffffffu;

// Vanilla ImprovedNoise with its Y smear at fixed coordinates (before the
// octave's own offsets). `delta_at` is the host's exact smear step, if any.
fn improved(octave: u32, cx: vec2<u32>, cy: vec2<u32>, cz: vec2<u32>, y_scale: f32, y_max: f32, delta_at: u32) -> f32 {
    let x = add64(cx, vec2(data[octave + 64u], data[octave + 65u]));
    let y = add64(cy, vec2(data[octave + 66u], data[octave + 67u]));
    let z = add64(cz, vec2(data[octave + 68u], data[octave + 69u]));
    let ix = i32(x.y >> 24u);
    let iy = i32(y.y >> 24u);
    let iz = i32(z.y >> 24u);
    let xr = f32((x.y << 8u) | (x.x >> 24u)) * 2.3283064365386963e-10;
    let yr = f32((y.y << 8u) | (y.x >> 24u)) * 2.3283064365386963e-10;
    let zr = f32((z.y << 8u) | (z.x >> 24u)) * 2.3283064365386963e-10;
    var delta = 0.0;
    if delta_at != NO_DELTA {
        delta = f(delta_at);
    } else if y_scale != 0.0 {
        var limited = yr;
        if y_max >= 0.0 && y_max < yr {
            limited = y_max;
        }
        delta = floor(limited / y_scale + 1.0e-7) * y_scale;
    }
    let gy = yr - delta;
    let x0 = perm(octave, ix);
    let x1 = perm(octave, ix + 1);
    let a = perm(octave, x0 + iy);
    let b = perm(octave, x0 + iy + 1);
    let c = perm(octave, x1 + iy);
    let d = perm(octave, x1 + iy + 1);
    let fx = fade(xr);
    let fy = fade(yr);
    let fz = fade(zr);
    let lower = lerp(
        fy,
        lerp(fx, grad(perm(octave, a + iz), xr, gy, zr), grad(perm(octave, c + iz), xr - 1.0, gy, zr)),
        lerp(fx, grad(perm(octave, b + iz), xr, gy - 1.0, zr), grad(perm(octave, d + iz), xr - 1.0, gy - 1.0, zr)),
    );
    let upper = lerp(
        fy,
        lerp(fx, grad(perm(octave, a + iz + 1), xr, gy, zr - 1.0), grad(perm(octave, c + iz + 1), xr - 1.0, gy, zr - 1.0)),
        lerp(fx, grad(perm(octave, b + iz + 1), xr, gy - 1.0, zr - 1.0), grad(perm(octave, d + iz + 1), xr - 1.0, gy - 1.0, zr - 1.0)),
    );
    return lerp(fz, lower, upper);
}

// One Perlin set: `count` levels at `levels`, multipliers at `table`.
fn perlin(levels: u32, count: u32, weight0: f32, table: u32, p: vec3<i32>, shift: vec3<f32>) -> f32 {
    var weight = weight0;
    var sum = 0.0;
    for (var i = 0u; i < count; i++) {
        let octave = data[levels + i * 2u + 1u];
        if octave != 0xffffffffu {
            let k = table + i * 5u;
            let m = f(k + 4u);
            let sx = add64(scaled(p.x, k), fixed_of(shift.x * m));
            let sy = add64(scaled(p.y, k + 2u), fixed_of(shift.y * m));
            let sz = add64(scaled(p.z, k), fixed_of(shift.z * m));
            sum += f(levels + i * 2u) * improved(octave_offset(octave), sx, sy, sz, 0.0, 0.0, NO_DELTA) * weight;
        }
        weight /= 2.0;
    }
    return sum;
}

fn noise_record(id: u32) -> u32 {
    return data[params.tables.y + id];
}

// Level-table words of one variant of noise `id`.
fn noise_stride(id: u32) -> u32 {
    let r = noise_record(id);
    return (data[r + 2u] + data[r + 4u]) * 5u;
}

fn normal(id: u32, table: u32, p: vec3<i32>, shift: vec3<f32>) -> f32 {
    let r = noise_record(id);
    let first = data[r + 2u];
    let second = data[r + 4u];
    let levels = r + 5u;
    let a = perlin(levels, first, f(r + 1u), table, p, shift);
    let b = perlin(levels + first * 2u, second, f(r + 3u), table + first * 5u, p, shift);
    return (a + b) * f(r);
}

// Word of the host smear step of blended octave `o` (min, max, main order).
fn smear_delta(base: u32, o: u32, y: i32) -> u32 {
    let rows = u32(params.geometry.y) + 1u;
    let row = y - params.geometry.x;
    if row < 0 || u32(row) >= rows {
        return NO_DELTA;
    }
    return base + 184u + o * rows + u32(row);
}

fn blended(id: u32, p: vec3<i32>) -> f32 {
    let base = data[params.tables.z + id];
    var main = 0.0;
    var inverse = 1.0;
    for (var i = 0u; i < 8u; i++) {
        let e = base + 136u + i * 6u;
        main += improved(
            octave_offset(data[base + 32u + i]), scaled(p.x, e), scaled(p.y, e + 2u), scaled(p.z, e),
            f(e + 4u), f32(p.y) * f(e + 5u), smear_delta(base, 32u + i, p.y),
        ) * inverse;
        inverse *= 2.0;
    }
    let alpha = (main / 10.0 + 1.0) / 2.0;
    var low = 0.0;
    var high = 0.0;
    inverse = 1.0;
    for (var i = 0u; i < 16u; i++) {
        let e = base + 40u + i * 6u;
        let sx = scaled(p.x, e);
        let sy = scaled(p.y, e + 2u);
        let sz = scaled(p.z, e);
        let y_scale = f(e + 4u);
        let y_max = f32(p.y) * f(e + 5u);
        if alpha < 1.0 {
            low += improved(octave_offset(data[base + i]), sx, sy, sz, y_scale, y_max, smear_delta(base, i, p.y)) * inverse;
        }
        if alpha > 0.0 {
            high += improved(octave_offset(data[base + 16u + i]), sx, sy, sz, y_scale, y_max, smear_delta(base, 16u + i, p.y)) * inverse;
        }
        inverse *= 2.0;
    }
    let t = clamp(alpha, 0.0, 1.0);
    return (low / 512.0 + t * (high / 512.0 - low / 512.0)) / 128.0;
}

fn point_value(at: u32) -> f32 {
    if data[at + 2u] != 0u {
        return regs[data[at + 3u]];
    }
    return f(at + 3u);
}

// Vanilla's f32 cubic spline step over already evaluated point values.
fn spline(points: u32, input: f32, first: u32, count: u32) -> f32 {
    let base = points + first * 4u;
    var next = 0u;
    loop {
        if next >= count || f(base + next * 4u) > input {
            break;
        }
        next++;
    }
    if next == 0u || next == count {
        var edge = base;
        if next != 0u {
            edge = base + (count - 1u) * 4u;
        }
        let v = point_value(edge);
        let derivative = f(edge + 1u);
        if derivative == 0.0 {
            return v;
        }
        return v + derivative * (input - f(edge));
    }
    let a = base + (next - 1u) * 4u;
    let b = base + next * 4u;
    let width = f(b) - f(a);
    let t = (input - f(a)) / width;
    let y1 = point_value(a);
    let y2 = point_value(b);
    let da = f(a + 1u) * width - (y2 - y1);
    let db = -f(b + 1u) * width + (y2 - y1);
    return y1 + t * (y2 - y1) + t * (1.0 - t) * (da + t * (db - da));
}

fn corner(slot: u32, dx: u32, dy: u32, dz: u32) -> f32 {
    let side = params.shape.z;
    let level = cell.y + dy;
    return corners[((cell.x * params.shape.x + slot) * params.shape.w + level) * side * side
        + (cell.w + dz) * side + cell.z + dx];
}

// Y, then X, then Z: the CPU's `interpolate_corners` order.
fn interpolated(slot: u32) -> f32 {
    let y00 = lerp(cell_t.y, corner(slot, 0u, 0u, 0u), corner(slot, 0u, 1u, 0u));
    let y10 = lerp(cell_t.y, corner(slot, 1u, 0u, 0u), corner(slot, 1u, 1u, 0u));
    let y01 = lerp(cell_t.y, corner(slot, 0u, 0u, 1u), corner(slot, 0u, 1u, 1u));
    let y11 = lerp(cell_t.y, corner(slot, 1u, 0u, 1u), corner(slot, 1u, 1u, 1u));
    return lerp(cell_t.z, lerp(cell_t.x, y00, y10), lerp(cell_t.x, y01, y11));
}

// Instructions [start, end) of the code at `insts` (points at `points`).
fn run(insts: u32, points: u32, start: u32, end: u32, p: vec3<i32>) {
    for (var i = start; i < end; i++) {
        let w = insts + i * 8u;
        let op = data[w];
        let a = data[w + 1u];
        let b = data[w + 2u];
        let c = data[w + 3u];
        let d = data[w + 4u];
        let c0 = f(w + 5u);
        let c1 = f(w + 6u);
        let table = data[w + 7u];
        var v = 0.0;
        switch op {
            case 0u: { v = c0; }
            case 1u: { v = regs[a] + regs[b]; }
            case 2u: {
                let left = regs[a];
                if left != 0.0 { v = left * regs[b]; }
            }
            case 3u: { v = min(regs[a], regs[b]); }
            case 4u: { v = max(regs[a], regs[b]); }
            case 5u: { v = abs(regs[a]); }
            case 6u: { v = regs[a] * regs[a]; }
            case 7u: { v = regs[a] * regs[a] * regs[a]; }
            case 8u: { v = select(regs[a] * 0.5, regs[a], regs[a] > 0.0); }
            case 9u: { v = select(regs[a] * 0.25, regs[a], regs[a] > 0.0); }
            case 10u: { v = 1.0 / regs[a]; }
            case 11u: {
                let s = clamp(regs[a], -1.0, 1.0);
                v = s / 2.0 - s * s * s / 24.0;
            }
            case 12u: { v = clamp(regs[a], c0, c1); }
            case 13u: {
                let from_y = f32(bitcast<i32>(a));
                let to_y = f32(bitcast<i32>(b));
                let t = clamp((f32(p.y) - from_y) / (to_y - from_y), 0.0, 1.0);
                v = c0 + t * (c1 - c0);
            }
            case 14u: { v = normal(a, table, p, vec3(0.0)); }
            case 15u: {
                var q = p;
                if b == 1u { q = vec3(p.x, 0, p.z); }
                if b == 2u { q = vec3(p.z, p.x, 0); }
                v = normal(a, table, q, vec3(0.0)) * 4.0;
            }
            case 16u: { v = normal(a, table, p, vec3(regs[b], regs[c], regs[d])); }
            case 17u: {
                let input = regs[b];
                var variant = 0u;
                var rarity = 0.0;
                if c != 0u {
                    if input < -0.75 { variant = 0u; rarity = 0.5; }
                    else if input < -0.5 { variant = 1u; rarity = 0.75; }
                    else if input < 0.5 { variant = 2u; rarity = 1.0; }
                    else if input < 0.75 { variant = 3u; rarity = 2.0; }
                    else { variant = 4u; rarity = 3.0; }
                } else {
                    if input < -0.5 { variant = 0u; rarity = 0.75; }
                    else if input < 0.0 { variant = 1u; rarity = 1.0; }
                    else if input < 0.5 { variant = 2u; rarity = 1.5; }
                    else { variant = 3u; rarity = 2.0; }
                }
                v = rarity * abs(normal(a, table + variant * noise_stride(a), p, vec3(0.0)));
            }
            case 18u: {
                let input = regs[a];
                v = select(regs[c], regs[b], input >= c0 && input < c1);
            }
            case 19u: { v = spline(points, regs[a], b, c); }
            case 20u: { v = blended(a, p); }
            case 21u: { v = interpolated(a); }
            default: {}
        }
        regs[i] = v;
    }
}

@compute @workgroup_size(64)
fn flat_pass(@builtin(global_invocation_id) id: vec3<u32>) {
    let side = params.shape.z;
    let columns = side * side;
    let chunk = id.x / columns;
    if chunk >= params.shape.y {
        return;
    }
    let column = id.x % columns;
    let origin = chunks[chunk];
    let width = params.geometry.z;
    let p = vec3(origin.x + i32(column % side) * width, 0, origin.y + i32(column / side) * width);
    let flat_len = params.corner.w;
    run(params.corner.x, params.corner.z, 0u, flat_len, p);
    let base = id.x * flat_len;
    for (var i = 0u; i < flat_len; i++) {
        flat[base + i] = regs[i];
    }
}

@compute @workgroup_size(64)
fn corner_pass(@builtin(global_invocation_id) id: vec3<u32>) {
    let side = params.shape.z;
    let columns = side * side;
    let per_chunk = columns * params.shape.w;
    let chunk = id.x / per_chunk;
    if chunk >= params.shape.y {
        return;
    }
    let rest = id.x % per_chunk;
    let level = rest / columns;
    let column = rest % columns;
    let origin = chunks[chunk];
    let width = params.geometry.z;
    let p = vec3(
        origin.x + i32(column % side) * width,
        params.geometry.x + i32(level) * params.geometry.w,
        origin.y + i32(column / side) * width,
    );
    let flat_len = params.corner.w;
    let base = (chunk * columns + column) * flat_len;
    for (var i = 0u; i < flat_len; i++) {
        regs[i] = flat[base + i];
    }
    run(params.corner.x, params.corner.z, flat_len, params.corner.y, p);
    for (var slot = 0u; slot < params.shape.x; slot++) {
        corners[((chunk * params.shape.x + slot) * params.shape.w + level) * columns + column] =
            regs[data[params.tables.w + slot]];
    }
}

// One invocation per block column and cell level: the cell's vertical blocks.
@compute @workgroup_size(64)
fn block_pass(@builtin(global_invocation_id) id: vec3<u32>) {
    let cells = params.shape.w - 1u;
    let per_chunk = 256u * cells;
    let chunk = id.x / per_chunk;
    if chunk >= params.shape.y {
        return;
    }
    let rest = id.x % per_chunk;
    let level = rest / 256u;
    let column = rest % 256u;
    let lx = column % 16u;
    let lz = column / 16u;
    let width = u32(params.geometry.z);
    let height = u32(params.geometry.w);
    cell = vec4(chunk, level, lx / width, lz / width);
    let origin = chunks[chunk];
    let x = origin.x + i32(lx);
    let z = origin.y + i32(lz);
    let tx = f32(lx % width) / f32(width);
    let tz = f32(lz % width) / f32(width);
    for (var dy = 0u; dy < height; dy++) {
        cell_t = vec3(tx, f32(dy) / f32(height), tz);
        let offset = level * height + dy;
        run(params.block.x, params.block.z, 0u, params.block.y, vec3(x, params.geometry.x + i32(offset), z));
        density[(chunk * u32(params.geometry.y) + offset) * 256u + column] = regs[params.block.w];
    }
}
