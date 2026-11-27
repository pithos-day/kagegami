// キューブマップ（iChannel0）の草原を見回しながら、シャボン玉が浮かんでいく壁紙の例。
// images/aero-meadow.jpg は Blender（Cycles）で描いた 2:1 の正距円筒図法の画像。
// 好きなパノラマに差し替えてよい。ドラッグで見る向きを変えられる。
// @iChannel0 images/aero-meadow.jpg type=cubemap

const float PI = 3.14159265;
const int BUBBLES = 9;

float hash(float n) { return fract(sin(n * 127.1) * 43758.5453); }

// シャボン玉の膜の干渉色。膜の厚さと、見る角度で色が変わる。
vec3 film(float thickness, float cosTheta) {
    float d = thickness * mix(1.0, 0.6, 1.0 - cosTheta);
    return 0.55 + 0.45 * cos(2.0 * PI * (d * 2.4 + vec3(0.0, 0.33, 0.67)));
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    // 見る向き: ゆっくり右へ回る。ドラッグした分だけ回す（一度も押していなければ iMouse は全部 0）。
    float yaw = iTime * 0.02;
    float pitch = 0.12;
    if (iMouse != vec4(0.0)) {
        yaw += (iMouse.x / iResolution.x - 0.5) * 2.0 * PI;
        pitch += (iMouse.y / iResolution.y - 0.5) * 1.2;
    }
    // 画像の中心が -Z、右へ 1/4 進んだ位置が +X、上が +Y。
    vec3 fw = vec3(sin(yaw) * cos(pitch), sin(pitch), -cos(yaw) * cos(pitch));
    vec3 right = normalize(cross(fw, vec3(0.0, 1.0, 0.0)));
    vec3 up = cross(right, fw);
    vec2 p = (2.0 * fragCoord - iResolution.xy) / iResolution.y;
    vec3 rd = normalize(fw * 1.6 + p.x * right + p.y * up);

    vec3 col = texture(iChannel0, rd).rgb;

    // シャボン玉。カメラのまわりの輪の上を、ゆっくり昇っていく。遠いものから順に重ねる。
    float nearest = 1e9;
    vec3 bubble = vec3(0.0);
    float alpha = 0.0;
    for (int i = 0; i < BUBBLES; i++) {
        float fi = float(i);
        float angle = fi / float(BUBBLES) * 2.0 * PI + hash(fi) * 0.6;
        float dist = 3.5 + 4.0 * hash(fi + 3.1);
        float radius = 0.25 + 0.35 * hash(fi + 7.7);
        float rise = mod(hash(fi + 1.9) * 8.0 + iTime * (0.12 + 0.08 * hash(fi + 5.3)), 8.0) - 2.5;
        vec3 c = vec3(sin(angle) * dist, rise, -cos(angle) * dist);
        c.x += 0.3 * sin(iTime * 0.4 + fi);

        // 原点からの光線と球の交わり。
        float b = dot(c, rd);
        float h = b * b - dot(c, c) + radius * radius;
        if (h <= 0.0 || b < 0.0) continue;
        float t = b - sqrt(h);
        if (t >= nearest) continue;
        nearest = t;

        vec3 n = normalize(rd * t - c);
        float cosTheta = clamp(-dot(rd, n), 0.0, 1.0);
        float rim = 1.0 - cosTheta;
        // 正面はほとんど透け、縁ほど景色が映り込む。
        float fresnel = 0.05 + 0.95 * pow(rim, 3.0);
        vec3 reflected = texture(iChannel0, reflect(rd, n)).rgb;
        // 膜は薄いので、透けた景色はほとんど曲がらない。
        vec3 behind = texture(iChannel0, rd).rgb;
        float thickness = 1.0 + 0.6 * sin(n.y * 3.0 + iTime * 0.7 + fi) + 0.3 * hash(fi + 2.2);
        vec3 tint = film(thickness, cosTheta);
        bubble = behind * (1.0 - 0.45 * fresnel)
               + reflected * mix(vec3(1.0), tint, 0.7) * fresnel
               + tint * 0.06 * rim;
        // 縁をなめらかにする（球の縁に近いほど h が 0 に近い）。
        alpha = smoothstep(0.0, 0.002 * radius * radius / max(t, 1.0), h);
    }
    col = mix(col, bubble, alpha);

    fragColor = vec4(col, 1.0);
}
