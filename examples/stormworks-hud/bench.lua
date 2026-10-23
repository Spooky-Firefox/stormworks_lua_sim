-- Test bench for https://github.com/Spooky-Firefox/stormworks-HUD (main.lua).
--
-- Reproduces the vehicle's wiring: the script's input alternates every tick
-- between the radar composite (bool 9 = false) and the physics sensor plus
-- monitor data (bool 9 = true). Radar pivot is driven by outputs 1 (yaw) and
-- 2 (pitch), in turns.
--
--   swlua sim ../stormworks-hud/main.lua -b examples/stormworks-hud/bench.lua -s 3x3 --no-touch

local radar

local props = {
    radar_pitch_fov = 0.02,      -- turns
    radar_yaw_fov = 0.02,        -- turns
    radar_max_range = 6000,
    radar_update_rate = 2,       -- ticks per scan step
    radar_search_overlap = 0.1,
    radar_range = 6000,
    radar_target_lifetime = 300, -- ticks
}

function onSimInit(sim)
    for k, v in pairs(props) do
        sim:setProperty(k, v)
    end

    world.setVehicle{pos = {0, 300, 0}, heading = 0, speed = 90}

    radar = world.addRadar{
        fov_x = props.radar_yaw_fov,
        fov_y = props.radar_pitch_fov,
        range = props.radar_max_range,
        noise = 0.005,
        yaw_output = 1,
        pitch_output = 2,
        slew = 1.0, -- turns per second, like a fast robotic pivot
    }

    world.addTarget{name = "bandit", pos = {400, 450, 3500}, vel = {-15, 0, -80}}
    world.addTarget{name = "circler", pos = {-1500, 600, 2500}, vel = {0, 0, 70}, turn_rate = 6}
    world.addTarget{name = "boat", pos = {900, 0, 1800}, vel = {5, 0, 3}}
end

function onSimTick(sim, tick)
    if tick % 2 == 0 then
        world.writeRadar(radar, 1, 1) -- numbers 1-32, bools 1-8
        sim:setBool(9, false)
    else
        world.writePhysics(1)          -- numbers 1-18
        local size = sim:getScreenSize()
        local t = sim:getTouch()
        sim:setNumber(19, size[1])
        sim:setNumber(20, size[2])
        sim:setNumber(21, t.x)
        sim:setNumber(22, t.y)
        sim:setNumber(23, 0)           -- look x (slave mode)
        sim:setNumber(24, 0)           -- look y
        sim:setBool(1, t.down)
        sim:setBool(9, true)
    end
end
