// インクが渦に流されて広がる壁紙。Buffer A（sim.glsl）が前のフレームのインクを運び、
// ここでは色を付けて表示する。渦の動きは common.glsl にまとめてある。
// @bufferA sim.glsl
// @common common.glsl
// @iChannel0 bufferA
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec3 ink = texture(iChannel0, fragCoord / iResolution.xy).rgb;
    vec3 background = vec3(0.02, 0.025, 0.04);
    fragColor = vec4(background + (1.0 - exp(-ink * 1.5)), 1.0);
}
