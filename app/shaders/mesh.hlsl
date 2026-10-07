// D3D12 mesh shader pair. Matrices arrive column-major (glam `to_cols_array`),
// which matches HLSL's default constant-buffer matrix packing, so `mul(M, v)`
// computes M*v directly.

cbuffer Frame : register(b0)
{
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

struct VSOut
{
    float4 pos : SV_Position;
    float3 nrm : NORMAL;
    float4 bands : TEXCOORD0;
    float3 vpos : TEXCOORD1;
};

float4 rotate_plane(float4 p, uint a, uint b, float angle)
{
    float s = sin(angle);
    float c = cos(angle);
    float pa = p[a];
    float pb = p[b];
    p[a] = pa * c - pb * s;
    p[b] = pa * s + pb * c;
    return p;
}

VSOut VSMain(float3 pos : POSITION0, float4 pos4 : POSITION1, float3 nrm : NORMAL)
{
    VSOut o;
    float4 p4 = pos4;
    p4 *= 1.0 + dimension_drive * visual.z;
    p4.w *= plane_angles1.w;
    p4 = rotate_plane(p4, 0, 1, plane_angles0.x);
    p4 = rotate_plane(p4, 0, 2, plane_angles0.y);
    p4 = rotate_plane(p4, 0, 3, plane_angles0.z);
    p4 = rotate_plane(p4, 1, 2, plane_angles0.w);
    p4 = rotate_plane(p4, 1, 3, plane_angles1.x);
    p4 = rotate_plane(p4, 2, 3, plane_angles1.y);

    float depth = max(1.5, plane_angles1.z);
    float perspective = depth / max(0.2, depth - p4.w);
    float3 projected = float3(p4.x, p4.y, p4.z) * perspective;

    o.pos = mul(view_proj, float4(projected, 1.0));
    o.nrm = nrm;
    o.vpos = mul(model_view, float4(projected, 1.0)).xyz;
    o.bands = audio_bands;
    return o;
}

float4 PSMain(VSOut i) : SV_Target
{
    // Normal from screen-space derivatives of the *deformed* camera-space
    // position, so it tracks the 4D rotation; flipped to face the viewer.
    float3 vp = i.vpos;
    float3 n = normalize(cross(ddx(vp), ddy(vp)));
    float3 v = normalize(-vp);
    n = dot(n, v) < 0.0 ? -n : n;

    // Soft clay: wrapped key + cool fill + hemisphere ambient + rim, in camera space.
    float3 key_dir = normalize(light_dir.xyz);
    float3 fill_dir = normalize(light_fill.xyz);
    float nv = saturate(dot(n, v));
    float key = saturate((dot(n, key_dir) + 0.55) / 1.55);
    key *= key;
    float fill = saturate((dot(n, fill_dir) + 0.3) / 1.3);
    float hemi = lerp(0.50, 1.0, n.y * 0.5 + 0.5);
    float3 h = normalize(key_dir + v);
    float spec = pow(saturate(dot(n, h)), 28.0) * light_misc.z;
    float rim = pow(1.0 - nv, 3.0) * light_misc.y;
    // Curvature darkening: where the normal changes fast, tuck it into shadow.
    float curv = saturate(length(fwidth(n)) * 5.0);
    float ao = 1.0 - light_misc.w * curv;

    float band_mix = saturate(dot(i.bands, float4(0.35, 0.25, 0.22, 0.18)));
    float a = saturate(lerp(params.y, band_mix, visual.y));
    float3 clay = material.rgb;
    float3 tint = lerp(color_a.rgb, color_b.rgb, a);
    float3 base = lerp(clay, tint, 0.22 * visual.y);

    float3 lit = base * (light_misc.x * hemi + light_dir.w * key * float3(1.0, 0.98, 0.94) + light_fill.w * fill * float3(0.85, 0.92, 1.0)) * ao
               + spec + rim;
    lit *= visual.x;
    lit = lit / (1.0 + 0.25 * lit); // gentle shoulder
    return float4(pow(saturate(lit), 1.0 / 2.2), 1.0);
}
