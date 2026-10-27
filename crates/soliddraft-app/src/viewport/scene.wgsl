// Scene shading, shadows, ambient occlusion, B-rep edges, the id buffer and the final
// blit into the egui pass.
//
// Depth is reversed: the near plane is 1, infinity 0, and the depth tests are
// GreaterEqual. Lighting is linear and tone mapped once per fragment with Khronos PBR
// Neutral; the selection and hover tints are mixed in after tone mapping so they read
// the same under every preset.

struct Globals {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    inv_proj: mat4x4<f32>,
    // Shadow light: world to shadow-map clip space.
    sun_view_proj: mat4x4<f32>,
    // xyz: camera position. w: 1 for an orthographic camera.
    eye: vec4<f32>,
    // x, y: viewport size in pixels. z: edge width in pixels. w: pixels per point.
    viewport: vec4<f32>,
    // x: how far lines are pulled toward the viewer, as a fraction of their distance
    // (perspective). y: the same as a world distance (orthographic). z: occlusion radius
    // in world units. w: occlusion depth bias.
    pull: vec4<f32>,
    // Studio rig, which follows the camera: xyz toward the light, w intensity.
    key: vec4<f32>,
    fill: vec4<f32>,
    rim: vec4<f32>,
    // The shadow light, fixed above the model: xyz toward the light, w intensity.
    sun: vec4<f32>,
    // xyz: centre of the ground under the model. w: radius of the shadow on it.
    ground: vec4<f32>,
    // x: strength. y: filter radius in shadow-map uv. z: 1 when on. w: world size of a
    // shadow-map texel.
    shadow: vec4<f32>,
    // x: environment light. y: reflections. z: occlusion strength. w: roughest mip.
    look: vec4<f32>,
    // Diffuse environment light as nine spherical-harmonic coefficients (environment.rs).
    sh: array<vec4<f32>, 9>,
    hover_id: u32,
    selected_id: u32,
    selected_feature: u32,
    _pad1: u32,
};

// One appearance (soliddraft-materials), metallic-roughness in linear colour.
struct Shade {
    // rgb: base colour. a: opacity.
    base: vec4<f32>,
    emissive: vec4<f32>,
    // x: metallic. y: roughness.
    params: vec4<f32>,
};

struct Body {
    // bit 0: whole body selected, bit 1: ghost preview, bit 2: preview result, bit 3: ghost removes material
    flags: u32,
    _p0: u32,
    _p1: u32,
    _p2: u32,
    tint: vec4<f32>,
    // The body's appearances; each face vertex carries an index into them.
    shades: array<Shade, 64>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var shadow_map: texture_depth_2d;
@group(0) @binding(2) var shadow_sampler: sampler_comparison;
@group(0) @binding(3) var env_map: texture_cube<f32>;
@group(0) @binding(4) var env_sampler: sampler;
@group(1) @binding(0) var<uniform> body: Body;
@group(2) @binding(0) var ao_map: texture_2d<f32>;

const PI: f32 = 3.14159265;

struct FaceIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) face_id: u32,
    @location(3) feature_id: u32,
    @location(4) appearance: u32,
};

struct FaceOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) @interpolate(flat) face_id: u32,
    @location(3) @interpolate(flat) feature_id: u32,
    @location(4) @interpolate(flat) appearance: u32,
};

@vertex
fn vs_face(in: FaceIn) -> FaceOut {
    var out: FaceOut;
    out.clip = globals.view_proj * vec4<f32>(in.position, 1.0);
    out.world = in.position;
    out.normal = in.normal;
    out.face_id = in.face_id;
    out.feature_id = in.feature_id;
    out.appearance = in.appearance;
    return out;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return pow(c, vec3<f32>(2.2));
}

// Khronos PBR Neutral tone mapping (https://github.com/KhronosGroup/ToneMapping).
fn pbr_neutral(color_in: vec3<f32>) -> vec3<f32> {
    let start = 0.8 - 0.04;
    let desaturation = 0.15;
    var color = color_in;
    let x = min(color.r, min(color.g, color.b));
    var offset = 0.04;
    if (x < 0.08) {
        offset = x - 6.25 * x * x;
    }
    color = color - offset;
    let peak = max(color.r, max(color.g, color.b));
    if (peak < start) {
        return color;
    }
    let d = 1.0 - start;
    let new_peak = 1.0 - d * d / (peak + d - start);
    color = color * (new_peak / peak);
    let g = 1.0 - 1.0 / (desaturation * (peak - new_peak) + 1.0);
    return mix(color, vec3<f32>(new_peak), g);
}

