-- ドラッグで引っぱって離すと、押した位置から玉が飛び出す壁紙。
-- mouse.press_x・mouse.press_y・mouse.dragging の例。どのボタンでもドラッグできる。

local GRAVITY = 600       -- ピクセル / 秒^2
local POWER = 4           -- 引っぱった距離に掛ける速さの倍率
local BOUNCE = 0.8        -- 壁で跳ね返るときに残る速さの割合
local LIFE = 6            -- 玉が消えるまでの秒数
local RADIUS = 8

local balls = {}
local was_dragging = false
local t = 0

local function launch()
  -- 押した位置から、引っぱった向きと反対へ飛ばす。
  local dx = mouse.press_x - mouse.x
  local dy = mouse.press_y - mouse.y
  if dx * dx + dy * dy < 25 then
    return
  end
  balls[#balls + 1] = {
    x = mouse.press_x, y = mouse.press_y,
    vx = dx * POWER, vy = dy * POWER,
    age = 0, hue = t * 0.3 % 1,
  }
end

function update(dt)
  t = t + dt
  if was_dragging and not mouse.dragging then
    launch()
  end
  was_dragging = mouse.dragging

  local w, h = screen.w, screen.h
  local alive = {}
  for _, b in ipairs(balls) do
    b.age = b.age + dt
    b.vy = b.vy + GRAVITY * dt
    b.x = b.x + b.vx * dt
    b.y = b.y + b.vy * dt
    if b.x < RADIUS then b.x, b.vx = RADIUS, -b.vx * BOUNCE end
    if b.x > w - RADIUS then b.x, b.vx = w - RADIUS, -b.vx * BOUNCE end
    if b.y > h - RADIUS then b.y, b.vy = h - RADIUS, -b.vy * BOUNCE end
    if b.age < LIFE then
      alive[#alive + 1] = b
    end
  end
  balls = alive
end

local function hue_rgb(hue)
  local r = math.abs(hue * 6 - 3) - 1
  local g = 2 - math.abs(hue * 6 - 2)
  local b = 2 - math.abs(hue * 6 - 4)
  local c = function(v) return math.max(0, math.min(1, v)) end
  return c(r), c(g), c(b)
end

function draw()
  clear(0.04, 0.04, 0.05)

  blend("add")
  for _, b in ipairs(balls) do
    local r, g, bl = hue_rgb(b.hue)
    local a = 1 - b.age / LIFE
    color(r, g, bl, a * 0.25)
    circle(b.x, b.y, RADIUS * 2.5)
    color(r, g, bl, a)
    circle(b.x, b.y, RADIUS)
  end
  blend("alpha")

  if mouse.dragging then
    -- 引っぱっているゴムと、飛んでいく向きの目安。
    color(1, 1, 1, 0.6)
    line(mouse.press_x, mouse.press_y, mouse.x, mouse.y, 2)
    color(1, 1, 1, 0.25)
    line(mouse.press_x, mouse.press_y,
      2 * mouse.press_x - mouse.x, 2 * mouse.press_y - mouse.y, 1)
    color(1, 1, 1, 0.9)
    circle(mouse.press_x, mouse.press_y, 5)
  end
end
