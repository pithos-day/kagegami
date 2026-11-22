// 画像（iChannel0）をゆっくり揺らす壁紙の例。ドラッグした位置から波紋が広がる。
// images/plasma.jpg は ImageMagick の plasma: で作った画像。好きな写真に差し替えてよい。
// @iChannel0 images/plasma.jpg wrap=clamp

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    // 画像の縦横比を保ったまま、画面いっぱいに広げる（はみ出した分は切る）。
    vec2 img = iChannelResolution[0].xy;
    float s = max(iResolution.x / img.x, iResolution.y / img.y);
    vec2 st = (fragCoord - 0.5 * iResolution.xy) / (img * s) + 0.5;

    float t = iTime * 0.3;
    st += 0.008 * vec2(sin(st.y * 9.0 + t), cos(st.x * 7.0 - t * 1.3));

    // 一度も押していなければ iMouse は全部 0。
    if (iMouse != vec4(0.0)) {
        vec2 d = (fragCoord - iMouse.xy) / iResolution.y;
        float r = length(d);
        st += normalize(d + 1e-6) * 0.006 * sin(r * 60.0 - iTime * 6.0) * exp(-r * 4.0);
    }

    fragColor = vec4(texture(iChannel0, st).rgb, 1.0);
}
