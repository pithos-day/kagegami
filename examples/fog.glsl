// 組み込みの 3D ノイズで、ゆっくり流れる霧を描く壁紙の例。
// グレーのノイズ（iChannel0）を重ねて霧の濃さにし、RGBA のノイズ（iChannel1）で霧を歪ませる。
// どちらも 32³ で repeat なので、座標がいくら大きくなっても折り返して読める。
// @iChannel0 noise3d-grey
// @iChannel1 noise3d-rgba

// 大きさを半分ずつにして 4 回重ねる。iChannelResolution[0] は (32, 32, 32)。
float fbm(vec3 p) {
    float v = 0.0;
    float a = 0.5;
    for (int i = 0; i < 4; i++) {
        v += a * texture(iChannel0, p / iChannelResolution[0]).r;
        p = p * 2.03 + vec3(1.7, 9.2, 3.1);
        a *= 0.5;
    }
    return v / 0.9375;
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.y;
    float t = iTime * 0.05;

    // RGBA のノイズの 2 つのチャンネルで、霧を読む位置をずらす。
    vec3 q = vec3(uv * 6.0, t * 8.0);
    vec2 warp = texture(iChannel1, q / 64.0).rg - 0.5;
    vec3 p = vec3(uv * 10.0 + warp * 3.0 + vec2(t * 20.0, 0.0), t * 12.0);

    float fog = smoothstep(0.3, 0.8, fbm(p));
    vec3 deep = vec3(0.03, 0.04, 0.07);
    vec3 mist = vec3(0.45, 0.52, 0.62);
    vec3 col = mix(deep, mist, fog);
    // 画面の上ほど少し明るくする。
    col += 0.06 * fragCoord.y / iResolution.y;
    fragColor = vec4(col, 1.0);
}
