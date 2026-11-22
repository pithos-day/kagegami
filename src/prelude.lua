-- 描画関数の本体。ffi で Rust 側の頂点バッファーに直接書き込む。
--
-- 安全のための約束:
-- * ffi とバッファーのポインターは、このチャンクの引数（= upvalue）としてだけ持つ。
--   壁紙スクリプトからは debug / getfenv が無いので届かない。
-- * このチャンクは空の環境で動かすので、グローバルを参照しない（壁紙側で書き換えられても影響しない）。
-- * 頂点を書く前に必ず残り容量を確かめる。満杯なら Rust 側へ渡してから書く。
-- * 返す関数は何も返さない（cdata を壁紙側に渡さない）。imagesize だけは数を 2 つ返す。
-- * 画像は図形と別のバッファーに書く。書き込み済みの画像は、書き込み済みの図形より必ず前に描くもの
--   （画像を書く前に、書き込み済みの図形を渡す）。なので渡すときは、いつも画像を先に渡す。

local ffi, ptr, image_ptr, CAP, take, take_images, rust_clear, rust_blend, images,
  sqrt, cos, sin, error, type, select = ...

ffi.cdef [[
typedef struct {
  float px, py;
  float lx, ly;
  float sl, sr;
  uint8_t r, g, b, a;
} kagegami_vertex;
typedef struct {
  float px, py;
  float u, v;
  float lx, ly;
  float hx, hy;
  uint8_t r, g, b, a;
} kagegami_image_vertex;
]]

local buf = ffi.cast("kagegami_vertex *", ptr)
local ibuf = ffi.cast("kagegami_image_vertex *", image_ptr)
ptr, image_ptr = nil, nil

-- 名前 → 番号（0 始まり）と、番号 + 1 → 幅・高さ。
local IMAGE_INDEX, IMAGE_W, IMAGE_H = images.index, images.w, images.h
images = nil

-- render.rs の AA_PAD / FILL_RADIUS / MIN_HALF_SIZE と同じ値。
local AA_PAD = 1.0
local FILL_RADIUS = 8192.0
local MIN_HALF_SIZE = 1e-4

local n = 0 -- 書き込み済みの図形の頂点数
local ni = 0 -- 書き込み済みの画像の頂点数
local cur = -1 -- 書き込み済みの画像の番号
local cr, cg, cb, ca = 255, 255, 255, 255

local function flush_images()
  if ni > 0 then
    take_images(ni, cur)
    ni = 0
  end
end

local function flush()
  flush_images()
  if n > 0 then
    take(n)
    n = 0
  end
end

local function reserve()
  if n + 4 > CAP then
    flush()
  end
end

-- 0〜1 を 0〜255 に。NaN や範囲外も必ず 0〜255 に収める。
local function to_u8(v)
  if not (v > 0) then return 0 end
  if v >= 1 then return 255 end
  return v * 255 + 0.5
end

local function num(v, name)
  if type(v) ~= "number" then
    error(name .. ": number expected, got " .. type(v), 3)
  end
  return v
end

-- 決まった数より後ろの引数（...）が、すべて nil でなければエラーにする。
-- 後ろに付いた nil は数えないので、変数が nil のまま渡しても省略と同じになる。
local function too_many(name, max, ...)
  for i = 1, select("#", ...) do
    if select(i, ...) ~= nil then
      error(name .. ": too many arguments (max " .. max .. ")", 3)
    end
  end
end

local function put(px, py, lx, ly, sl, sr)
  local v = buf[n]
  v.px, v.py, v.lx, v.ly, v.sl, v.sr = px, py, lx, ly, sl, sr
  v.r, v.g, v.b, v.a = cr, cg, cb, ca
  n = n + 1
end

local function circle_at(x, y, r)
  if not (r > 0) then r = 0 end
  local e = r + AA_PAD
  reserve()
  put(x - e, y - e, -e, -e, 0, r)
  put(x + e, y - e, e, -e, 0, r)
  put(x - e, y + e, -e, e, 0, r)
  put(x + e, y + e, e, e, 0, r)
end

local api = {}

-- どの描画関数も、決まった数より多い引数はエラーにする（後ろに付いた nil は数えない）。

function api.color(r, g, b, a, ...)
  if select("#", ...) > 0 then too_many("color", 4, ...) end
  cr = to_u8(num(r, "color"))
  cg = to_u8(num(g, "color"))
  cb = to_u8(num(b, "color"))
  ca = a == nil and 255 or to_u8(num(a, "color"))
end

