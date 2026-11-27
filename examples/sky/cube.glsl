// examples/sky/main.glsl の Cubemap A。動く太陽と流れる雲の空を描く。
// 雲の計算は重いので、毎フレーム描き直すのは 6 面のうち 1 面だけにして、ほかの面は前のフレームの
// 結果（自分を読むと前のフレームになる）をそのまま写す。雲はゆっくり動くので、6 フレームに 1 回で足りる。
// rayDir は、この画素を texture(iChannel0, rayDir) で読むときの向き。
// @iChannel0 cubemapA filter=nearest

float hash(vec2 p) {
    p = fract(p * vec2(0.1031, 0.1030));
    p += dot(p, p.yx + 33.33);
    return fract((p.x + p.y) * p.x);
}

// 値ノイズ（0〜1）。
float noise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash(i), hash(i + vec2(1.0, 0.0)), f.x),
               mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0, 1.0)), f.x), f.y);
}

// 雲の形。オクターブごとに違う速さで流し、形が少しずつ変わるようにする。
float fbm(vec2 p, float t) {
    float v = 0.0;
    float a = 0.5;
    mat2 rot = mat2(0.8, 0.6, -0.6, 0.8);
    for (int i = 0; i < 4; i++) {
        v += a * noise(p + t * float(i + 1) * vec2(0.3, 0.1));
        p = rot * p * 2.02;
        a *= 0.5;
    }
    return v;
}

// rayDir が当たる面（+X, -X, +Y, -Y, +Z, -Z の順に 0〜5）。
int face(vec3 d) {
    vec3 a = abs(d);
    if (a.x >= a.y && a.x >= a.z) return d.x > 0.0 ? 0 : 1;
    if (a.y >= a.z) return d.y > 0.0 ? 2 : 3;
    return d.z > 0.0 ? 4 : 5;
}

void mainCubemap(out vec4 fragColor, in vec2 fragCoord, in vec3 rayOri, in vec3 rayDir) {
    // 最初のフレームは 6 面とも描く。
    if (iFrame > 0 && face(rayDir) != iFrame % 6) {
        fragColor = texture(iChannel0, rayDir);
        return;
    }

    // 太陽: ゆっくり空を一周しながら、高さが変わる。
    float a = iTime * 0.03;
    vec3 sun = normalize(vec3(cos(a), 0.25 + 0.2 * sin(a * 2.0), sin(a)));
    float y = rayDir.y;
    float s = max(dot(rayDir, sun), 0.0);

    // 空のグラデーション。太陽が低いほど、地平線の近くが赤くなる。
    vec3 zenith = vec3(0.12, 0.32, 0.75);
    vec3 horizon = mix(vec3(1.0, 0.55, 0.3), vec3(0.7, 0.82, 0.95), smoothstep(0.0, 0.4, sun.y));
    vec3 col = mix(horizon, zenith, pow(clamp(y, 0.0, 1.0), 0.45));
    col += vec3(1.0, 0.7, 0.4) * pow(s, 12.0) * 0.5;
    // 太陽の円盤。RGBA16F なので 1 を超える明るさのまま残る。
    col += vec3(12.0, 10.0, 8.0) * smoothstep(0.9993, 0.9996, s);

    if (y > 0.0) {
        // 雲: 高さ 1 の平面に写した位置の fbm。風で流れ、形も少しずつ変わる。
        vec2 uv = rayDir.xz / y * 0.8;
        float d = fbm(uv, iTime * 0.05);
        float cover = smoothstep(0.45, 0.75, d) * smoothstep(0.02, 0.25, y);
        // 太陽の側が明るく、厚いところは暗い。
        vec3 cloud = mix(vec3(0.55, 0.58, 0.65), vec3(1.05, 1.0, 0.95), 0.4 + 0.6 * s)
                   * (1.15 - 0.5 * smoothstep(0.55, 0.9, d));
        col = mix(col, cloud, cover);
    } else {
        // 地面: 高さ -1 の平面の草地。まだらな濃淡を付け、遠いほど地平線の色に溶ける。
        vec2 uv = rayDir.xz / -y;
        float grass = noise(uv * 0.7) * 0.6 + noise(uv * 3.1) * 0.4;
        vec3 ground = mix(vec3(0.12, 0.17, 0.08), vec3(0.25, 0.3, 0.12), grass);
        ground *= 0.5 + 0.5 * smoothstep(-0.1, 0.4, sun.y);
        col = mix(horizon * 0.9, ground, smoothstep(0.0, 0.2, -y));
    }
    fragColor = vec4(col, 1.0);
}
