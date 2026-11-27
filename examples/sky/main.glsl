// 空に浮かぶ鏡の球を、まわりを回りながら眺める壁紙。空は Cubemap A（cube.glsl）が毎フレーム
// キューブマップに描き、ここでは背景と映り込みにそれを読むだけにする。雲の計算は重いが、
// 画面の大きさによらず 1024×1024×6 面ぶんで済み、球どうしの映り込みで何度読んでも安い。
// ドラッグで見る向きを変えられる。
// @cubemapA cube.glsl
// @iChannel0 cubemapA

const float PI = 3.14159265;
const int SPHERES = 4;

// 球 i の中心（xyz）と半径（w）。真ん中に大きな球、そのまわりを小さな球が回る。
vec4 sphere(int i) {
    if (i == 0) return vec4(0.0, 0.0, 0.0, 1.0);
    float fi = float(i);
    float a = iTime * (0.25 + 0.07 * fi) + fi * 2.1;
    float r = 1.6 + 0.35 * fi;
    return vec4(cos(a) * r, 0.45 * sin(iTime * 0.5 + fi * 1.7), sin(a) * r, 0.22 + 0.06 * fi);
}

// 球の色味（映り込みに掛ける）。
vec3 tint(int i) {
    if (i == 0) return vec3(0.95);
    return 0.6 + 0.4 * cos(float(i) * 2.0 + vec3(0.0, 2.1, 4.2));
}

// 光線と球の交わり。いちばん近い球の距離と番号（当たらなければ -1）。
float hit(vec3 ro, vec3 rd, out int id) {
    float nearest = 1e9;
    id = -1;
    for (int i = 0; i < SPHERES; i++) {
        vec4 s = sphere(i);
        vec3 oc = ro - s.xyz;
        float b = dot(oc, rd);
        float h = b * b - dot(oc, oc) + s.w * s.w;
        if (h <= 0.0) continue;
        float t = -b - sqrt(h);
        if (t > 1e-3 && t < nearest) {
            nearest = t;
            id = i;
        }
    }
    return nearest;
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    // カメラ: 球のまわりをゆっくり回る。ドラッグした分だけ回す（一度も押していなければ iMouse は全部 0）。
    float yaw = iTime * 0.05;
    float pitch = 0.15;
    if (iMouse != vec4(0.0)) {
        yaw += (iMouse.x / iResolution.x - 0.5) * 2.0 * PI;
        pitch += (iMouse.y / iResolution.y - 0.5) * 1.2;
    }
    vec3 ro = 5.0 * vec3(sin(yaw) * cos(pitch), sin(pitch), cos(yaw) * cos(pitch));
    vec3 fw = normalize(-ro);
    vec3 right = normalize(cross(fw, vec3(0.0, 1.0, 0.0)));
    vec3 up = cross(right, fw);
    vec2 p = (2.0 * fragCoord - iResolution.xy) / iResolution.y;
    vec3 rd = normalize(fw * 1.8 + p.x * right + p.y * up);

    // 鏡の球で 3 回まで跳ね返し、最後に空（キューブマップ）を読む。
    vec3 weight = vec3(1.0);
    for (int bounce = 0; bounce < 3; bounce++) {
        int id;
        float t = hit(ro, rd, id);
        if (id < 0) break;
        vec4 s = sphere(id);
        ro += rd * t;
        vec3 n = (ro - s.xyz) / s.w;
        float fresnel = 0.6 + 0.4 * pow(1.0 - clamp(-dot(rd, n), 0.0, 1.0), 5.0);
        weight *= tint(id) * fresnel;
        rd = reflect(rd, n);
    }
    vec3 col = weight * texture(iChannel0, rd).rgb;

    // キューブマップは RGBA16F なので、太陽は 1 を超える明るさで描いてある。ここで 0〜1 に収める。
    col = 1.0 - exp(-1.4 * col);
    fragColor = vec4(col, 1.0);
}
