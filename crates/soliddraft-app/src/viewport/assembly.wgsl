// Assemblies: every part occurrence is an instance of its definition's mesh.
//
// Positions are in the part's frame; each instance places them relative to the scene
// origin, and `view_proj` already includes the move from the origin to the world, so
// large assemblies keep f32 precision near their parts.

struct Globals {
    view_proj: mat4x4<f32>,
    // The eye relative to the scene origin.
    eye: vec4<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) face: u32,
};

struct InstanceIn {
    @location(3) row0: vec4<f32>,
    @location(4) row1: vec4<f32>,
    @location(5) row2: vec4<f32>,
    // row, flags, colour, finish
    @location(6) info: vec4<u32>,
};

const HIDDEN: u32 = 1u;
const SELECTED: u32 = 2u;
const HOVERED: u32 = 4u;
const COLOURED: u32 = 8u;

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) @interpolate(flat) info: vec4<u32>,
    @location(3) @interpolate(flat) face: u32,
};

fn place(p: vec4<f32>, i: InstanceIn) -> vec3<f32> {
    return vec3<f32>(dot(i.row0, p), dot(i.row1, p), dot(i.row2, p));
}

fn shade_vertex(position: vec3<f32>, normal: vec3<f32>, face: u32, i: InstanceIn) -> Out {
    var out: Out;
    out.info = i.info;
    out.face = face;
    if ((i.info.y & HIDDEN) != 0u) {
        // Outside the clip volume: the triangle is dropped.
        out.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        return out;
    }
    let world = place(vec4<f32>(position, 1.0), i);
    out.world = world;
    out.normal = place(vec4<f32>(normal, 0.0), i);
    out.clip = globals.view_proj * vec4<f32>(world, 1.0);
    return out;
}

@vertex
fn vs_main(v: VertexIn, i: InstanceIn) -> Out {
    return shade_vertex(v.position, v.normal, v.face, i);
}

// Culled drawing: instances come from storage through the lists the cull pass wrote.

struct Inst {
    r0: vec4<f32>,
    r1: vec4<f32>,
    r2: vec4<f32>,
    info: vec4<u32>,
};

struct Def {
    sphere: vec4<f32>,
    lo: vec4<f32>,
    hi: vec4<f32>,
    mesh: vec4<u32>,
    dev: vec4<f32>,
    count: vec4<u32>,
};

@group(1) @binding(0) var<storage, read> instances: array<Inst>;
@group(1) @binding(1) var<storage, read> visible: array<u32>;
@group(1) @binding(2) var<storage, read> proxies: array<u32>;
@group(1) @binding(3) var<storage, read> slot_defs: array<u32>;
@group(1) @binding(4) var<storage, read> defs: array<Def>;

fn instance_at(slot: u32) -> InstanceIn {
    let s = instances[slot];
    return InstanceIn(s.r0, s.r1, s.r2, s.info);
}

@vertex
fn vs_culled(v: VertexIn, @builtin(instance_index) n: u32) -> Out {
    return shade_vertex(v.position, v.normal, v.face, instance_at(visible[n]));
}

// A small instance drawn as its definition's bounding box: the unit cube stretched.
fn proxy_position(unit: vec3<f32>, slot: u32) -> vec3<f32> {
    let d = defs[slot_defs[slot]];
    return d.lo.xyz + unit * (d.hi.xyz - d.lo.xyz);
}

@vertex
fn vs_proxy(v: VertexIn, @builtin(instance_index) n: u32) -> Out {
    let slot = proxies[n];
    return shade_vertex(proxy_position(v.position, slot), v.normal, 0u, instance_at(slot));
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return pow(c, vec3<f32>(2.2));
}

fn unpack_srgb(c: u32) -> vec3<f32> {
    let rgb = vec3<f32>(f32(c & 255u), f32((c >> 8u) & 255u), f32((c >> 16u) & 255u)) / 255.0;
    return srgb_to_linear(rgb);
}

@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    let v = normalize(globals.eye.xyz - in.world);
    // The same rig as single parts: key over the left shoulder, fill from the right.
    let key = normalize(vec3<f32>(-0.45, -0.55, 0.75));
    let fill = normalize(vec3<f32>(0.8, 0.3, 0.2));
    let diffuse = 0.62 * max(dot(n, key), 0.0) + 0.22 * max(dot(n, fill), 0.0);
    let h = normalize(key + v);
    var roughness = 0.45;
    var metallic = 0.0;
    var base = srgb_to_linear(vec3<f32>(0.78, 0.80, 0.83));
    let flags = in.info.y;
    if ((flags & COLOURED) != 0u) {
        // The assembly's colour replaces every colour of the part.
        base = unpack_srgb(in.info.z);
        metallic = f32(in.info.w & 255u) / 255.0;
        roughness = f32((in.info.w >> 8u) & 255u) / 255.0;
    }
    let shininess = mix(96.0, 8.0, roughness);
    let spec = mix(0.18, 0.45, metallic) * pow(max(dot(n, h), 0.0), shininess);
    let fresnel = 0.08 * pow(1.0 - max(dot(n, v), 0.0), 3.0);
    let accent = srgb_to_linear(vec3<f32>(0.12, 0.37, 0.82));
    if ((flags & SELECTED) != 0u) {
        base = mix(base, accent, 0.55);
    } else if ((flags & HOVERED) != 0u) {
        base = mix(base, accent, 0.22);
    }
    let shade = 0.36 + diffuse * (1.0 - 0.5 * metallic);
    return vec4<f32>(base * shade + vec3<f32>(spec) + vec3<f32>(fresnel), 1.0);
}

// Id buffer: the occurrence's row plus one, and the face code (kind 1, face plus one).
struct IdOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) id: vec2<u32>,
};

fn id_vertex(position: vec3<f32>, face: u32, i: InstanceIn) -> IdOut {
    var out: IdOut;
    out.id = vec2<u32>(i.info.x + 1u, (1u << 30u) | face);
    if ((i.info.y & HIDDEN) != 0u) {
        out.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        return out;
    }
    out.clip = globals.view_proj * vec4<f32>(place(vec4<f32>(position, 1.0), i), 1.0);
    return out;
}

@vertex
fn vs_id(v: VertexIn, i: InstanceIn) -> IdOut {
    return id_vertex(v.position, v.face + 1u, i);
}

@vertex
fn vs_culled_id(v: VertexIn, @builtin(instance_index) n: u32) -> IdOut {
    return id_vertex(v.position, v.face + 1u, instance_at(visible[n]));
}

// A box stands in for the part: it picks the occurrence, and no face.
@vertex
fn vs_proxy_id(v: VertexIn, @builtin(instance_index) n: u32) -> IdOut {
    let slot = proxies[n];
    return id_vertex(proxy_position(v.position, slot), 0u, instance_at(slot));
}

@fragment
fn fs_id(in: IdOut) -> @location(0) vec2<u32> {
    return in.id;
}
