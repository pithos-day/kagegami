//! キーボードの入力（#69）。
//!
//! - シェーダー: `@iChannelN keyboard` のチャンネルに、Shadertoy と同じ 256×3 のテクスチャーを渡す。
//!   1 行目（y=0）は押している間、2 行目（y=1）は押したフレームだけ、3 行目（y=2）は押すたびに
//!   切り替わる。値は赤のチャンネルの 0 か 255。x はブラウザーの `keyCode`
//! - Lua: `keyboard.down(name)` で、そのキーを押しているかを読む。名前は LÖVE と同じ形
//!
//! キーは xkbcommon で、キーボードの配列に従って決める。Shift などを押していないときの記号（レベル 0）を
//! 使うので、Shift を押しながら押しても同じキーになる。

use xkbcommon::xkb::{self, Keysym, keysyms as k};

/// 1 つのキー。押したときに決め、離すまで変えない（Shift を先に離しても、同じキーを離した扱いにするため）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    /// ブラウザーの `keyCode`。対応する番号がないキーは None（テクスチャーに出ない）。
    pub code: Option<u8>,
    /// Lua の名前。対応する名前がないキーは None（`keyboard.down` で読めない）。
    pub name: Option<&'static str>,
}

/// 文字・数字・記号のほかに `keyboard.down` が受け付ける名前と、その `keyCode`。
const NAMED: &[(u32, &str, Option<u8>)] = &[
    (k::KEY_space, "space", Some(32)),
    (k::KEY_Return, "return", Some(13)),
    (k::KEY_Escape, "escape", Some(27)),
    (k::KEY_BackSpace, "backspace", Some(8)),
    (k::KEY_Tab, "tab", Some(9)),
    (k::KEY_ISO_Left_Tab, "tab", Some(9)),
    (k::KEY_Delete, "delete", Some(46)),
    (k::KEY_Insert, "insert", Some(45)),
    (k::KEY_Home, "home", Some(36)),
    (k::KEY_End, "end", Some(35)),
    (k::KEY_Prior, "pageup", Some(33)),
    (k::KEY_Next, "pagedown", Some(34)),
    (k::KEY_Left, "left", Some(37)),
    (k::KEY_Up, "up", Some(38)),
    (k::KEY_Right, "right", Some(39)),
    (k::KEY_Down, "down", Some(40)),
    (k::KEY_Shift_L, "lshift", Some(16)),
    (k::KEY_Shift_R, "rshift", Some(16)),
    (k::KEY_Control_L, "lctrl", Some(17)),
    (k::KEY_Control_R, "rctrl", Some(17)),
    (k::KEY_Alt_L, "lalt", Some(18)),
    (k::KEY_Alt_R, "ralt", Some(18)),
    // AltGr。LÖVE（SDL）と同じく右の Alt として扱う。
    (k::KEY_ISO_Level3_Shift, "ralt", Some(18)),
    (k::KEY_Super_L, "lgui", Some(91)),
    (k::KEY_Super_R, "rgui", Some(92)),
    (k::KEY_Menu, "menu", Some(93)),
    (k::KEY_Caps_Lock, "capslock", Some(20)),
    (k::KEY_Num_Lock, "numlock", Some(144)),
    (k::KEY_Scroll_Lock, "scrolllock", Some(145)),
    (k::KEY_Pause, "pause", Some(19)),
    (k::KEY_Print, "printscreen", Some(44)),
    (k::KEY_KP_0, "kp0", Some(96)),
    (k::KEY_KP_1, "kp1", Some(97)),
    (k::KEY_KP_2, "kp2", Some(98)),
    (k::KEY_KP_3, "kp3", Some(99)),
    (k::KEY_KP_4, "kp4", Some(100)),
    (k::KEY_KP_5, "kp5", Some(101)),
    (k::KEY_KP_6, "kp6", Some(102)),
    (k::KEY_KP_7, "kp7", Some(103)),
    (k::KEY_KP_8, "kp8", Some(104)),
    (k::KEY_KP_9, "kp9", Some(105)),
    (k::KEY_KP_Decimal, "kp.", Some(110)),
    (k::KEY_KP_Divide, "kp/", Some(111)),
    (k::KEY_KP_Multiply, "kp*", Some(106)),
    (k::KEY_KP_Subtract, "kp-", Some(109)),
    (k::KEY_KP_Add, "kp+", Some(107)),
    (k::KEY_KP_Equal, "kp=", None),
    (k::KEY_KP_Enter, "kpenter", Some(13)),
];

