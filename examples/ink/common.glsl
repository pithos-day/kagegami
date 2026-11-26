// examples/ink/main.glsl の Common。Buffer A（sim.glsl）と Image の両方の先頭に入る。

// p を中心 c のまわりに回す渦。中心から離れるほど弱くなる。
vec2 swirl(vec2 p, vec2 c, float strength) {
    vec2 d = p - c;
    return strength * vec2(-d.y, d.x) * exp(-dot(d, d) * 6.0);
}

// i 番目の渦と、色を出す点の位置。p は縦が -0.5〜0.5 の座標。
vec2 center(int i, float t) {
    float f = float(i);
    return vec2(0.55 * sin(t * (0.07 + 0.03 * f) + f * 2.1),
                0.32 * cos(t * (0.09 + 0.02 * f) + f * 1.3));
}

// インクを運ぶ流れ（1 秒あたりの移動量）。
vec2 flow(vec2 p, float t) {
    vec2 v = 0.06 * vec2(sin(p.y * 3.0 + t * 0.21), cos(p.x * 2.5 - t * 0.17));
    for (int i = 0; i < 3; i++) {
        v += swirl(p, center(i, t), i == 1 ? -1.6 : 1.6);
    }
    return v;
}

vec3 inkColor(int i) {
    return i == 0 ? vec3(0.1, 0.8, 0.75) : i == 1 ? vec3(0.95, 0.45, 0.15) : vec3(0.55, 0.3, 0.95);
}