function api.circle(x, y, r, ...)
  if select("#", ...) > 0 then too_many("circle", 3, ...) end
  circle_at(num(x, "circle"), num(y, "circle"), num(r, "circle"))
end

function api.line(x1, y1, x2, y2, w, ...)
  if select("#", ...) > 0 then too_many("line", 5, ...) end
  num(x1, "line"); num(y1, "line"); num(x2, "line"); num(y2, "line")
  local hw = (w == nil and 1 or num(w, "line")) * 0.5
  if not (hw > 0) then hw = 0 end
  local dx, dy = x2 - x1, y2 - y1
  local len = sqrt(dx * dx + dy * dy)
  if not (len >= 1e-4) then
    return circle_at(x1, y1, hw)
  end
  local ux, uy = dx / len, dy / len
  local nx, ny = -uy, ux
  local e = hw + AA_PAD
  reserve()
  put(x1 - ux * e - nx * e, y1 - uy * e - ny * e, -e, -e, len, hw)
  put(x2 + ux * e - nx * e, y2 + uy * e - ny * e, len + e, -e, len, hw)
  put(x1 - ux * e + nx * e, y1 - uy * e + ny * e, -e, e, len, hw)
  put(x2 + ux * e + nx * e, y2 + uy * e + ny * e, len + e, e, len, hw)
end

-- angle を渡したときは、値が 0 でも回転した矩形として描く（縁にアンチエイリアスが付く）。
-- 0 をまたいで回したときに、0 の瞬間だけ縁の見た目が変わらないようにするため。
local function rotated_rect(x, y, w, h, angle)
  local hx = (w < 0 and -w or w) * 0.5
  local hy = (h < 0 and -h or h) * 0.5
  -- NaN もここで弾く。
  if not (hx >= MIN_HALF_SIZE and hy >= MIN_HALF_SIZE) then return end
  local cx, cy = x + w * 0.5, y + h * 0.5
  local c, s = cos(angle), sin(angle)
  local ex, ey = hx + AA_PAD, hy + AA_PAD
  reserve()
  -- ローカル座標 (lx, ly) を (cx + lx*c - ly*s, cy + lx*s + ly*c) に置く。
  put(cx - ex * c + ey * s, cy - ex * s - ey * c, -ex, -ey, -hx, hy)
  put(cx + ex * c + ey * s, cy + ex * s - ey * c, ex, -ey, -hx, hy)
  put(cx - ex * c - ey * s, cy - ex * s + ey * c, -ex, ey, -hx, hy)
  put(cx + ex * c - ey * s, cy + ex * s + ey * c, ex, ey, -hx, hy)
end

function api.rect(x, y, w, h, angle, ...)
  if select("#", ...) > 0 then too_many("rect", 5, ...) end
  num(x, "rect"); num(y, "rect"); num(w, "rect"); num(h, "rect")
  if angle ~= nil then
    return rotated_rect(x, y, w, h, num(angle, "rect"))
  end
  reserve()
  put(x, y, 0, 0, 0, FILL_RADIUS)
  put(x + w, y, 0, 0, 0, FILL_RADIUS)
  put(x, y + h, 0, 0, 0, FILL_RADIUS)
  put(x + w, y + h, 0, 0, 0, FILL_RADIUS)
end

-- 画像の名前を番号にする。直接 api.* から呼ぶ（エラーの位置を呼び出し元にするため）。
local function image_index(name, fname)
  local i = IMAGE_INDEX[name]
  if i ~= nil then return i end
  if type(name) ~= "string" then
    error(fname .. ": image name expected, got " .. type(name), 3)
  end
  error(fname .. ": unknown image \"" .. name .. "\" (declare it with -- @image)", 3)
end

local function put_image(px, py, u, v, lx, ly, hx, hy)
  local p = ibuf[ni]
  p.px, p.py, p.u, p.v, p.lx, p.ly, p.hx, p.hy = px, py, u, v, lx, ly, hx, hy
  p.r, p.g, p.b, p.a = cr, cg, cb, ca
  ni = ni + 1
end

-- 画像の頂点を書く前に、順番を保つために渡すものを渡し、空きを作る。
local function reserve_image(i)
  if n > 0 then
    flush()
  elseif i ~= cur then
    flush_images()
  end
  if ni + 4 > CAP then flush_images() end
  cur = i
end

