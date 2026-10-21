// Culling on the GPU: one thread per instance decides whether the camera sees it, and
// whether it is big enough on screen for its mesh, small enough for its bounding box,
// or too small to draw at all. Visible instances are written to compact lists that the
// colour pass then draws with indirect calls.
//
// Each list buffer has two halves. The first is the colour pass's; the second, the id
// pass's, holds only what overlaps the pick window around the cursor, so the id pass
// draws a handful of instances instead of all of them. The draws of the second half
// start where it does, so both passes read the lists the same way.
//
// Each definition has three meshes, one per level (coarse, standard, fine), and so
// three draws and three lists. An instance picks its level from how large each level's
// deviation looks on screen, as `soliddraft_tessellation::select` does on the CPU: it
// moves to a finer level as soon as its error covers more than a pixel, and back to a
// coarser one after its error has stayed under a tenth of a pixel for 120 frames. Its
// choice is kept from frame to frame. A level that is not uploaded yet is drawn as the
// nearest one that is, and marked as wanted so the app can load it.

struct Inst {
    r0: vec4<f32>,
    r1: vec4<f32>,
    r2: vec4<f32>,
    // row, flags, colour, finish
    info: vec4<u32>,
};

struct Def {
    // Bounding sphere in the part's frame: centre and radius.
    sphere: vec4<f32>,
    lo: vec4<f32>,
    hi: vec4<f32>,
    // The standard level: index count (0 until the mesh is uploaded), first index,
    // base vertex, first instance slot.
    mesh: vec4<u32>,
    // Each level's deviation in millimetres, coarsest first; 0 when nothing bounds it.
    dev: vec4<f32>,
    // x: instance count, y: one bit per level that is uploaded.
    count: vec4<u32>,
};

struct Args {
    index_count: u32,
    instance_count: atomic<u32>,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
};

struct Cull {
    view_proj: mat4x4<f32>,
    planes: array<vec4<f32>, 6>,
    // x: pixels per unit of radius at depth 1, y: radius below which an instance is
    // dropped, z: radius below which it is drawn as its box, all in pixels; w: 1 for a
    // perspective camera.
    params: vec4<f32>,
    // x: instance count, y: 1 when there is a pick window, z: definition count, w: the
    // finest level the device budget allows.
    count: vec4<u32>,
    // The pick window in pixels: left, top, right, bottom; and the viewport size.
    pick: vec4<f32>,
    viewport: vec4<f32>,
};

@group(0) @binding(0) var<uniform> cull: Cull;
@group(0) @binding(1) var<storage, read> instances: array<Inst>;
@group(0) @binding(2) var<storage, read> slot_defs: array<u32>;
@group(0) @binding(3) var<storage, read> defs: array<Def>;
@group(0) @binding(4) var<storage, read_write> args: array<Args>;
@group(0) @binding(5) var<storage, read_write> visible: array<u32>;
@group(0) @binding(6) var<storage, read_write> proxy_args: array<Args>;
@group(0) @binding(7) var<storage, read_write> proxies: array<u32>;
// One word per instance, its level and how many frames it has been quiet (unset before
// its first frame); then one word per definition, a bit per level its instances wanted.
@group(0) @binding(8) var<storage, read_write> levels: array<atomic<u32>>;

const HIDDEN: u32 = 1u;
const LEVELS: u32 = 3u;
const STANDARD: u32 = 1u;
const UNSET: u32 = 0xFFFFFFFFu;
const PROMOTE_PX: f32 = 1.0;
const DEMOTE_PX: f32 = 0.1;
const DEMOTE_AFTER: u32 = 120u;

// The coarsest level from `lowest` to `ceiling` whose error is within a pixel, or
// `ceiling` when none is. Standard when nothing bounds the error.
fn coarsest_sharp(dev: vec4<f32>, ppm: f32, lowest: u32, ceiling: u32) -> u32 {
    if (dev[STANDARD] <= 0.0) {
        return min(STANDARD, ceiling);
    }
    for (var l = lowest; l < ceiling; l = l + 1u) {
        if (dev[l] > 0.0 && dev[l] * ppm <= PROMOTE_PX) {
            return l;
        }
    }
    return ceiling;
}

