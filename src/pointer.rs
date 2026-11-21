//! ポインターの状態。

/// ドラッグの状態。シェーダーの `iMouse`（#11 で決めた動き）と、Lua の `mouse.press_x`・`press_y`・
/// `dragging`（#13）の元になる。どのボタンでも反応し、ボタンを区別しない。
/// 座標は Lua の `mouse` と同じ出力のピクセル（左上原点）で持ち、`iMouse` にするときに y を反転する。
/// シェーダーに渡すときは `shader::mouse_to_buffer_px` でバッファーのピクセルに換算する。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Drag {
    pos: [f32; 2],
    start: [f32; 2],
    /// ドラッグ中（z が正）。
    down: bool,
    /// 押した後、まだ 1 フレームも描いていない（w が正）。
    clicked: bool,
    /// 一度でも押したか。押す前は Shadertoy と同じく全部 0 にする。
    touched: bool,
}

impl Drag {
    /// どれかのボタンを押した。ほかのボタンを押していても、ここからドラッグを始め直す。
    pub fn pressed(&mut self, x: f32, y: f32) {
        self.pos = [x, y];
        self.start = [x, y];
        self.down = true;
        self.clicked = true;
        self.touched = true;
    }

    /// どれかのボタンを離した、またはカーソルが surface の外に出た（Leave）。
    pub fn released(&mut self) {
        self.down = false;
    }

    /// カーソルが動いた。ドラッグ中だけ位置を更新する（surface の外の座標もそのまま）。
    pub fn moved(&mut self, x: f32, y: f32) {
        if self.down {
            self.pos = [x, y];
        }
    }

    /// 1 フレーム描いた。押した直後の印（w が正）はここで消える。
    pub fn frame_rendered(&mut self) {
        self.clicked = false;
    }

    /// Lua の `mouse.press_x`・`press_y`・`dragging`（#13）。座標は左上原点のまま。
    /// 一度も押していなければ `iMouse` と同じく位置は 0。
    pub fn lua_press(&self) -> ([f32; 2], bool) {
        (self.start, self.down)
    }

    /// `iMouse` の値（出力のピクセル）。y は `output_height - y` にして左下原点にする。
    pub fn imouse(&self, output_height: f32) -> [f32; 4] {
        if !self.touched {
            return [0.0; 4];
        }
        let sign = |b: bool| if b { 1.0 } else { -1.0 };
        [
            self.pos[0],
            output_height - self.pos[1],
            self.start[0] * sign(self.down),
            (output_height - self.start[1]) * sign(self.clicked),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: f32 = 1080.0;

    #[test]
    fn starts_at_zero() {
        // Shadertoy と同じく、一度も押していなければ全部 0（y を反転しない）。
        assert_eq!(Drag::default().imouse(H), [0.0; 4]);
    }

    #[test]
    fn lua_fields_follow_drag() {
        let mut m = Drag::default();
        assert_eq!(m.lua_press(), ([0.0, 0.0], false));
        m.pressed(100.0, 80.0);
        m.moved(150.0, 90.0);
        // 押し始めた位置は動かず、y も反転しない。
        assert_eq!(m.lua_press(), ([100.0, 80.0], true));
        m.released();
        assert_eq!(m.lua_press(), ([100.0, 80.0], false));
    }

    #[test]
    fn press_sets_position_and_click_for_one_frame() {
        let mut m = Drag::default();
        m.pressed(100.0, 80.0);
        assert_eq!(m.imouse(H), [100.0, H - 80.0, 100.0, H - 80.0]);
        m.frame_rendered();
        assert_eq!(m.imouse(H), [100.0, H - 80.0, 100.0, -(H - 80.0)]);
    }

    #[test]
    fn drag_moves_xy_only() {
        let mut m = Drag::default();
        m.pressed(100.0, 80.0);
        m.frame_rendered();
        m.moved(150.0, 90.0);
        assert_eq!(m.imouse(H), [150.0, H - 90.0, 100.0, -(H - 80.0)]);
        // surface の外の座標もそのまま渡す。
        m.moved(-20.0, 2000.0);
        assert_eq!(m.imouse(H), [-20.0, H - 2000.0, 100.0, -(H - 80.0)]);
    }

    #[test]
    fn hover_without_button_does_not_move() {
        let mut m = Drag::default();
        m.moved(10.0, 10.0);
        assert_eq!(m.imouse(H), [0.0; 4]);
        m.released();
        assert_eq!(m.imouse(H), [0.0; 4]);
    }

    #[test]
    fn release_makes_z_negative_and_keeps_values() {
        let mut m = Drag::default();
        m.pressed(100.0, 80.0);
        m.frame_rendered();
        m.moved(150.0, 90.0);
        m.released();
        assert_eq!(m.imouse(H), [150.0, H - 90.0, -100.0, -(H - 80.0)]);
        // 切れた後は、残っているボタンを押したまま動かしても変わらない。
        m.moved(300.0, 300.0);
        assert_eq!(m.imouse(H), [150.0, H - 90.0, -100.0, -(H - 80.0)]);
    }

    #[test]
    fn second_button_restarts_drag() {
        let mut m = Drag::default();
        m.pressed(100.0, 80.0);
        m.frame_rendered();
        m.moved(150.0, 90.0);
        m.pressed(150.0, 90.0);
        assert_eq!(m.imouse(H), [150.0, H - 90.0, 150.0, H - 90.0]);
    }

    #[test]
    fn press_and_release_before_a_frame_still_shows_the_click() {
        let mut m = Drag::default();
        m.pressed(100.0, 80.0);
        m.released();
        assert_eq!(m.imouse(H), [100.0, H - 80.0, -100.0, H - 80.0]);
        m.frame_rendered();
        assert_eq!(m.imouse(H), [100.0, H - 80.0, -100.0, -(H - 80.0)]);
    }
}
