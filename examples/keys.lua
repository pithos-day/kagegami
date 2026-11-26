-- 矢印キーか WASD で円を動かす壁紙。keyboard.down の例。
-- キーが届くのは、壁紙にキーボードのフォーカスがあるとき（壁紙をクリックしたときなど）だけ。

local SPEED = 400 -- ピクセル / 秒
local x, y

function load()
  x, y = screen.w / 2, screen.h / 2
end

local function held(a, b)
  return (keyboard.down(a) or keyboard.down(b)) and 1 or 0
end

function update(dt)
  local fast = (keyboard.down("lshift") or keyboard.down("rshift")) and 2 or 1
  x = x + (held("right", "d") - held("left", "a")) * SPEED * fast * dt
  y = y + (held("down", "s") - held("up", "w")) * SPEED * fast * dt
  x = math.max(0, math.min(screen.w, x))
  y = math.max(0, math.min(screen.h, y))
end

function draw()
  clear(0.04, 0.04, 0.06)
  if keyboard.down("space") then
    color(1, 0.8, 0.3, 0.9)
  else
    color(0.4, 0.7, 1, 0.9)
  end
  circle(x, y, 30)
end