-- 画像 i の (u0, v0)〜(u1, v1) の部分を、(x, y, w, h) の矩形に描く。w・h が負なら裏返す。
-- angle を渡したときは、矩形の中心のまわりに回し、縁にアンチエイリアスを付ける（rect と同じ）。
local function draw_image(i, x, y, w, h, u0, v0, u1, v1, angle)
  if angle == nil then
    reserve_image(i)
    put_image(x, y, u0, v0, 0, 0, 0, 0)
    put_image(x + w, y, u1, v0, 0, 0, 0, 0)
    put_image(x, y + h, u0, v1, 0, 0, 0, 0)
    put_image(x + w, y + h, u1, v1, 0, 0, 0, 0)
    return
  end
  local hx = (w < 0 and -w or w) * 0.5
  local hy = (h < 0 and -h or h) * 0.5
  if not (hx >= MIN_HALF_SIZE and hy >= MIN_HALF_SIZE) then return end
  local cx, cy = x + w * 0.5, y + h * 0.5
  local c, s = cos(angle), sin(angle)
  local ex, ey = hx + AA_PAD, hy + AA_PAD
  -- ローカル座標 lx でのテクスチャー座標は uc + lx * du（w が負なら du も負で、裏返る）。
  local uc, vc = (u0 + u1) * 0.5, (v0 + v1) * 0.5
  local du, dv = (u1 - u0) / w, (v1 - v0) / h
  reserve_image(i)
  put_image(cx - ex * c + ey * s, cy - ex * s - ey * c, uc - ex * du, vc - ey * dv, -ex, -ey, hx, hy)
  put_image(cx + ex * c + ey * s, cy + ex * s - ey * c, uc + ex * du, vc - ey * dv, ex, -ey, hx, hy)
  put_image(cx - ex * c - ey * s, cy - ex * s + ey * c, uc - ex * du, vc + ey * dv, -ex, ey, hx, hy)
  put_image(cx + ex * c - ey * s, cy + ex * s + ey * c, uc + ex * du, vc + ey * dv, ex, ey, hx, hy)
end

-- image(name, x, y) / (name, x, y, angle) / (name, x, y, w, h) / (name, x, y, w, h, angle)
function api.image(name, x, y, a, b, c, ...)
  if select("#", ...) > 0 then too_many("image", 6, ...) end
  local i = image_index(name, "image")
  num(x, "image"); num(y, "image")
  local w, h, angle
  if b == nil and c == nil then
    w, h = IMAGE_W[i + 1], IMAGE_H[i + 1]
    if a ~= nil then angle = num(a, "image") end
  else
    w, h = num(a, "image"), num(b, "image")
    if c ~= nil then angle = num(c, "image") end
  end
  draw_image(i, x, y, w, h, 0, 0, 1, 1, angle)
end

-- subimage(name, sx, sy, sw, sh, x, y, w, h[, angle])。(sx, sy, sw, sh) は画像のピクセルで、
-- 画像の外は wrap の指定に従う。
function api.subimage(name, sx, sy, sw, sh, x, y, w, h, angle, ...)
  if select("#", ...) > 0 then too_many("subimage", 10, ...) end
  local i = image_index(name, "subimage")
  num(sx, "subimage"); num(sy, "subimage"); num(sw, "subimage"); num(sh, "subimage")
  num(x, "subimage"); num(y, "subimage"); num(w, "subimage"); num(h, "subimage")
  if angle ~= nil then num(angle, "subimage") end
  local iw, ih = IMAGE_W[i + 1], IMAGE_H[i + 1]
  draw_image(i, x, y, w, h, sx / iw, sy / ih, (sx + sw) / iw, (sy + sh) / ih, angle)
end

function api.imagesize(name, ...)
  if select("#", ...) > 0 then too_many("imagesize", 1, ...) end
  local i = image_index(name, "imagesize")
  return IMAGE_W[i + 1], IMAGE_H[i + 1]
end

function api.clear(r, g, b, ...)
  if select("#", ...) > 0 then too_many("clear", 3, ...) end
  -- clear より前の図形と画像は見えなくなるので捨てる。
  n, ni = 0, 0
  rust_clear(num(r, "clear"), num(g, "clear"), num(b, "clear"))
end

function api.blend(mode, ...)
  if select("#", ...) > 0 then too_many("blend", 1, ...) end
  -- 切り替える前に、それまでの図形と画像を前のモードの区間として渡す。
  flush()
  rust_blend(mode)
end

-- 以下は Rust 側だけが呼ぶ。
local host = {}

function host.begin()
  n, ni = 0, 0
  cr, cg, cb, ca = 255, 255, 255, 255
end

host.finish = flush

return api, host
