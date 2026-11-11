// One layer of the canvas: a rotated, rounded rectangle showing a texture (or a
// flat/gradient fill), with optional border, drop shadow and color grade.
// Colors are blended premultiplied in linear light.

struct Layer {
    rect: vec4<f32>,         // center x, center y, half width, half height (canvas px)
    params: vec4<f32>,       // rotation (rad), opacity, corner radius, border width
    shadow: vec4<f32>,       // blur, offset y, quad padding, antialias width (canvas px per screen px)
    uv: vec4<f32>,           // u0, v0, u1, v1
    tint_top: vec4<f32>,     // sRGB
    tint_bottom: vec4<f32>,  // sRGB
    border_color: vec4<f32>, // sRGB
    shadow_color: vec4<f32>, // sRGB rgb + strength
    grade: vec4<f32>,        // brightness, contrast, saturation, temperature
    grade2: vec4<f32>,       // lift, gamma, gain, unused
    grade3: vec4<f32>,       // tint, vibrance, fade, split tone
    grade4: vec4<f32>,       // smoothness, sharpness, vignette, unused
    canvas: vec4<f32>,       // width, height, target is sRGB (1/0), has texture (1/0)
    shape: vec4<f32>,        // outline kind, ring thickness, gradient kind (0 linear / 1 radial), gradient angle (rad)
    view: vec4<f32>,         // canvas px -> clip space: x * view.x + view.y, y * view.z + view.w
    // Blurred areas of the source (up to 4): rect (u0, v0, u1, v1) then
    // (radius u, radius v, feather in uv, shape 0 rectangle / 1 ellipse). Radius 0 = unused.
    blur_rect: array<vec4<f32>, 4>,
    blur_params: array<vec4<f32>, 4>,
    // Masked areas of the source (up to 4): rect (u0, v0, u1, v1) then
    // (feather in uv, shape 0 rectangle / 1 ellipse, invert 0/1, edge fade 0..1). Feather 0 = unused.
    mask_rect: array<vec4<f32>, 4>,
    mask_params: array<vec4<f32>, 4>,
};

@group(0) @binding(0) var<uniform> layer: Layer;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) local: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    let corner = vec2<f32>(f32(i & 1u), f32((i >> 1u) & 1u)) * 2.0 - 1.0;
    let extent = layer.rect.zw + vec2<f32>(layer.shadow.z);
    let local = corner * extent;
    let c = cos(layer.params.x);
    let s = sin(layer.params.x);
    let p = vec2<f32>(local.x * c - local.y * s, local.x * s + local.y * c) + layer.rect.xy;
    var out: VsOut;
    out.pos = vec4<f32>(p.x * layer.view.x + layer.view.y, p.y * layer.view.z + layer.view.w, 0.0, 1.0);
    out.local = local;
    return out;
}

