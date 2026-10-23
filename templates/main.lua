-- Entry point. `swlua build` bundles required modules and minifies the
-- result into out/main.lua, ready to paste into a microcontroller.

require("util")

local gain = property.getNumber("Gain")
local value = 0

function onTick()
    value = input.getNumber(7) * gain
    output.setNumber(1, value)
    output.setBool(1, value > 0)
end

function onDraw()
    local w, h = screen.getWidth(), screen.getHeight()
    screen.setColor(0, 20, 40)
    screen.drawClear()
    screen.setColor(0, 200, 120)
    local bar = clamp(value / gain, -1, 1) * (h / 2 - 2)
    if bar >= 0 then
        screen.drawRectF(w - 6, h / 2 - bar, 4, bar)
    else
        screen.drawRectF(w - 6, h / 2, 4, -bar)
    end
    screen.setColor(255, 255, 255)
    screen.drawText(2, 2, string.format("%.2f", value))
end