fn sh_irradiance(n: vec3<f32>) -> vec3<f32> {
    let s = globals.sh;
    var c = s[0].xyz * 0.282095;
    c += s[1].xyz * (0.488603 * n.y) + s[2].xyz * (0.488603 * n.z) + s[3].xyz * (0.488603 * n.x);
    c += s[4].xyz * (1.092548 * n.x * n.y) + s[5].xyz * (1.092548 * n.y * n.z);
    c += s[6].xyz * (0.315392 * (3.0 * n.z * n.z - 1.0)) + s[7].xyz * (1.092548 * n.x * n.z);
    c += s[8].xyz * (0.546274 * (n.x * n.x - n.y * n.y));
    return max(c, vec3<f32>(0.0));
}

// GGX specular for one light, already multiplied by n.l and π so a light of intensity
// 1 on a white Lambertian surface facing it gives 1.
fn direct(n: vec3<f32>, v: vec3<f32>, l: vec3<f32>, diffuse: vec3<f32>, f0: vec3<f32>, roughness: f32) -> vec3<f32> {
    let nl = max(dot(n, l), 0.0);
    if (nl <= 0.0) {
        return vec3<f32>(0.0);
    }
    let h = normalize(l + v);
    let nh = max(dot(n, h), 0.0);
    let nv = max(dot(n, v), 1e-3);
    let vh = max(dot(v, h), 0.0);
    let a = max(roughness * roughness, 0.002);
    let a2 = a * a;
    let dd = nh * nh * (a2 - 1.0) + 1.0;
    let distribution = a2 / (PI * dd * dd);
    let k = (roughness + 1.0) * (roughness + 1.0) / 8.0;
    let visibility = 1.0 / ((nl * (1.0 - k) + k) * (nv * (1.0 - k) + k) * 4.0);
    let fresnel = f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - vh, 5.0);
    return (diffuse * (vec3<f32>(1.0) - fresnel) + PI * distribution * visibility * fresnel) * nl;
}

// How much of the shadow light reaches `world`, 0 to 1, filtered over a disc.
fn sun_visibility(world: vec3<f32>, n: vec3<f32>) -> f32 {
    if (globals.shadow.z < 0.5) {
        return 1.0;
    }
    // Push the lookup off the surface by a couple of texels against acne.
    let p = world + n * (2.0 * globals.shadow.w);
    let c = globals.sun_view_proj * vec4<f32>(p, 1.0);
    let uv = vec2<f32>(c.x * 0.5 + 0.5, 0.5 - c.y * 0.5);
    let depth = c.z;
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) || depth > 1.0 || depth < 0.0) {
        return 1.0;
    }
    var taps = array<vec2<f32>, 12>(
        vec2<f32>(-0.326, -0.406), vec2<f32>(-0.840, -0.074), vec2<f32>(-0.696, 0.457),
        vec2<f32>(-0.203, 0.621), vec2<f32>(0.962, -0.195), vec2<f32>(0.473, -0.480),
        vec2<f32>(0.519, 0.767), vec2<f32>(0.185, -0.893), vec2<f32>(0.507, 0.064),
        vec2<f32>(0.896, 0.412), vec2<f32>(-0.322, -0.933), vec2<f32>(-0.792, -0.598));
    var lit = 0.0;
    for (var i = 0; i < 12; i++) {
        lit += textureSampleCompareLevel(shadow_map, shadow_sampler, uv + taps[i] * globals.shadow.y, depth);
    }
    return lit / 12.0;
}

fn ambient_occlusion(frag: vec4<f32>) -> f32 {
    if (globals.look.z <= 0.0) {
        return 1.0;
    }
    let ao = textureLoad(ao_map, vec2<i32>(frag.xy), 0).r;
    return mix(1.0, ao, globals.look.z);
}