fn sd_round_box(p: vec2<f32>, half: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - half + vec2<f32>(r);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

// Signed distances (negative inside) for the shape outlines, in pixels.
fn sd_ellipse(p: vec2<f32>, half: vec2<f32>) -> f32 {
    let k = length(p / half);
    return (k - 1.0) * min(half.x, half.y);
}

fn sd_triangle(p_in: vec2<f32>, half: vec2<f32>) -> f32 {
    // Isosceles, pointing up, filling the box: apex moved to the origin, base below it
    // (screen y grows downwards, so the base is at +2·half.y).
    var p = vec2<f32>(abs(p_in.x), p_in.y + half.y);
    let q = vec2<f32>(half.x, 2.0 * half.y);
    let a = p - q * clamp(dot(p, q) / dot(q, q), 0.0, 1.0);
    let b = p - q * vec2<f32>(clamp(p.x / q.x, 0.0, 1.0), 1.0);
    let s = -sign(q.y);
    let d = min(vec2<f32>(dot(a, a), s * (p.x * q.y - p.y * q.x)), vec2<f32>(dot(b, b), s * (p.y - q.y)));
    return -sqrt(d.x) * sign(d.y);
}

fn sd_star(p_in: vec2<f32>, r: f32) -> f32 {
    // Five-pointed star (after Inigo Quilez).
    let k1 = vec2<f32>(0.809016994375, -0.587785252292);
    let k2 = vec2<f32>(-k1.x, k1.y);
    var p = vec2<f32>(abs(p_in.x), -p_in.y);
    p = p - 2.0 * max(dot(k1, p), 0.0) * k1;
    p = p - 2.0 * max(dot(k2, p), 0.0) * k2;
    p.x = abs(p.x);
    p.y = p.y - r;
    let rf = 0.45;
    let ba = rf * vec2<f32>(-k1.y, k1.x) - vec2<f32>(0.0, 1.0);
    let h = clamp(dot(p, ba) / dot(ba, ba), 0.0, r);
    return length(p - ba * h) * sign(p.y * ba.x - p.x * ba.y);
}

fn sd_hexagon(p_in: vec2<f32>, r: f32) -> f32 {
    let k = vec3<f32>(-0.866025404, 0.5, 0.577350269);
    var p = abs(p_in);
    p = p - 2.0 * min(dot(k.xy, p), 0.0) * k.xy;
    p = p - vec2<f32>(clamp(p.x, -k.z * r, k.z * r), r);
    return length(p) * sign(p.y);
}

fn sd_shape(p: vec2<f32>, half: vec2<f32>, radius: f32) -> f32 {
    let kind = layer.shape.x;
    let m = min(half.x, half.y);
    if (kind < 0.5) {
        return sd_round_box(p, half, radius);
    } else if (kind < 1.5) {
        return sd_ellipse(p, half);
    } else if (kind < 2.5) {
        return sd_triangle(p, half - vec2<f32>(radius)) - radius;
    } else if (kind < 3.5) {
        return sd_star(p, m - radius) - radius;
    } else if (kind < 4.5) {
        return sd_hexagon(p, m * 0.866 - radius) - radius;
    } else if (kind > 5.5) {
        // Frame: an outlined rounded rectangle.
        let t = layer.shape.y * 0.5;
        return abs(sd_round_box(p, half - vec2<f32>(t), max(radius - t, 0.0))) - t;
    }
    return abs(sd_ellipse(p, half - vec2<f32>(layer.shape.y * 0.5))) - layer.shape.y * 0.5;
}

// How much of blur area `i` covers source point `uv` (0..1, soft at the edge).
fn blur_mask(i: i32, uv: vec2<f32>) -> f32 {
    let r = layer.blur_rect[i];
    let p = layer.blur_params[i];
    let feather = max(p.z, 1e-4);
    let c = (r.xy + r.zw) * 0.5;
    let half = max((r.zw - r.xy) * 0.5, vec2<f32>(1e-4));
    var d: f32;
    if (p.w > 0.5) {
        d = (length((uv - c) / half) - 1.0) * min(half.x, half.y);
    } else {
        let q = abs(uv - c) - half;
        d = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0);
    }
    return 1.0 - smoothstep(-feather, feather, d);
}

// How visible mask area `i` leaves source point `uv` (0..1, soft at the edge): 1 inside (or
// outside, if inverted), blended toward fully visible by the clip's own edge fade.
fn mask_visibility(i: i32, uv: vec2<f32>) -> f32 {
    let r = layer.mask_rect[i];
    let p = layer.mask_params[i];
    let feather = max(p.x, 1e-4);
    let c = (r.xy + r.zw) * 0.5;
    let half = max((r.zw - r.xy) * 0.5, vec2<f32>(1e-4));
    var d: f32;
    if (p.y > 0.5) {
        d = (length((uv - c) / half) - 1.0) * min(half.x, half.y);
    } else {
        let q = abs(uv - c) - half;
        d = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0);
    }
    var inside = 1.0 - smoothstep(-feather, feather, d);
    if (p.z > 0.5) {
        inside = 1.0 - inside;
    }
    return mix(1.0, inside, p.w);
}