/// `f1`〜`f24`。`keyCode` は 112〜135。
const FUNCTION_NAMES: [&str; 24] = [
    "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10", "f11", "f12", "f13", "f14", "f15",
    "f16", "f17", "f18", "f19", "f20", "f21", "f22", "f23", "f24",
];

/// 1 文字の名前に使う、ASCII の表示できる文字（0x21〜0x7e）。
const PRINTABLE: &str = "!\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~";

/// 1 文字の名前。大文字は小文字にする。
fn char_name(c: u8) -> Option<&'static str> {
    let c = c.to_ascii_lowercase();
    let i = c.checked_sub(0x21)? as usize;
    PRINTABLE.get(i..=i)
}

/// 記号の `keyCode`（US 配列のブラウザーと同じ番号）。
fn punct_code(c: u8) -> Option<u8> {
    Some(match c {
        b';' => 186,
        b'=' => 187,
        b',' => 188,
        b'-' => 189,
        b'.' => 190,
        b'/' => 191,
        b'`' => 192,
        b'[' => 219,
        b'\\' => 220,
        b']' => 221,
        b'\'' => 222,
        _ => return None,
    })
}

/// 記号（keysym）からキーを決める。
pub fn key_from_sym(sym: Keysym) -> Key {
    let raw = sym.raw();
    if let Some(&(_, name, code)) = NAMED.iter().find(|(s, _, _)| *s == raw) {
        return Key {
            code,
            name: Some(name),
        };
    }
    if (k::KEY_F1..=k::KEY_F24).contains(&raw) {
        let i = (raw - k::KEY_F1) as usize;
        return Key {
            code: Some(112 + i as u8),
            name: Some(FUNCTION_NAMES[i]),
        };
    }
    // Latin-1 の範囲の keysym は文字の番号と同じ。
    if (0x21..=0x7e).contains(&raw) {
        let c = (raw as u8).to_ascii_lowercase();
        let code = match c {
            b'a'..=b'z' => Some(c.to_ascii_uppercase()),
            b'0'..=b'9' => Some(c),
            _ => punct_code(c),
        };
        return Key {
            code,
            name: char_name(c),
        };
    }
    Key {
        code: None,
        name: None,
    }
}

/// `keyboard.down` が受け付ける名前か。
pub fn is_key_name(name: &str) -> bool {
    match name.as_bytes() {
        [c] => (0x21..=0x7e).contains(c) && !c.is_ascii_uppercase(),
        _ => NAMED.iter().any(|(_, n, _)| *n == name) || FUNCTION_NAMES.contains(&name),
    }
}

/// コンポジターから届いた配列。キーの番号から、Shift などを押していないときの記号を引く。
#[derive(Default)]
pub struct Layout {
    keymap: Option<xkb::Keymap>,
    /// 使っている配列の番号（`wl_keyboard.modifiers` の group）。
    group: u32,
}

impl Layout {
    /// 配列を読み直す。読めなければ、届いたイベントの記号をそのまま使う。
    pub fn set_keymap(&mut self, text: String) {
        let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        self.keymap = xkb::Keymap::new_from_string(
            &ctx,
            text,
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::COMPILE_NO_FLAGS,
        );
    }

    pub fn set_group(&mut self, group: u32) {
        self.group = group;
    }

    /// `raw` は `wl_keyboard` のキーの番号（evdev）。`fallback` は今の修飾キーで決まった記号。
    pub fn key(&self, raw: u32, fallback: Keysym) -> Key {
        key_from_sym(self.base_sym(raw).unwrap_or(fallback))
    }

