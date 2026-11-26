// examples/ink/ink.glsl の Buffer A。前のフレームのインクを流れに沿って運び、
// 渦の中心から色を足す。ドラッグした場所には白いインクを足す。
// @iChannel0 bufferA
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    vec2 p = (fragCoord - 0.5 * iResolution.xy) / iResolution.y;
    // 止まっていた後などに大きく飛ばないようにする。
    float dt = min(iTimeDelta, 0.05);

    // 流れを逆にたどった位置の、前のフレームのインク（bufferA は自分なので前のフレームになる）。
    vec2 back = flow(p, iTime) * dt * vec2(iResolution.y / iResolution.x, 1.0);
    vec3 ink = texture(iChannel0, uv - back).rgb * exp(-0.12 * dt);

    for (int i = 0; i < 3; i++) {
        vec2 d = p - center(i, iTime);
        ink += inkColor(i) * exp(-dot(d, d) * 900.0) * dt * 6.0;
    }
    if (iMouse.z > 0.0) {
        vec2 d = p - (iMouse.xy - 0.5 * iResolution.xy) / iResolution.y;
        ink += vec3(1.0) * exp(-dot(d, d) * 2500.0) * dt * 12.0;
    }
    fragColor = vec4(ink, 1.0);
}
