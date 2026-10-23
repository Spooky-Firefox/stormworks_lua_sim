-- Modules are plain Lua files found next to main.lua or in [project] lib.

function clamp(x, lo, hi)
    return math.max(lo, math.min(hi, x))
end
