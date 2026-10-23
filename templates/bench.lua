-- Test bench for `swlua run` / `swlua sim`. Runs in its own Lua state
-- (full standard library) next to your script and drives its inputs.
--
--   sim:setNumber(ch, v)      sim:setBool(ch, b)       hold an input value
--   sim:getOutputNumber(ch)   sim:getOutputBool(ch)    read script outputs
--   sim:setProperty(name, v)  sim:touch(x, y, down[, 2]) sim:log(...)
--   sim:stop()                end a `swlua run`

function onSimInit(sim)
    sim:setProperty("Gain", 2)
end

function onSimTick(sim, tick)
    -- Simulated sensor: a slow sine wave on channel 7.
    sim:setNumber(7, math.sin(tick / 60))
end

function onSimAfterTick(sim, tick)
    if tick == 120 then
        sim:log("output 1 after 2s:", sim:getOutputNumber(1))
    end
end
