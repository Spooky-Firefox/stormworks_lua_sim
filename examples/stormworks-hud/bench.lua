-- Interactive test bench for the radar controller (`swlua sim` in this
-- folder). Recreates the vehicle wiring: the composite input alternates
-- every tick between the radar (bool 9 = false) and the physics sensor plus
-- monitor data (bool 9 = true). Outputs 1 and 2 drive the radar pivot.

local radar

-- Targets around the vehicle's flight path; extend freely.
function addTargets()
    world.addTarget{name = "bandit", pos = {400, 450, 3500}, vel = {-15, 0, -80}}
    world.addTarget{name = "circler", pos = {-1500, 600, 2500}, vel = {0, 0, 70}, turn_rate = 6}
    world.addTarget{name = "boat", pos = {900, 0, 1800}, vel = {5, 0, 3}}
end

-- Builds the world radar from the same properties the script reads.
function addRadar(sim)
    return world.addRadar{
        fov_x = sim:getProperty("radar_yaw_fov"),
        fov_y = sim:getProperty("radar_pitch_fov"),
        range = sim:getProperty("radar_max_range"),
        noise = 0.005,
        yaw_output = 1,
        pitch_output = 2,
        slew = sim:getProperty("radar_pivot_speed"),
    }
end

-- Writes one tick of the multiplexed composite input.
function feedInputs(sim, tick, radar, look)
    if tick % 2 == 0 then
        world.writeRadar(radar, 1, 1)
        sim:setBool(9, false)
    else
        world.writePhysics(1)
        local size, t = sim:getScreenSize(), sim:getTouch()
        sim:setNumber(19, size[1])
        sim:setNumber(20, size[2])
        sim:setNumber(21, t.x)
        sim:setNumber(22, t.y)
        sim:setNumber(23, look and look[1] or 0)
        sim:setNumber(24, look and look[2] or 0)
        sim:setBool(1, t.down)
        sim:setBool(9, true)
    end
end

function onSimInit(sim)
    world.setVehicle{pos = {0, 300, 0}, heading = 0, speed = 90}
    radar = addRadar(sim)
    addTargets()
end

function onSimTick(sim, tick)
    feedInputs(sim, tick, radar)
end