// Metallic-roughness shading under the studio rig, the environment and the shadow light.
fn shade_surface(world: vec3<f32>, n: vec3<f32>, base: vec3<f32>, metallic: f32, roughness: f32, ao: f32) -> vec3<f32> {
    let v = normalize(globals.eye.xyz - world);
    let f0 = mix(vec3<f32>(0.04), base, metallic);
    let diffuse = base * (1.0 - metallic);
    // The rig and the environment stand in for light from everywhere, so occlusion
    // darkens both; the fixed light has its own shadow.
    var color = vec3<f32>(0.0);
    color += direct(n, v, globals.key.xyz, diffuse, f0, roughness) * globals.key.w * ao;
    color += direct(n, v, globals.fill.xyz, diffuse, f0, roughness) * globals.fill.w * ao;
    color += direct(n, v, globals.rim.xyz, diffuse, f0, roughness) * globals.rim.w * ao;
    if (globals.sun.w > 0.0) {
        color += direct(n, v, globals.sun.xyz, diffuse, f0, roughness) * globals.sun.w * sun_visibility(world, n);
    }
    // Environment: irradiance for the diffuse part, the prefiltered map for reflections,
    // with the split-sum term approximated analytically (Karis, mobile).
    let nv = max(dot(n, v), 0.0);
    let r = reflect(-v, n);
    let prefiltered = textureSampleLevel(env_map, env_sampler, r, roughness * globals.look.w).rgb;
    let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let rr = roughness * c0 + c1;
    let a004 = min(rr.x * rr.x, exp2(-9.28 * nv)) * rr.x + rr.y;
    let ab = vec2<f32>(-1.04, 1.04) * a004 + rr.zw;
    let specular = prefiltered * (f0 * ab.x + ab.y);
    color += (diffuse * sh_irradiance(n) * globals.look.x + specular * globals.look.y) * ao;
    return color;
}

@fragment
fn fs_face(in: FaceOut) -> @location(0) vec4<f32> {
    var n = normalize(in.normal);
    let v = normalize(globals.eye.xyz - in.world);
    // Light the side the viewer sees, even through a sheet or an open preview.
    if (dot(n, v) < 0.0) {
        n = -n;
    }
    let accent = srgb_to_linear(vec3<f32>(0.12, 0.37, 0.82));
    let red = srgb_to_linear(vec3<f32>(0.80, 0.24, 0.24));
    let selected_body = (body.flags & 1u) != 0u;
    let ghost = (body.flags & 2u) != 0u;
    let preview = (body.flags & 4u) != 0u;
    let removes = (body.flags & 8u) != 0u;

    let shade = body.shades[min(in.appearance, 63u)];
    var base = shade.base.rgb;
    let metallic = shade.params.x;
    let roughness = shade.params.y;
    if (ghost && removes) {
        base = mix(base, red, 0.5);
    } else if (ghost) {
        base = mix(base, accent, 0.35);
    }
    if (preview) {
        base = mix(base, accent, 0.10);
    }
    var ao = 1.0;
    if (!ghost) {
        ao = ambient_occlusion(in.clip);
    }
    var color = pbr_neutral(shade_surface(in.world, n, base, metallic, roughness, ao) + shade.emissive.rgb);

    // Selection and hover sit on top of the tone-mapped colour.
    let feature_selected = globals.selected_feature != 0u && in.feature_id == globals.selected_feature && !ghost && !preview;
    let lit_accent = accent * (0.55 + 0.45 * max(dot(n, v), 0.0));
    if (selected_body || (in.face_id != 0u && in.face_id == globals.selected_id)) {
        color = mix(color, lit_accent, 0.55);
    } else if (feature_selected) {
        color = mix(color, lit_accent, 0.40);
    } else if (in.face_id != 0u && in.face_id == globals.hover_id) {
        color = mix(color, lit_accent, 0.22);
    }
    // Opacity blends in draw order; sorting transparent faces comes later.
    var alpha = shade.base.a;
    if (ghost) {
        alpha = 0.42;
    }
    return vec4<f32>(color * alpha, alpha);
}

// Shadow map --------------------------------------------------------------------

@vertex
fn vs_shadow(in: FaceIn) -> @builtin(position) vec4<f32> {
    return globals.sun_view_proj * vec4<f32>(in.position, 1.0);
}

