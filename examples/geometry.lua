-- 点が漂い、近い点同士とマウスカーソルを線で結ぶ壁紙。
-- shaderbg 用の geometry.glsl を kagegami に移植したもの。

local N = 25
local BACKGROUND = { 0.039, 0.039, 0.039 }
local DRAW = { 0.773, 0.784, 0.776 }
local RADIUS = 1.8        -- 点の半径（ピクセル）
local LINE_WIDTH = 1.2    -- 線の太さ（ピクセル）
local LINE_THRESH = 0.6   -- 線を引く距離（画面の高さに対する割合）
local SPEED = 2.0
local MAX_LINES = 5       -- 1 点から引く線の上限
local MAX_MOUSE_LINES = 10

local points = {}

function load()
  for i = 1, N do
    points[i] = {
      x = math.random(), y = math.random(),
      vx = math.random() - 0.5, vy = math.random() - 0.5,
    }
  end
end

function update(dt)
  local step = dt * SPEED / screen.h
  for i = 1, N do
    local p = points[i]
    p.x = (p.x + p.vx * step) % 1
    p.y = (p.y + p.vy * step) % 1
  end
end

local sqrt = math.sqrt

function draw()
  local w, h = screen.w, screen.h
  local thresh = LINE_THRESH * h
  local r, g, b = DRAW[1], DRAW[2], DRAW[3]

  clear(BACKGROUND[1], BACKGROUND[2], BACKGROUND[3])
  -- 元のシェーダーは「背景 + 色 × 不透明度 × 0.5」の足し算なので、加算合成で描く。
  blend("add")

  local mcount = 0
  for i = 1, N do
    local ax, ay = points[i].x * w, points[i].y * h

    color(r, g, b, 0.5)
    circle(ax, ay, RADIUS)

    local count = 0
    for j = i + 1, N do
      local bx, by = points[j].x * w, points[j].y * h
      local d = sqrt((ax - bx) ^ 2 + (ay - by) ^ 2)
      if d < thresh then
        color(r, g, b, (1 - d / thresh) * 0.5)
        line(ax, ay, bx, by, LINE_WIDTH)
        count = count + 1
        if count >= MAX_LINES then break end
      end
    end

    if mouse.inside and mcount < MAX_MOUSE_LINES then
      local d = sqrt((ax - mouse.x) ^ 2 + (ay - mouse.y) ^ 2)
      if d < thresh then
        color(r, g, b, (1 - d / thresh) * 0.75)
        line(ax, ay, mouse.x, mouse.y, LINE_WIDTH)
        mcount = mcount + 1
      end
    end
  end
end