    fn base_sym(&self, raw: u32) -> Option<Keysym> {
        let keymap = self.keymap.as_ref()?;
        let code = xkb::Keycode::new(raw + 8);
        let layouts = keymap.num_layouts_for_key(code);
        if layouts == 0 {
            return None;
        }
        // 配列の番号がキーの配列の数を超えたら、xkbcommon の既定と同じく折り返す。
        let group = self.group % layouts;
        let sym = |level| {
            keymap
                .key_get_syms_by_level(code, group, level)
                .first()
                .copied()
        };
        let base = sym(0)?;
        // テンキーは NumLock を切っていても数字のキーとして扱う（レベル 1 が数字）。
        if (k::KEY_KP_Space..=k::KEY_KP_Equal).contains(&base.raw())
            && let Some(num) = sym(1)
            && (k::KEY_KP_Decimal..=k::KEY_KP_9).contains(&num.raw())
        {
            return Some(num);
        }
        Some(base)
    }
}

/// テクスチャーの大きさ（Shadertoy と同じ）。
pub const TEXTURE_WIDTH: usize = 256;
pub const TEXTURE_HEIGHT: usize = 3;

/// 1 つの出力のキーの状態。押したフレームだけの印（2 行目）を、出力ごとに描いたときに消すため、
/// 出力ごとに持つ。キーのイベントはすべての出力に渡す。
#[derive(Clone, Debug, Default)]
pub struct Keys {
    /// 押しているキー（`wl_keyboard` のキーの番号と、押したときに決めたキー）。
    held: Vec<(u32, Key)>,
    /// 押してから、まだ 1 フレームも描いていない `keyCode`。
    hit: Vec<u8>,
    /// 押すたびに切り替わる。
    toggle: Vec<u8>,
}

impl Keys {
    /// キーを押した。
    pub fn press(&mut self, raw: u32, key: Key) {
        if self.held.iter().any(|(r, _)| *r == raw) {
            return;
        }
        if let Some(c) = key.code
            && !self.code_down(c)
        {
            if !self.hit.contains(&c) {
                self.hit.push(c);
            }
            match self.toggle.iter().position(|t| *t == c) {
                Some(i) => {
                    self.toggle.swap_remove(i);
                }
                None => self.toggle.push(c),
            }
        }
        self.held.push((raw, key));
    }

    /// フォーカスが来たときに、すでに押していたキー。押した瞬間は見ていないので、2 行目と 3 行目は変えない。
    pub fn hold(&mut self, raw: u32, key: Key) {
        if !self.held.iter().any(|(r, _)| *r == raw) {
            self.held.push((raw, key));
        }
    }

    pub fn release(&mut self, raw: u32) {
        self.held.retain(|(r, _)| *r != raw);
    }

    /// フォーカスが外れた。押していたキーをすべて離した扱いにする。
    pub fn release_all(&mut self) {
        self.held.clear();
    }

    /// 1 フレーム描いた。押したフレームだけの印はここで消える。
    pub fn frame_rendered(&mut self) {
        self.hit.clear();
    }

    fn code_down(&self, code: u8) -> bool {
        self.held.iter().any(|(_, k)| k.code == Some(code))
    }