// Gaussian-weighted 9×9 samples around `uv`, `radius` in uv units.
fn blurred(uv: vec2<f32>, radius: vec2<f32>) -> vec4<f32> {
    var acc = vec4<f32>(0.0);
    var total = 0.0;
    for (var y = -4; y <= 4; y++) {
        for (var x = -4; x <= 4; x++) {
            let o = vec2<f32>(f32(x), f32(y)) / 4.0;
            let w = exp(-dot(o, o) * 2.0);
            let lo = min(layer.uv.xy, layer.uv.zw);
            let hi = max(layer.uv.xy, layer.uv.zw);
            acc = acc + textureSampleLevel(tex, samp, clamp(uv + o * radius, lo, hi), 0.0) * w;
            total = total + w;
        }
    }
    return acc / total;
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

fn to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

// Skin smoothing: an edge-keeping (bilateral) average of the picture around `uv`. Small
// differences (pores, noise) are averaged away, larger ones (edges) are left alone. The reach
// is a fraction of the picture, so the preview and a full-size export look the same.
fn smoothed(uv: vec2<f32>, center: vec4<f32>, amount: f32) -> vec4<f32> {
    let dims = vec2<f32>(textureDimensions(tex));
    let reach = vec2<f32>(dims.y / max(dims.x, 1.0), 1.0) * (0.0022 + 0.0038 * amount);
    let base = sqrt(max(center.rgb, vec3<f32>(0.0)));
    var acc = center;
    var total = 1.0;
    for (var y = -3; y <= 3; y++) {
        for (var x = -3; x <= 3; x++) {
            if (x == 0 && y == 0) {
                continue;
            }
            let o = vec2<f32>(f32(x), f32(y));
            let s = textureSampleLevel(tex, samp, uv + o * reach, 0.0);
            let d = sqrt(max(s.rgb, vec3<f32>(0.0))) - base;
            let w = exp(-dot(o, o) * 0.11 - dot(d, d) * 80.0);
            acc = acc + s * w;
            total = total + w;
        }
    }
    return mix(center, acc / total, clamp(amount * 1.15, 0.0, 1.0));
}

// The fine detail at `uv`: the picture minus its small-radius average (for sharpening).
fn detail(uv: vec2<f32>, center: vec4<f32>) -> vec3<f32> {
    let dims = vec2<f32>(textureDimensions(tex));
    let r = max(vec2<f32>(1.0) / max(dims, vec2<f32>(1.0)), vec2<f32>(dims.y / max(dims.x, 1.0), 1.0) * 0.0011);
    let around = textureSampleLevel(tex, samp, uv + vec2<f32>(r.x, 0.0), 0.0).rgb
        + textureSampleLevel(tex, samp, uv - vec2<f32>(r.x, 0.0), 0.0).rgb
        + textureSampleLevel(tex, samp, uv + vec2<f32>(0.0, r.y), 0.0).rgb
        + textureSampleLevel(tex, samp, uv - vec2<f32>(0.0, r.y), 0.0).rgb;
    return center.rgb - around * 0.25;
}

// Color correction. `Grade::apply` in clip.rs is the CPU copy of this (preset previews):
// change both together. `edge` is the distance from the layer's center (about 1.41 in a corner).
fn apply_grade(rgb_in: vec3<f32>, g: vec4<f32>, g2: vec4<f32>, g3: vec4<f32>, vignette: f32, edge: f32) -> vec3<f32> {
    var rgb = rgb_in;
    if (any(g != vec4<f32>(0.0)) || any(g2.xyz != vec3<f32>(0.0)) || any(g3 != vec4<f32>(0.0)) || vignette != 0.0) {
        let w = vec3<f32>(0.2126, 0.7152, 0.0722);
        // Grade in display space, where these controls feel natural.
        var c = to_srgb(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)));
        c = c + vec3<f32>(g.x * 0.5);
        c = (c - vec3<f32>(0.5)) * (1.0 + g.y) + vec3<f32>(0.5);
        c = mix(vec3<f32>(dot(c, w)), c, 1.0 + g.z);
        // Vibrance: more saturation for the muted colors, less for the ones that are already strong.
        let spread = clamp(max(c.r, max(c.g, c.b)) - min(c.r, min(c.g, c.b)), 0.0, 1.0);
        c = mix(vec3<f32>(dot(c, w)), c, 1.0 + g3.y * (1.0 - spread));
        // White balance: warmth (blue to orange) and tint (green to magenta).
        c = c * vec3<f32>(1.0 + g.w * 0.12, 1.0 + g.w * 0.02, 1.0 - g.w * 0.12) * vec3<f32>(1.0 + g3.x * 0.05, 1.0 - g3.x * 0.09, 1.0 + g3.x * 0.05);
        c = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
        // Lift/gamma/gain: three-way color correction targeting shadows, mids and highlights
        // separately (lift pulls up the darks, gamma reshapes the mids, gain scales the highs).
        c = c + vec3<f32>(g2.x) * (1.0 - c) * (1.0 - c);
        c = pow(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / exp(g2.y)));
        c = clamp(c * (1.0 + g2.z * c), vec3<f32>(0.0), vec3<f32>(1.0));
        // Split tone: cool shadows and warm highlights (or the reverse when negative).
        let l = dot(c, w);
        let sh = (1.0 - l) * (1.0 - l);
        let hi = l * l;
        c = c + g3.w * (vec3<f32>(-0.09, 0.02, 0.08) * sh + vec3<f32>(0.09, 0.01, -0.07) * hi);
        // Fade: raised blacks and slightly lowered whites.
        c = c + g3.z * (vec3<f32>(0.11) - 0.17 * c);
        c = c * (1.0 - vignette * smoothstep(0.6, 1.6, edge) * 0.8);
        rgb = to_linear(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)));
    }
    return rgb;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let half = layer.rect.zw;
    let radius = layer.params.z;
    let border = layer.params.w;
    let opacity = layer.params.y;
    let aa = max(layer.shadow.w, 0.0001);
    let d = sd_shape(in.local, half, radius);
    let cover = clamp(0.5 - d / aa, 0.0, 1.0);

    var texel = vec4<f32>(1.0);
    var mask_vis = 1.0;
    if (layer.canvas.w > 0.5) {
        let f = in.local / max(half, vec2<f32>(0.0001)) * 0.5 + 0.5;
        let suv = mix(layer.uv.xy, layer.uv.zw, f);
        texel = textureSample(tex, samp, suv);
        if (layer.grade4.x > 0.0 || layer.grade4.y > 0.0) {
            let raw = texel;
            if (layer.grade4.x > 0.0) {
                texel = smoothed(suv, raw, layer.grade4.x);
            }
            if (layer.grade4.y > 0.0) {
                texel = vec4<f32>(max(texel.rgb + detail(suv, raw) * layer.grade4.y * 1.6, vec3<f32>(0.0)), texel.a);
            }
        }
        for (var i = 0; i < 4; i++) {
            let radius = layer.blur_params[i].xy;
            if (radius.x > 0.0) {
                let m = blur_mask(i, suv);
                if (m > 0.001) {
                    texel = mix(texel, blurred(suv, radius), m);
                }
            }
        }
        var mask_active = false;
        for (var i = 0; i < 4; i++) {
            if (layer.mask_params[i].x > 0.0) {
                let v = mask_visibility(i, suv);
                mask_vis = select(v, max(mask_vis, v), mask_active);
                mask_active = true;
            }
        }
    }
    // Gradient position: along the angle (CSS-style, 180° = top to bottom) or out from the center.
    let n = in.local / max(half, vec2<f32>(0.0001));
    let dir = vec2<f32>(sin(layer.shape.w), -cos(layer.shape.w));
    var g = clamp(dot(n, dir) * 0.5 + 0.5, 0.0, 1.0);
    if (layer.shape.z > 0.5) {
        g = clamp(length(n), 0.0, 1.0);
    }
    let tint = mix(layer.tint_top, layer.tint_bottom, g);
    var rgb = apply_grade(texel.rgb * to_linear(tint.rgb), layer.grade, layer.grade2, layer.grade3, layer.grade4.z, length(n));
    var a = texel.a * tint.a * mask_vis;

    if (border > 0.0) {
        let inside = clamp(0.5 - (d + border) / aa, 0.0, 1.0);
        rgb = mix(to_linear(layer.border_color.rgb), rgb, inside);
        a = mix(layer.border_color.a, a, inside);
    }

    let alpha = a * cover * opacity;
    var out = vec4<f32>(rgb * alpha, alpha);

    let strength = layer.shadow_color.a;
    if (strength > 0.0) {
        let blur = max(layer.shadow.x, 1.0);
        let ds = sd_shape(in.local - vec2<f32>(0.0, layer.shadow.y), half, radius);
        let sa = strength * opacity * (1.0 - smoothstep(-blur, blur, ds));
        out = out + vec4<f32>(to_linear(layer.shadow_color.rgb) * sa, sa) * (1.0 - alpha);
    }

    if (layer.canvas.z < 0.5 && out.a > 0.0) {
        out = vec4<f32>(to_srgb(out.rgb / out.a) * out.a, out.a);
    }
    return out;
}
