-- 格子に並んだ四角形が、画面の中心から広がる波に合わせて 90 度ずつ回る壁紙。
-- rect に angle を渡すと、矩形の中心のまわりに回り、縁にアンチエイリアスが付く。

local CELL = 56           -- 格子の間隔（ピクセル）
local SIZE = 34           -- 四角形の一辺（ピクセル）
local SPEED = 0.25        -- 波が 1 秒に進む周期の数
local WAVELENGTH = 0.45   -- 波長（画面の高さに対する割合）
local BACKGROUND = { 0.05, 0.05, 0.07 }
local FROM = { 0.30, 0.55, 0.95 }
local TO = { 0.95, 0.45, 0.55 }

local sqrt, cos, pi = math.sqrt, math.cos, math.pi
local t = 0

function update(dt)
  t = t + dt
end

function draw()
  clear(BACKGROUND[1], BACKGROUND[2], BACKGROUND[3])
  local w, h = screen.w, screen.h
  local cx, cy = w / 2, h / 2
  local cols, rows = math.ceil(w / CELL) + 1, math.ceil(h / CELL) + 1
  local ox, oy = (w - cols * CELL) / 2, (h - rows * CELL) / 2

  for j = 0, rows - 1 do
    local y = oy + (j + 0.5) * CELL
    for i = 0, cols - 1 do
      local x = ox + (i + 0.5) * CELL
      local d = sqrt((x - cx) ^ 2 + (y - cy) ^ 2) / h
      -- k は 0 → 1 → 0 となめらかに動く。1 周期で 90 度回る。
      local phase = (t * SPEED - d / WAVELENGTH) % 1
      local k = 0.5 - 0.5 * cos(phase * 2 * pi)
      local s = SIZE * (0.55 + 0.45 * k)
      color(
        FROM[1] + (TO[1] - FROM[1]) * k,
        FROM[2] + (TO[2] - FROM[2]) * k,
        FROM[3] + (TO[3] - FROM[3]) * k,
        0.9
      )
      rect(x - s / 2, y - s / 2, s, s, phase * pi / 2)
    end
  end
end