// Ground ------------------------------------------------------------------------
// A disc under the model that only darkens: it catches the shadow and stays clear
// elsewhere, so the background shows through.

struct GroundOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
};

@vertex
fn vs_ground(@builtin(vertex_index) i: u32) -> GroundOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0));
    let c = corners[i];
    let world = globals.ground.xyz + vec3<f32>(c * globals.ground.w, 0.0);
    var out: GroundOut;
    out.clip = globals.view_proj * vec4<f32>(world, 1.0);
    out.world = world;
    return out;
}

@fragment
fn fs_ground(in: GroundOut) -> @location(0) vec4<f32> {
    let d = length(in.world.xy - globals.ground.xy) / globals.ground.w;
    let fade = 1.0 - smoothstep(0.55, 1.0, d);
    // Fade out toward a grazing view, where the disc would read as a hard line.
    let v = normalize(globals.eye.xyz - in.world);
    let grazing = smoothstep(0.02, 0.2, v.z);
    let shadow = 1.0 - sun_visibility(in.world, vec3<f32>(0.0, 0.0, 1.0));
    let alpha = shadow * globals.shadow.x * fade * grazing;
    return vec4<f32>(0.0, 0.0, 0.0, alpha);
}

// Edges ------------------------------------------------------------------------
// Each segment is an instance drawn as a quad of constant width in pixels, so lines
// are wide on WebGL2 too. Ends are extended by half the width so polylines join.

struct LineIn {
    @builtin(vertex_index) corner: u32,
    @location(0) a: vec3<f32>,
    @location(1) b: vec3<f32>,
    @location(2) id: u32,
};

struct LineOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) id: u32,
};

// Clip position of `p` moved toward the viewer, so lines win the depth test against
// the faces they bound. `scale` multiplies the configured pull.
fn pulled_clip(p: vec3<f32>, scale: f32) -> vec4<f32> {
    var v = globals.view * vec4<f32>(p, 1.0);
    if (globals.eye.w > 0.5) {
        v.z = v.z + globals.pull.y * scale;
    } else {
        v = vec4<f32>(v.xyz * (1.0 - globals.pull.x * scale), 1.0);
    }
    return globals.proj * v;
}

fn line_vertex(in: LineIn, width_px: f32) -> vec4<f32> {
    var ca = pulled_clip(in.a, 1.0);
    var cb = pulled_clip(in.b, 1.0);
    // Clip the segment to the space in front of the eye.
    let eps = 1e-5;
    if (ca.w < eps && cb.w < eps) {
        return vec4<f32>(0.0, 0.0, -1.0, 1.0);
    }
    if (ca.w < eps) {
        ca = mix(ca, cb, (eps - ca.w) / (cb.w - ca.w));
    } else if (cb.w < eps) {
        cb = mix(cb, ca, (eps - cb.w) / (ca.w - cb.w));
    }
    let half_size = globals.viewport.xy * 0.5;
    let sa = ca.xy / ca.w * half_size;
    let sb = cb.xy / cb.w * half_size;
    var dir = sb - sa;
    if (dot(dir, dir) < 1e-8) {
        dir = vec2<f32>(1.0, 0.0);
    } else {
        dir = normalize(dir);
    }
    let side = vec2<f32>(-dir.y, dir.x);
    var ends = array<f32, 6>(0.0, 0.0, 1.0, 0.0, 1.0, 1.0);
    var sides = array<f32, 6>(-1.0, 1.0, 1.0, -1.0, 1.0, -1.0);
    let at_b = ends[in.corner] > 0.5;
    var c = ca;
    var along = -1.0;
    if (at_b) {
        c = cb;
        along = 1.0;
    }
    let h = width_px * 0.5;
    let offset = (side * sides[in.corner] + dir * along) * h;
    return vec4<f32>(c.xy + offset / half_size * c.w, c.z, c.w);
}

@vertex
fn vs_edge(in: LineIn) -> LineOut {
    var out: LineOut;
    out.clip = line_vertex(in, globals.viewport.z);
    out.id = in.id;
    return out;
}