// Moves instance `i`'s level on by one frame and returns it.
fn choose_level(i: u32, dev: vec4<f32>, ppm: f32, ceiling: u32) -> u32 {
    let state = atomicLoad(&levels[i]);
    var level = 0u;
    var quiet = 0u;
    if (state == UNSET) {
        level = coarsest_sharp(dev, ppm, 0u, ceiling);
    } else if (dev[STANDARD] <= 0.0) {
        level = min(STANDARD, ceiling);
    } else {
        level = state & 3u;
        quiet = state >> 2u;
        let now = dev[level] * ppm;
        if (level > ceiling) {
            level = ceiling;
            quiet = 0u;
        } else if (now > PROMOTE_PX && level < ceiling) {
            level = coarsest_sharp(dev, ppm, level, ceiling);
            quiet = 0u;
        } else if (now < DEMOTE_PX && level > 0u) {
            quiet = min(quiet + 1u, DEMOTE_AFTER);
            if (quiet >= DEMOTE_AFTER && dev[level - 1u] * ppm <= PROMOTE_PX) {
                level = level - 1u;
                quiet = 0u;
            }
        } else {
            quiet = 0u;
        }
    }
    atomicStore(&levels[i], level | (quiet << 2u));
    return level;
}

// The uploaded level nearest `level`, coarser first.
fn nearest_uploaded(level: u32, uploaded: u32) -> u32 {
    for (var k = 0u; k < LEVELS; k = k + 1u) {
        if (k <= level && (uploaded & (1u << (level - k))) != 0u) {
            return level - k;
        }
        if (level + k < LEVELS && (uploaded & (1u << (level + k))) != 0u) {
            return level + k;
        }
    }
    return STANDARD;
}

@compute @workgroup_size(256)
fn cull_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if (i >= cull.count.x) {
        return;
    }
    let inst = instances[i];
    if ((inst.info.y & HIDDEN) != 0u) {
        return;
    }
    let def_index = slot_defs[i];
    let d = defs[def_index];
    if (d.mesh.x == 0u) {
        return;
    }
    let p = vec4<f32>(d.sphere.xyz, 1.0);
    let c = vec3<f32>(dot(inst.r0, p), dot(inst.r1, p), dot(inst.r2, p));
    let r = d.sphere.w;
    for (var k = 0u; k < 6u; k = k + 1u) {
        let plane = cull.planes[k];
        if (dot(plane.xyz, c) + plane.w < -r) {
            return;
        }
    }
    let clip = cull.view_proj * vec4<f32>(c, 1.0);
    let w = max(clip.w, 1e-6);
    let px = r * cull.params.x / w;
    if (px < cull.params.y) {
        return;
    }
    // Does the sphere's square on screen touch the pick window? Behind the eye counts
    // as touching, to stay on the safe side.
    var picked = false;
    if (cull.count.y != 0u) {
        let s = vec2<f32>((clip.x / w * 0.5 + 0.5) * cull.viewport.x, (0.5 - clip.y / w * 0.5) * cull.viewport.y);
        picked = clip.w <= 1e-6
            || (s.x + px >= cull.pick.x && s.x - px <= cull.pick.z && s.y + px >= cull.pick.y && s.y - px <= cull.pick.w);
    }
    if (px < cull.params.z) {
        let k = atomicAdd(&proxy_args[0].instance_count, 1u);
        proxies[k] = i;
        if (picked) {
            let j = atomicAdd(&proxy_args[1].instance_count, 1u);
            proxies[cull.count.x + j] = i;
        }
        return;
    }
    // Pixels per millimetre at the near side of the sphere.
    var ppm = cull.params.x;
    if (cull.params.w > 0.5) {
        ppm = cull.params.x / max(clip.w - r, 1.0);
    }
    let want = choose_level(i, d.dev, ppm, cull.count.w);
    if ((d.count.y & (1u << want)) == 0u) {
        atomicOr(&levels[cull.count.x + def_index], 1u << want);
    }
    let level = nearest_uploaded(want, d.count.y);
    let draw = def_index * LEVELS + level;
    let first = LEVELS * d.mesh.w + level * d.count.x;
    let k = atomicAdd(&args[draw].instance_count, 1u);
    visible[first + k] = i;
    if (picked) {
        let j = atomicAdd(&args[LEVELS * cull.count.z + draw].instance_count, 1u);
        visible[LEVELS * cull.count.x + first + j] = i;
    }
}
