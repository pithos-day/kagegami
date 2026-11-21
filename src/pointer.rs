//! ポインターの状態。

use smithay_client_toolkit::seat::pointer::PointerEventKind;

use crate::script::{Button, MouseEvent, MouseState};

// linux/input-event-codes.h
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;
/// マウスのボタンの範囲（BTN_LEFT〜BTN_TASK）。シェーダーの `iMouse` はどのボタンでも反応する。
const MOUSE_BUTTONS: std::ops::RangeInclusive<u32> = 0x110..=0x117;

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
    /// ポインターのイベントを 1 つ反映する。座標は出力のピクセル。
    pub fn apply(&mut self, kind: &PointerEventKind, x: f32, y: f32) {
        match *kind {
            PointerEventKind::Motion { .. } => self.moved(x, y),
            PointerEventKind::Press { button, .. } if MOUSE_BUTTONS.contains(&button) => {
                self.pressed(x, y)
            }
            PointerEventKind::Release { button, .. } if MOUSE_BUTTONS.contains(&button) => {
                self.released()
            }
            PointerEventKind::Leave { .. } => self.released(),
            _ => {}
        }
    }

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

/// Lua の `mouse` にポインターのイベントを 1 つ反映し、`mousepressed`・`mousereleased` に渡す
/// ボタン操作を返す。`drag` は同じイベントを反映した後の状態。座標は出力のピクセル。
/// 左・右・中以外のボタンとホイール（Axis）では、ドラッグの状態のほかは変えない。
pub fn apply_to_lua_mouse(
    mouse: &mut MouseState,
    drag: &Drag,
    kind: &PointerEventKind,
    x: f32,
    y: f32,
) -> Option<MouseEvent> {
    // mouse.press_x・press_y・dragging（#13）。iMouse と同じくボタンを区別しない。
    let ([press_x, press_y], dragging) = drag.lua_press();
    mouse.press_x = press_x;
    mouse.press_y = press_y;
    mouse.dragging = dragging;
    let mut click = None;
    match *kind {
        PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
            mouse.inside = true;
        }
        PointerEventKind::Leave { .. } => {
            // カーソルが外に出たら、押していたボタンの状態も分からなくなる。
            *mouse = MouseState {
                x: mouse.x,
                y: mouse.y,
                press_x: mouse.press_x,
                press_y: mouse.press_y,
                ..Default::default()
            };
            return None;
        }
        PointerEventKind::Press { button, .. } | PointerEventKind::Release { button, .. } => {
            let pressed = matches!(kind, PointerEventKind::Press { .. });
            let b = match button {
                BTN_LEFT => Button::Left,
                BTN_RIGHT => Button::Right,
                BTN_MIDDLE => Button::Middle,
                _ => return None,
            };
            *match b {
                Button::Left => &mut mouse.left,
                Button::Right => &mut mouse.right,
                Button::Middle => &mut mouse.middle,
            } = pressed;
            click = Some(if pressed {
                MouseEvent::Pressed(x, y, b)
            } else {
                MouseEvent::Released(x, y, b)
            });
        }
        PointerEventKind::Axis { .. } => return None,
    }
    mouse.x = x;
    mouse.y = y;
    click
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

    fn press(button: u32) -> PointerEventKind {
        PointerEventKind::Press {
            time: 0,
            button,
            serial: 0,
        }
    }

    fn release(button: u32) -> PointerEventKind {
        PointerEventKind::Release {
            time: 0,
            button,
            serial: 0,
        }
    }

    const MOTION: PointerEventKind = PointerEventKind::Motion { time: 0 };
    const ENTER: PointerEventKind = PointerEventKind::Enter { serial: 0 };
    const LEAVE: PointerEventKind = PointerEventKind::Leave { serial: 0 };
    /// BTN_SIDE。iMouse とドラッグには効くが、Lua のボタンには対応しない。
    const BTN_SIDE: u32 = 0x113;

    /// イベントを Drag と Lua の mouse の両方に、pointer_frame と同じ順で反映する。
    fn feed(
        drag: &mut Drag,
        mouse: &mut MouseState,
        kind: PointerEventKind,
        x: f32,
        y: f32,
    ) -> Option<MouseEvent> {
        drag.apply(&kind, x, y);
        apply_to_lua_mouse(mouse, drag, &kind, x, y)
    }

    #[test]
    fn enter_and_motion_set_inside_and_position() {
        let (mut d, mut m) = (Drag::default(), MouseState::default());
        assert!(feed(&mut d, &mut m, ENTER, 10.0, 20.0).is_none());
        assert!(m.inside);
        assert_eq!((m.x, m.y), (10.0, 20.0));
        assert!(feed(&mut d, &mut m, MOTION, 30.0, 40.0).is_none());
        assert_eq!((m.x, m.y), (30.0, 40.0));
        // ボタンを押していなければドラッグは動かない。
        assert_eq!(d.imouse(H), [0.0; 4]);
    }

    #[test]
    fn left_press_and_release_are_reported() {
        let (mut d, mut m) = (Drag::default(), MouseState::default());
        let ev = feed(&mut d, &mut m, press(BTN_LEFT), 5.0, 6.0);
        assert!(matches!(
            ev,
            Some(MouseEvent::Pressed(5.0, 6.0, Button::Left))
        ));
        assert!(m.left && m.dragging);
        assert_eq!((m.press_x, m.press_y), (5.0, 6.0));
        feed(&mut d, &mut m, MOTION, 50.0, 60.0);
        assert_eq!((m.press_x, m.press_y), (5.0, 6.0));
        let ev = feed(&mut d, &mut m, release(BTN_LEFT), 50.0, 60.0);
        assert!(matches!(
            ev,
            Some(MouseEvent::Released(50.0, 60.0, Button::Left))
        ));
        assert!(!m.left && !m.dragging);
    }

    #[test]
    fn right_and_middle_buttons_are_reported() {
        let (mut d, mut m) = (Drag::default(), MouseState::default());
        let ev = feed(&mut d, &mut m, press(BTN_RIGHT), 1.0, 1.0);
        assert!(matches!(ev, Some(MouseEvent::Pressed(_, _, Button::Right))));
        let ev = feed(&mut d, &mut m, press(BTN_MIDDLE), 1.0, 1.0);
        assert!(matches!(
            ev,
            Some(MouseEvent::Pressed(_, _, Button::Middle))
        ));
        assert!(m.right && m.middle && !m.left);
    }

    #[test]
    fn side_button_drags_but_is_not_a_lua_button() {
        let (mut d, mut m) = (Drag::default(), MouseState::default());
        feed(&mut d, &mut m, MOTION, 1.0, 2.0);
        assert!(feed(&mut d, &mut m, press(BTN_SIDE), 7.0, 8.0).is_none());
        // ドラッグの状態は変わるが、ボタンと位置は変えない。
        assert!(m.dragging);
        assert_eq!((m.press_x, m.press_y), (7.0, 8.0));
        assert!(!m.left && !m.right && !m.middle);
        assert_eq!((m.x, m.y), (1.0, 2.0));
    }

    #[test]
    fn axis_does_not_move_the_lua_mouse() {
        use smithay_client_toolkit::seat::pointer::AxisScroll;
        let (mut d, mut m) = (Drag::default(), MouseState::default());
        feed(&mut d, &mut m, MOTION, 1.0, 2.0);
        let axis = PointerEventKind::Axis {
            time: 0,
            horizontal: AxisScroll::default(),
            vertical: AxisScroll::default(),
            source: None,
        };
        assert!(feed(&mut d, &mut m, axis, 9.0, 9.0).is_none());
        assert_eq!((m.x, m.y), (1.0, 2.0));
    }

    #[test]
    fn leave_forgets_buttons_but_keeps_positions() {
        let (mut d, mut m) = (Drag::default(), MouseState::default());
        feed(&mut d, &mut m, MOTION, 1.0, 2.0);
        feed(&mut d, &mut m, press(BTN_LEFT), 3.0, 4.0);
        feed(&mut d, &mut m, MOTION, 5.0, 6.0);
        assert!(feed(&mut d, &mut m, LEAVE, 99.0, 99.0).is_none());
        assert_eq!((m.x, m.y), (5.0, 6.0));
        assert_eq!((m.press_x, m.press_y), (3.0, 4.0));
        assert!(!m.inside && !m.left && !m.dragging);
        // iMouse もドラッグが切れる。
        assert_eq!(d.imouse(H)[2], -3.0);
    }
}
