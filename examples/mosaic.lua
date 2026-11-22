-- 画像を格子に切り分けた欠片が、ゆっくり揺れる壁紙。カーソルの近くの欠片は回りながら離れる。
-- images/plasma.jpg は ImageMagick の plasma: で作った画像。好きな写真に差し替えてよい。
-- @image photo images/plasma.jpg

local PIECES = 12         -- 短い辺を何個に切るか
local GAP = 3             -- 欠片の間の隙間（画面のピクセル）
local REACH = 260         -- カーソルが欠片を押しのける距離（画面のピクセル）

local sin, cos, sqrt, max = math.sin, math.cos, math.sqrt, math.max
local iw, ih
local t = 0

function load()
  iw, ih = imagesize("photo")
end

function update(dt)
  t = t + dt
end

function draw()
  clear(0.03, 0.03, 0.04)
  local w, h = screen.w, screen.h
  -- 画像の縦横比を保ったまま、画面いっぱいに広げる（はみ出した分は切る）。
  local s = max(w / iw, h / ih)
  local ox, oy = (w - iw * s) / 2, (h - ih * s) / 2

  -- 背景に、暗くした画像全体を敷く。
  color(0.25, 0.25, 0.3)
  image("photo", ox, oy, iw * s, ih * s)

  local cell = math.min(iw, ih) / PIECES -- 画像のピクセルでの欠片の大きさ
  local size = cell * s                   -- 画面のピクセルでの欠片の大きさ
  local mx, my = mouse.x, mouse.y
  for j = 0, math.ceil(ih / cell) - 1 do
    for i = 0, math.ceil(iw / cell) - 1 do
      local x, y = ox + i * size, oy + j * size
      local cx, cy = x + size / 2, y + size / 2
      -- 欠片ごとに少しずつずらして揺らす。
      local a = 0.06 * sin(t * 0.7 + i * 0.9 + j * 1.3)
      local dx, dy = 0, 0
      if mouse.inside then
        local ex, ey = cx - mx, cy - my
        local d = sqrt(ex * ex + ey * ey)
        if d < REACH then
          local k = (1 - d / REACH) ^ 2
          dx, dy = ex / (d + 1e-6) * k * size * 0.8, ey / (d + 1e-6) * k * size * 0.8
          a = a + k * 0.8
        end
      end
      color(1, 1, 1)
      subimage("photo", i * cell, j * cell, cell, cell,
        x + GAP / 2 + dx, y + GAP / 2 + dy, size - GAP, size - GAP, a)
    end
  end
end