@fragment
fn fs_edge(in: LineOut) -> @location(0) vec4<f32> {
    let ghost = (body.flags & 2u) != 0u;
    let removes = (body.flags & 8u) != 0u;
    var color = srgb_to_linear(vec3<f32>(0.16, 0.19, 0.25));
    var alpha = 0.9;
    if (ghost && removes) {
        color = srgb_to_linear(vec3<f32>(0.80, 0.24, 0.24));
        alpha = 0.7;
    } else if (ghost) {
        color = srgb_to_linear(vec3<f32>(0.12, 0.37, 0.82));
        alpha = 0.6;
    }
    return vec4<f32>(color * alpha, alpha);
}

// Id buffer ----------------------------------------------------------------
// Two channels: the occurrence (0 for the open part; assemblies write their own) and
// the entity, whose top two bits are the kind (1 face, 2 edge, 3 vertex) and low 30
// bits the id.

struct IdFaceOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) id: u32,
};

@vertex
fn vs_id_face(in: FaceIn) -> IdFaceOut {
    var out: IdFaceOut;
    out.clip = globals.view_proj * vec4<f32>(in.position, 1.0);
    out.id = (1u << 30u) | in.face_id;
    return out;
}

@fragment
fn fs_id(in: IdFaceOut) -> @location(0) vec2<u32> {
    return vec2<u32>(0u, in.id);
}

@vertex
fn vs_id_edge(in: LineIn) -> IdFaceOut {
    var out: IdFaceOut;
    out.clip = line_vertex(in, max(globals.viewport.z, 2.0));
    out.id = (2u << 30u) | in.id;
    return out;
}

struct IdVertexIn {
    @builtin(vertex_index) corner: u32,
    @location(0) position: vec3<f32>,
    @location(1) vertex_id: u32,
};

// Each vertex becomes a 7-pixel square facing the camera, drawn as two triangles.
@vertex
fn vs_id_vertex(in: IdVertexIn) -> IdFaceOut {
    var out: IdFaceOut;
    let clip = pulled_clip(in.position, 4.0);
    var offsets = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0));
    let px = 3.5 * 2.0 / globals.viewport.xy;
    let o = offsets[in.corner] * px * clip.w;
    out.clip = vec4<f32>(clip.x + o.x, clip.y + o.y, clip.z, clip.w);
    out.id = (3u << 30u) | in.vertex_id;
    return out;
}

// Ambient occlusion (full tier only) ----------------------------------------------
// A depth and view-space normal prepass, a hemisphere sample per pixel, and a
// depth-aware blur that removes the per-pixel rotation noise.

struct NormalOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
};

@vertex
fn vs_normals(in: FaceIn) -> NormalOut {
    var out: NormalOut;
    out.clip = globals.view_proj * vec4<f32>(in.position, 1.0);
    out.normal = (globals.view * vec4<f32>(in.normal, 0.0)).xyz;
    return out;
}

@fragment
fn fs_normals(in: NormalOut) -> @location(0) vec4<f32> {
    var n = normalize(in.normal);
    // View space looks down -Z; turn back faces toward the viewer.
    if (n.z < 0.0) {
        n = -n;
    }
    return vec4<f32>(n * 0.5 + 0.5, 1.0);
}

@group(1) @binding(0) var ao_depth: texture_depth_2d;
@group(1) @binding(1) var ao_input: texture_2d<f32>;

fn view_position(px: vec2<i32>) -> vec3<f32> {
    let d = textureLoad(ao_depth, px, 0);
    let size = globals.viewport.xy;
    let ndc = vec2<f32>((f32(px.x) + 0.5) / size.x * 2.0 - 1.0, 1.0 - (f32(px.y) + 0.5) / size.y * 2.0);
    let p = globals.inv_proj * vec4<f32>(ndc, d, 1.0);
    return p.xyz / p.w;
}

