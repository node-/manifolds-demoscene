// Metal mesh shader pair, mirroring shaders/mesh.hlsl. `float4x4` is
// column-major (matches glam `to_cols_array`), so `f.view_proj * v` is M*v.

#include <metal_stdlib>
using namespace metal;

struct Frame {
    float4x4 view_proj;
    float4 light_dir; // xyz
    float4 params;    // x=time, y=master_level, z=object_kind
    float4 audio_bands;
    float4 audio_bands_hi; // bands 4,5
    float4 dimension_drive;
    float4 plane_angles0; // xy, xz, xw, yz
    float4 plane_angles1; // yw, zw, projection_depth, w_scale
    float4 color_a;
    float4 color_b;
    float4 visual;        // exposure, color_mix, dimension_pulse
    float4x4 model_view;
    float4 light_fill; // xyz camera-space dir, w=intensity
    float4 light_misc; // ambient, rim, spec, ao
    float4 material;   // clay rgb
    float4 bg;         // clear colour (CPU side)
};

struct VSIn {
    float3 pos [[attribute(0)]];
    float4 pos4 [[attribute(1)]];
    float3 nrm [[attribute(2)]];
};

struct VSOut {
    float4 pos [[position]];
    float3 nrm;
    float4 bands;
    float3 vpos;
};

static float4 rotate_plane(float4 p, uint a, uint b, float angle) {
    float s = sin(angle);
    float c = cos(angle);
    float pa = p[a];
    float pb = p[b];
    p[a] = pa * c - pb * s;
    p[b] = pa * s + pb * c;
    return p;
}

vertex VSOut vs_main(VSIn in [[stage_in]], constant Frame& f [[buffer(1)]]) {
    VSOut o;
    float4 p4 = in.pos4;
    p4 *= 1.0 + f.dimension_drive * f.visual.z;
    p4.w *= f.plane_angles1.w;
    p4 = rotate_plane(p4, 0, 1, f.plane_angles0.x);
    p4 = rotate_plane(p4, 0, 2, f.plane_angles0.y);
    p4 = rotate_plane(p4, 0, 3, f.plane_angles0.z);
    p4 = rotate_plane(p4, 1, 2, f.plane_angles0.w);
    p4 = rotate_plane(p4, 1, 3, f.plane_angles1.x);
    p4 = rotate_plane(p4, 2, 3, f.plane_angles1.y);

    float depth = max(1.5, f.plane_angles1.z);
    float perspective = depth / max(0.2, depth - p4.w);
    float3 projected = float3(p4.x, p4.y, p4.z) * perspective;

    o.pos = f.view_proj * float4(projected, 1.0);
    o.nrm = in.nrm;
    o.vpos = (f.model_view * float4(projected, 1.0)).xyz;
    o.bands = f.audio_bands;
    return o;
}

fragment float4 fs_main(VSOut in [[stage_in]], constant Frame& f [[buffer(1)]]) {
    // Normal from screen-space derivatives of the *deformed* camera-space
    // position, so it tracks the 4D rotation; flipped to face the viewer.
    float3 vp = in.vpos;
    float3 n = normalize(cross(dfdx(vp), dfdy(vp)));
    float3 v = normalize(-vp);
    n = dot(n, v) < 0.0 ? -n : n;

    float3 key_dir = normalize(f.light_dir.xyz);
    float3 fill_dir = normalize(f.light_fill.xyz);
    float nv = saturate(dot(n, v));
    float key = saturate((dot(n, key_dir) + 0.55) / 1.55);
    key *= key;
    float fill = saturate((dot(n, fill_dir) + 0.3) / 1.3);
    float hemi = mix(0.50, 1.0, n.y * 0.5 + 0.5);
    float3 h = normalize(key_dir + v);
    float spec = pow(saturate(dot(n, h)), 28.0) * f.light_misc.z;
    float rim = pow(1.0 - nv, 3.0) * f.light_misc.y;
    float curv = saturate(length(fwidth(n)) * 5.0);
    float ao = 1.0 - f.light_misc.w * curv;

    float band_mix = saturate(dot(in.bands, float4(0.35, 0.25, 0.22, 0.18)));
    float a = saturate(mix(f.params.y, band_mix, f.visual.y));
    float3 clay = f.material.rgb;
    float3 tint = mix(f.color_a.rgb, f.color_b.rgb, a);
    float3 base = mix(clay, tint, 0.22 * f.visual.y);

    float3 lit = base * (f.light_misc.x * hemi + f.light_dir.w * key * float3(1.0, 0.98, 0.94) + f.light_fill.w * fill * float3(0.85, 0.92, 1.0)) * ao
               + spec + rim;
    lit *= f.visual.x;
    lit = lit / (1.0 + 0.25 * lit);
    return float4(pow(saturate(lit), float3(1.0 / 2.2)), 1.0);
}
