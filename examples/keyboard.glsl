// @iChannel0 keyboard
// キーボードの入力の例。押しているキーの番号（keyCode）の位置に縦の帯を出す。
// 下の段は押している間、中の段は押したフレームだけ、上の段は押すたびに切り替わる。
// キーが届くのは、壁紙にキーボードのフォーカスがあるとき（壁紙をクリックしたときなど）だけ。
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    int key = int(uv.x * 256.0);
    int row = int(uv.y * 3.0);
    float on = texelFetch(iChannel0, ivec2(key, row), 0).x;
    vec3 base = vec3(0.03, 0.04, 0.07) + 0.02 * float(key % 2);
    vec3 lit = row == 0 ? vec3(0.2, 0.6, 1.0) : row == 1 ? vec3(1.0, 0.9, 0.3) : vec3(0.4, 1.0, 0.5);
    fragColor = vec4(mix(base, lit, on), 1.0);
}