@fragment
fn fs_ssao(in: BlitOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(in.clip.xy);
    if (textureLoad(ao_depth, px, 0) <= 0.0) {
        return vec4<f32>(1.0);
    }
    let p = view_position(px);
    let n = normalize(textureLoad(ao_input, px, 0).xyz * 2.0 - 1.0);
    // Interleaved gradient noise turns the kernel per pixel; the blur averages it out.
    let noise = fract(52.9829189 * fract(dot(in.clip.xy, vec2<f32>(0.06711056, 0.00583715))));
    let angle = noise * 2.0 * PI;
    var t = vec3<f32>(cos(angle), sin(angle), 0.0);
    t = normalize(t - n * dot(t, n) + vec3<f32>(1e-4, 0.0, 0.0));
    let b = cross(n, t);
    let radius = globals.pull.z;
    let size = globals.viewport.xy;
    var occlusion = 0.0;
    let count = 12;
    for (var i = 0; i < count; i++) {
        // Points on a spiral through the hemisphere, denser near the centre.
        let fi = f32(i) + 0.5;
        let r = fi / f32(count);
        let phi = fi * 2.39996;
        let z = sqrt(1.0 - r);
        let rho = sqrt(r);
        let scale = mix(0.15, 1.0, r * r);
        let s = p + (t * (rho * cos(phi)) + b * (rho * sin(phi)) + n * z) * radius * scale;
        let c = globals.proj * vec4<f32>(s, 1.0);
        if (c.w <= 0.0) {
            continue;
        }
        let uv = vec2<f32>(c.x / c.w * 0.5 + 0.5, 0.5 - c.y / c.w * 0.5);
        if (any(uv < vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0))) {
            continue;
        }
        let q = vec2<i32>(uv * size);
        if (textureLoad(ao_depth, q, 0) <= 0.0) {
            continue;
        }
        let scene = view_position(q);
        let range = smoothstep(0.0, 1.0, radius / max(abs(p.z - scene.z), 1e-4));
        if (scene.z >= s.z + globals.pull.w) {
            occlusion += range;
        }
    }
    let ao = clamp(1.0 - 1.6 * occlusion / f32(count), 0.0, 1.0);
    return vec4<f32>(ao, ao, ao, 1.0);
}

@fragment
fn fs_ao_blur(in: BlitOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(in.clip.xy);
    let size = vec2<i32>(globals.viewport.xy);
    let d0 = textureLoad(ao_depth, px, 0);
    if (d0 <= 0.0) {
        return vec4<f32>(1.0);
    }
    let z0 = view_position(px).z;
    var sum = 0.0;
    var weight = 0.0;
    for (var dy = -2; dy <= 2; dy++) {
        for (var dx = -2; dx <= 2; dx++) {
            let q = clamp(px + vec2<i32>(dx, dy), vec2<i32>(0), size - vec2<i32>(1));
            let z = view_position(q).z;
            let w = select(0.0, 1.0, abs(z - z0) < 0.02 * abs(z0) + globals.pull.w);
            sum += textureLoad(ao_input, q, 0).r * w;
            weight += w;
        }
    }
    let ao = sum / max(weight, 1.0);
    return vec4<f32>(ao, ao, ao, 1.0);
}

// Blit into the egui render pass ---------------------------------------------

@group(0) @binding(0) var scene_tex: texture_2d<f32>;
@group(0) @binding(1) var scene_sampler: sampler;
// The modelling space behind the scene: a vertical fade, linear colour, from the
// theme's viewport tokens (theme.rs). The ground disc darkens whatever lies behind it,
// so the shadow lands on the bottom tone.
struct Backdrop {
    top: vec4<f32>,
    bottom: vec4<f32>,
};
@group(0) @binding(2) var<uniform> backdrop: Backdrop;

struct BlitOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_blit(@builtin(vertex_index) i: u32) -> BlitOut {
    var out: BlitOut;
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    out.clip = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>((x + 1.0) * 0.5, 1.0 - (y + 1.0) * 0.5);
    return out;
}

fn composite(uv: vec2<f32>) -> vec3<f32> {
    let scene = textureSample(scene_tex, scene_sampler, uv);
    let bg = mix(backdrop.top.rgb, backdrop.bottom.rgb, uv.y);
    return scene.rgb + bg * (1.0 - scene.a);
}

// For an sRGB swapchain: hand over linear values, the hardware encodes them.
@fragment
fn fs_blit_linear(in: BlitOut) -> @location(0) vec4<f32> {
    return vec4<f32>(composite(in.uv), 1.0);
}

// For a non-sRGB swapchain (egui's default): encode gamma here.
@fragment
fn fs_blit_gamma(in: BlitOut) -> @location(0) vec4<f32> {
    return vec4<f32>(pow(composite(in.uv), vec3<f32>(1.0 / 2.2)), 1.0);
}
