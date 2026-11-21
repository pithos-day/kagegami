// Shadertoy 形式のシェーダーの壁紙の例。ドラッグすると波の中心が動く。
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.y;
    vec2 center = iMouse.z > 0.0 || iMouse.x > 0.0
        ? iMouse.xy / iResolution.y
        : vec2(0.5 * iResolution.x / iResolution.y, 0.5);
    float d = length(uv - center);
    float wave = 0.5 + 0.5 * sin(d * 40.0 - iTime * 2.0);
    vec3 col = mix(vec3(0.02, 0.03, 0.06), vec3(0.15, 0.35, 0.6), wave * exp(-d * 2.5));
    fragColor = vec4(col, 1.0);
}
