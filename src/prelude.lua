-- 描画関数の本体。ffi で Rust 側の頂点バッファーに直接書き込む。
--
-- 安全のための約束:
-- * ffi とバッファーのポインターは、このチャンクの引数（= upvalue）としてだけ持つ。
--   壁紙スクリプトからは debug / getfenv が無いので届かない。
-- * このチャンクは空の環境で動かすので、グローバルを参照しない（壁紙側で書き換えられても影響しない）。
-- * 頂点を書く前に必ず残り容量を確かめる。満杯なら Rust 側へ渡してから書く。
-- * 返す関数は何も返さない（cdata を壁紙側に渡さない）。

local ffi, ptr, CAP, take, rust_clear, rust_blend, sqrt, cos, sin, error, type = ...

ffi.cdef [[
typedef struct {
  float px, py;
  float lx, ly;
  float sl, sr;
  uint8_t r, g, b, a;
} kagegami_vertex;
]]

local buf = ffi.cast("kagegami_vertex *", ptr)
ptr = nil

-- render.rs の AA_PAD / FILL_RADIUS / MIN_HALF_SIZE と同じ値。
local AA_PAD = 1.0
local FILL_RADIUS = 8192.0
local MIN_HALF_SIZE = 1e-4

local n = 0 -- 書き込み済みの頂点数
local cr, cg, cb, ca = 255, 255, 255, 255

local function flush()
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

function api.color(r, g, b, a)
  cr = to_u8(num(r, "color"))
  cg = to_u8(num(g, "color"))
  cb = to_u8(num(b, "color"))
  ca = a == nil and 255 or to_u8(num(a, "color"))
end

function api.circle(x, y, r)
  circle_at(num(x, "circle"), num(y, "circle"), num(r, "circle"))
end

function api.line(x1, y1, x2, y2, w)
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

function api.rect(x, y, w, h, angle)
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

function api.clear(r, g, b)
  -- clear より前の図形は見えなくなるので捨てる。
  n = 0
  rust_clear(num(r, "clear"), num(g, "clear"), num(b, "clear"))
end

function api.blend(mode)
  -- 切り替える前に、それまでの図形を前のモードの区間として渡す。
  flush()
  rust_blend(mode)
end

-- 以下は Rust 側だけが呼ぶ。
local host = {}

function host.begin()
  n = 0
  cr, cg, cb, ca = 255, 255, 255, 255
end

host.finish = flush

return api, host