    /// 押している名前（Lua に渡す）。
    pub fn names_down(&self) -> Vec<&'static str> {
        let mut names: Vec<_> = self.held.iter().filter_map(|(_, k)| k.name).collect();
        names.dedup();
        names
    }

    /// シェーダーに渡すテクスチャー（R8、256×3、1 行目が y=0）。
    pub fn texture(&self) -> [u8; TEXTURE_WIDTH * TEXTURE_HEIGHT] {
        let mut t = [0u8; TEXTURE_WIDTH * TEXTURE_HEIGHT];
        for (_, k) in &self.held {
            if let Some(c) = k.code {
                t[c as usize] = 255;
            }
        }
        for &c in &self.hit {
            t[TEXTURE_WIDTH + c as usize] = 255;
        }
        for &c in &self.toggle {
            t[2 * TEXTURE_WIDTH + c as usize] = 255;
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(raw: u32) -> Key {
        key_from_sym(Keysym::new(raw))
    }

    #[test]
    fn keys_get_browser_key_codes_and_love_names() {
        let cases: &[(u32, Option<u8>, Option<&str>)] = &[
            (k::KEY_a, Some(65), Some("a")),
            // Shift を押したときの記号でも、同じキーになる。
            (k::KEY_A, Some(65), Some("a")),
            (k::KEY_z, Some(90), Some("z")),
            (k::KEY_0, Some(48), Some("0")),
            (k::KEY_9, Some(57), Some("9")),
            (k::KEY_space, Some(32), Some("space")),
            (k::KEY_Return, Some(13), Some("return")),
            (k::KEY_Escape, Some(27), Some("escape")),
            (k::KEY_Left, Some(37), Some("left")),
            (k::KEY_Down, Some(40), Some("down")),
            (k::KEY_Shift_L, Some(16), Some("lshift")),
            (k::KEY_Shift_R, Some(16), Some("rshift")),
            (k::KEY_F1, Some(112), Some("f1")),
            (k::KEY_F12, Some(123), Some("f12")),
            (k::KEY_KP_5, Some(101), Some("kp5")),
            (k::KEY_KP_Enter, Some(13), Some("kpenter")),
            (k::KEY_semicolon, Some(186), Some(";")),
            (k::KEY_bracketleft, Some(219), Some("[")),
            // JIS 配列の @ のキー。名前はあるが、US 配列のブラウザーの番号はない。
            (k::KEY_at, None, Some("@")),
            // 日本語の入力のキーは、名前も番号もない。
            (k::KEY_Henkan, None, None),
        ];
        for &(raw, code, name) in cases {
            assert_eq!(sym(raw), Key { code, name }, "keysym 0x{raw:x}");
        }
    }

    #[test]
    fn key_names_are_lowercase_love_names() {
        for n in [
            "a", "z", "0", "space", "return", "left", "lshift", "f1", "f24", "kp0", "kpenter", ";",
            "@",
        ] {
            assert!(is_key_name(n), "{n}");
        }
        for n in ["A", "Space", "enter", "shift", "f25", "", " ", "ab", "あ"] {
            assert!(!is_key_name(n), "{n}");
        }
        // 作るキーの名前は、どれも受け付ける名前。
        for raw in (0x20..=0x7e)
            .chain(NAMED.iter().map(|n| n.0))
            .chain(k::KEY_F1..=k::KEY_F24)
        {
            if let Some(n) = sym(raw).name {
                assert!(is_key_name(n), "{n}");
            }
        }
    }

    fn row(t: &[u8], y: usize, x: u8) -> u8 {
        t[y * TEXTURE_WIDTH + x as usize]
    }

    #[test]
    fn texture_rows_follow_shadertoy() {
        let a = sym(k::KEY_a);
        let mut keys = Keys::default();
        keys.press(30, a);
        let t = keys.texture();
        assert_eq!(
            (row(&t, 0, 65), row(&t, 1, 65), row(&t, 2, 65)),
            (255, 255, 255)
        );
        // 押したフレームだけの印は、描いたら消える。押している間と切り替えは残る。
        keys.frame_rendered();
        let t = keys.texture();
        assert_eq!(
            (row(&t, 0, 65), row(&t, 1, 65), row(&t, 2, 65)),
            (255, 0, 255)
        );
        keys.release(30);
        let t = keys.texture();
        assert_eq!(
            (row(&t, 0, 65), row(&t, 1, 65), row(&t, 2, 65)),
            (0, 0, 255)
        );
        // もう一度押すと切り替わる。
        keys.press(30, a);
        let t = keys.texture();
        assert_eq!(
            (row(&t, 0, 65), row(&t, 1, 65), row(&t, 2, 65)),
            (255, 255, 0)
        );
        // 押したまま、離す前に描かなくても、押したフレームの印は描くまで残る。
        keys.release(30);
        assert_eq!(row(&keys.texture(), 1, 65), 255);
    }

    #[test]
    fn repeated_press_and_both_shifts() {
        let mut keys = Keys::default();
        let (l, r) = (sym(k::KEY_Shift_L), sym(k::KEY_Shift_R));
        keys.press(42, l);
        keys.frame_rendered();
        // 同じキーの 2 回目の押下（キーのリピートなど）は、押した印も切り替えも変えない。
        keys.press(42, l);
        let t = keys.texture();
        assert_eq!((row(&t, 1, 16), row(&t, 2, 16)), (0, 255));
        // 左を押したまま右を押しても、番号は同じ 16 なので新しく押した扱いにしない。
        keys.press(54, r);
        let t = keys.texture();
        assert_eq!((row(&t, 1, 16), row(&t, 2, 16)), (0, 255));
        // 左を離しても、右を押している間は 16 を押している。
        keys.release(42);
        assert_eq!(row(&keys.texture(), 0, 16), 255);
        assert_eq!(keys.names_down(), ["rshift"]);
        keys.release(54);
        assert_eq!(row(&keys.texture(), 0, 16), 0);
    }

    #[test]
    fn focus_changes_hold_and_release_keys() {
        let mut keys = Keys::default();
        keys.hold(30, sym(k::KEY_a));
        let t = keys.texture();
        assert_eq!(
            (row(&t, 0, 65), row(&t, 1, 65), row(&t, 2, 65)),
            (255, 0, 0)
        );
        assert_eq!(keys.names_down(), ["a"]);
        keys.press(57, sym(k::KEY_space));
        keys.release_all();
        let t = keys.texture();
        assert_eq!((row(&t, 0, 65), row(&t, 0, 32)), (0, 0));
        assert!(keys.names_down().is_empty());
    }

    /// US 配列の xkb の keymap。テストの環境に xkeyboard-config がなくても読めるように、文字列で持つ。
    const US_KEYMAP: &str = r#"xkb_keymap {
xkb_keycodes { minimum = 8; maximum = 255; <AC01> = 38; <KP0> = 90; <AE01> = 10; };
xkb_types {
    virtual_modifiers NumLock;
    type "ONE_LEVEL" { modifiers = none; level_name[Level1] = "Any"; };
    type "TWO_LEVEL" { modifiers = Shift; map[Shift] = Level2; level_name[Level1] = "Base"; level_name[Level2] = "Shift"; };
    type "ALPHABETIC" { modifiers = Shift+Lock; map[Shift] = Level2; map[Lock] = Level2; level_name[Level1] = "Base"; level_name[Level2] = "Caps"; };
    type "KEYPAD" { modifiers = Shift+NumLock; map[None] = Level1; map[NumLock] = Level2; level_name[Level1] = "Base"; level_name[Level2] = "Number"; };
};
xkb_compatibility { };
xkb_symbols {
    key <AC01> { type = "ALPHABETIC", [ a, A ] };
    key <AE01> { type = "TWO_LEVEL", [ 1, exclam ] };
    key <KP0> { type = "KEYPAD", [ KP_Insert, KP_0 ] };
};
};"#;

    #[test]
    fn layout_uses_the_unshifted_symbol() {
        let mut layout = Layout::default();
        // 配列が届く前は、イベントの記号をそのまま使う。
        assert_eq!(layout.key(2, Keysym::new(k::KEY_exclam)).name, Some("!"));
        layout.set_keymap(US_KEYMAP.to_string());
        assert!(layout.keymap.is_some());
        // evdev の番号は xkb の番号から 8 を引いたもの。
        assert_eq!(layout.key(30, Keysym::new(k::KEY_A)), sym(k::KEY_a));
        // Shift+1 で ! になっても、キーは 1。
        assert_eq!(layout.key(2, Keysym::new(k::KEY_exclam)), sym(k::KEY_1));
        // テンキーは NumLock を切っていても数字。
        assert_eq!(
            layout.key(82, Keysym::new(k::KEY_KP_Insert)),
            sym(k::KEY_KP_0)
        );
        // keymap にないキーは、イベントの記号を使う。
        assert_eq!(
            layout.key(1, Keysym::new(k::KEY_Escape)),
            sym(k::KEY_Escape)
        );
    }
}
