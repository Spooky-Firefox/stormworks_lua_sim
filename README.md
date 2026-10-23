# swlua

A command line toolkit for writing **Stormworks microcontroller Lua** outside the game, written in Rust.

- **Build**: bundles `require`d modules into a single script and minifies it to fit the character limit.
- **Check**: lints for typos in API calls (`screen.drawrect`), undefined globals, and libraries the game doesn't have.
- **Simulate**: runs your script headless, or in a browser debugger with a live monitor, touch input, editable composite inputs and properties, and reload on save.
- **Editor support**: `swlua init` writes [lua-language-server](https://luals.github.io/) type definitions for the whole game API, so VS Code, Neovim and other editors get autocomplete and hover docs. This covers what the deprecated LifeBoat extension used to provide.

The simulator runs real Lua 5.3, the version the game uses, through [mlua](https://github.com/mlua-rs/mlua) with a vendored interpreter. You don't need to install Lua.

## Install

```sh
cargo install --git https://github.com/spooky-firefox/stormworks_lua_sim
# or from a checkout:
cargo install --path .
```

## Quick start

```sh
swlua init my_radar --screen 3x2   # project with example script, test bench, editor types
cd my_radar
swlua sim                          # open http://127.0.0.1:7272
swlua build                        # -> out/main.lua, paste into the microcontroller
```

A project looks like this:

```
swlua.toml          project, minifier and simulator settings
src/main.lua        entry point
src/util.lua        require("util") is found next to main.lua or in [project] lib
bench.lua           test bench that drives inputs in the simulator
.swlua/             API type definitions for lua-language-server
.luarc.json         language server config (Lua 5.3, no io/os, …)
```

## Commands

| Command | What it does |
| --- | --- |
| `swlua init [dir] [--screen 3x2]` | Scaffold a project |
| `swlua build [-w] [-o file]` | Bundle and minify `[project] main` into `[project] out`. `-w` rebuilds on save |
| `swlua minify <file\|-> [-o file]` | Minify any file without a project (requires are still bundled) |
| `swlua check [file]` | Lint the script and report its minified size |
| `swlua run [file] -t 600 [--csv out.csv] [--screenshot f.png] [-p]` | Headless simulation |
| `swlua sim [file] [--lan] [--port 7272] [--listen addr] [--paused]` | Browser debugger |

`run` and `sim` share these options: `--screen 3x2|96x64`, `--bench bench.lua`, `--prop "Max Speed=12"`, `--no-touch`, and `-m/--minified`. With `--minified` the simulator runs the bundled and minified output instead of your source, which checks that the build behaves the same.

Minifier options: `--keep-locals`, `--keep-globals`, `--no-hoist`, `--no-constants`, `--whitespace-only`, `--pretty` (indented output for inspection), and `--no-minify` (bundle only).

## The minifier

The minifier parses the whole Lua 5.3 grammar (goto, integer division, bitwise operators) and resolves scopes before it changes any names:

- **Comments and whitespace** are removed. A space is kept only where two tokens would merge, such as `a- -b` or `1 ..x`.
- **Parentheses** are rebuilt from operator precedence. Parentheses that change meaning are kept, such as `(f())`, which truncates to one value.
- **Locals** get the shortest names. Locals and renamed globals are named together, most-used first. Variables whose lifetimes don't overlap share a name.
- **Globals your script defines** are renamed too. Callbacks (`onTick`, `onDraw`, `httpReply`) and the game API are never renamed. Renaming switches off if the script uses `_ENV`, `_G`, `rawget` or `load`.
- **Method definitions** `function a:b()` become `function a.b(self)`, so `self` can be shortened as well.
- **API caching**: `screen.drawRect` used three times becomes one `local a=screen.drawRect` plus calls to `a(...)`. This only happens when it saves characters, and never for a table your script writes to.
- **Unused definitions**: top-level functions and constants that nothing reads are removed, including those inside bundled modules. This way, a shared library only costs the characters for the parts a script uses. `--keep-unused` turns this off.
- **Repeated literals**: repeated long strings and numbers move into locals when that saves space.
- **Numbers and strings** are written in their shortest form. `0.5` becomes `.5` and `1000000.0` becomes `1e6`. Integers and floats stay distinct, as Lua 5.3 requires. Each string gets whichever quote style needs fewer escapes. A call with one string or table argument loses its parentheses, so `f("x")` becomes `f"x"`.

The test suite runs the original and the minified code side by side in Lua and compares their output. That includes about 400 random expressions that mix every operator.

`require` bundling: each module is inserted once, in dependency order, before the main script.

- A module required as a statement (`require "util"`) becomes a `do … end` block.
- A module whose value is used (`local PID = require "pid"`) runs once, and every use refers to that single result.
- `require("a.b")` looks for `a/b.lua` or `a/b/init.lua`, next to the main file or in a `[project] lib` folder.

## The simulator

The simulator provides the whole microcontroller API:

- `input.*`, `output.*` and `property.*`: 32 number and 32 bool composite channels, plus properties.
- `screen.*`: every drawing call, rasterised in software with alpha blending. This includes `drawTextBox` with word wrap and alignment, and a 4×5 pixel font with 5 px spacing, like the game.
- `map.screenToMap` and `map.mapToScreen`. `drawMap` draws ocean with a 1 km grid, because the world map isn't available offline.
- `async.httpGet`: makes a real request to `localhost:<port>`, and the reply arrives in `httpReply`.
- `debug.log` writes to the log panel. `print` does too, but `check` warns about it because the game doesn't show print output.
- **Sandboxing**: `io`, `os`, `load`, `require` and the other functions the game removes are missing in the simulator too.
- **Hang detection**: a callback that runs for more than 2 seconds is stopped and reported as a probable infinite loop.
- **Errors** stop the script, as in the game. The report shows your real file and line, because the simulator runs your source files rather than the bundle.
- **Monitor touch**: with a screen and `touch = true`, the simulator fills the inputs a monitor's composite output would:

  | Channel | Meaning |
  | --- | --- |
  | number 1–2 | width, height |
  | number 3–4 | touch 1 x, y |
  | number 5–6 | touch 2 x, y |
  | bool 1–2 | touch 1 and 2 pressed |

### Browser debugger (`swlua sim`)

By default the debugger only accepts connections from the same machine, at http://127.0.0.1:7272. If your code lives on another computer, for example a server you edit over SSH, run `swlua sim --lan`. It then listens on every network interface and prints the URL for each network, for example:

```
swlua debugger listening on all interfaces, port 7272  (Ctrl+C to quit)
  http://192.168.1.23:7272   (eth0, subnet 192.168.1.0/24)
```

Open that URL from any machine on the same network. The debugger has no login, so only use `--lan` on networks you trust. `--port` changes the port, and `--listen 192.168.1.23:8000` binds one exact address.

VS Code Remote-SSH can also forward the default port for you, with no flag needed: open the Ports panel and forward 7272.

- **Monitor**: shows the screen at an adjustable zoom. Left click is touch 1 and right click is touch 2.
- **Run control**: run, pause, single-step (`.`), and reload (`r`). Speed goes from 0.1× to 10×.
- **Inputs**: type a number or click a light to *hold* an input. Held inputs win over the bench, touch data and the config. ✕ releases a held input.
- **Live view**: outputs, properties (editable), HTTP requests, and a log that is also mirrored to the terminal.
- **Reload on save**: picks up changes to the main file, every required module and the bench. Held inputs and edited properties are kept.

### Test bench

The bench is a separate Lua state with the full standard library. It runs alongside your script and drives it, like LifeBoat's simulator hooks:

```lua
function onSimInit(sim)            -- once, before the script loads
    sim:setProperty("Max Speed", 12)
end

function onSimTick(sim, tick)      -- before every onTick
    sim:setNumber(1, math.sin(tick / 60) * 100)  -- fake sensor
    sim:setBool(3, tick % 120 < 60)
end

function onSimAfterTick(sim, tick) -- after onTick/onDraw: assertions
    assert(sim:getOutputNumber(1) <= 12, "speed limit exceeded")
    if tick == 600 then sim:stop() end
end
```

The full bench API is `setNumber`, `setBool`, `getNumber`, `getBool`, `getOutputNumber`, `getOutputBool`, `setProperty`, `getProperty`, `touch(x, y, down[, 2])`, `getTouch([2])`, `log(...)`, `stop()`, `getScreenSize()` and `addController(path[, props])`.

A bench can `require` Lua files that sit next to it, which is handy for helpers shared between benches.

#### Chained microcontrollers

`sim:addController(path, props)` loads another microcontroller script. The path is relative to the bench, and the controller uses the `lib` folders of its own `swlua.toml`. This lets you test a controller that is fed by another one, for example a HUD fed by a radar processor, or a radar fed by sensors. The bench moves the values between them:

```lua
function onSimInit(sim)
    radar = sim:addController("../radar/src/main.lua", {radar_yaw_fov = 0.02})
end

function onSimTick(sim, tick)
    radar:setInputs(world.physics())         -- setInputs(numbers, bools, num_start, bool_start)
    radar:tick()                             -- runs its onTick; errors stop the simulation
    local nums, bools = radar:getOutputs()
    for i = 1, 19 do sim:setNumber(i, nums[i]) sim:setBool(i, bools[i]) end
end
```

A controller handle also has `setNumber`, `setBool`, `getNumber`, `getBool` (these read its outputs) and `setProperty`. Its `debug.log` output appears in the log with its name in front. Saving any of its files reloads the simulation.

A failing `assert` in the bench makes `swlua run` exit with a non-zero code. That lets a bench work as an automated test, for example in CI.

### 3D world: radar, physics sensor and targets

The bench can also build a small 3D world: your vehicle, moving targets, radars on pivots, and a physics sensor. The bench then passes the sensor readings into whatever input channels your vehicle uses. That makes it possible to develop a radar display or tracker without the game. When a world exists, `swlua sim` shows it in a **3D view** with these features:

- **Camera**: orbit around the vehicle, or look through the radar.
- **Beam**: the radar's beam is drawn as a wireframe.
- **Targets**: drawn with velocity vectors, trails and altitude lines. Targets the radar currently sees turn green.
- **Reported contacts**: red crosses mark the positions the radar reports, noise included, so you can compare them with the true positions.
- **Flying**: click the view, then use W/S for speed, A/D for yaw, ↑/↓ for pitch, Q/E for roll and L to level out.
- **Tables**: live target and physics sensor tables, and a button that adds a target ahead of you.

```lua
function onSimInit(sim)
    world.setVehicle{pos = {0, 300, 0}, heading = 0, speed = 90}   -- flies along its nose
    radar = world.addRadar{fov_x = 0.02, fov_y = 0.02, range = 6000, noise = 0.005,
                           yaw_output = 1, pitch_output = 2, slew = 1}
    world.addTarget{name = "bandit", pos = {400, 450, 3500}, vel = {-15, 0, -80}}
    world.addTarget{name = "circler", pos = {-1500, 600, 2500}, vel = {0, 0, 70}, turn_rate = 6}
end

function onSimTick(sim, tick)
    -- Wire sensors to inputs however your vehicle does, e.g. alternating ticks:
    if tick % 2 == 0 then
        world.writeRadar(radar, 1, 1)   -- numbers 1-32 + bools 1-8
        sim:setBool(9, false)
    else
        world.writePhysics(1)           -- numbers 1-17
        sim:setBool(9, true)
    end
end
```

| Function | |
| --- | --- |
| `world.setVehicle{...}` / `world.getVehicle()` | Body fields: `pos`, `heading`/`pitch`/`roll` (degrees) or `euler`, `speed` (m/s along the nose) or `vel` (world m/s), `yaw_rate`/`pitch_rate`/`roll_rate` (deg/s) |
| `world.addTarget{name=..., ...}` → id | Same body fields. `turn_rate` (deg/s) flies a circle |
| `world.setTarget(id, {...})`, `world.removeTarget(id)`, `world.getTargets()` | |
| `world.addRadar{...}` → id | Radar fields: `fov_x`, `fov_y` (turns, full width), `range`, `noise` (position error σ as a fraction of distance), `yaw_output`/`pitch_output` (script outputs that drive the pivot, in turns), `slew` (turns/s), `hold_ticks`, `offset` (mount position), `angles` (`"vehicle"`, the default, or `"radar"`: the frame contact angles are reported in) |
| `world.aimRadar(id, yaw, pitch)` | Point a radar that has no output channels |
| `world.getRadar(id)` | Returns `{yaw, pitch, contacts = {{slot, target, distance, azimuth, elevation, ticks}}}` |
| `world.writeRadar(id, num_start, bool_start)` | Radar composite. Each of 8 slots gets 4 numbers (distance, azimuth, elevation, ticks since detection) and 1 detected bool. Angles are in turns, relative to the vehicle unless the radar was added with `angles = "radar"` |
| `world.radarValues(id)` | The same values as `writeRadar`, returned as two tables, for passing to a controller |
| `world.toLocal(pos)` | A world position in the vehicle frame (x right, y up, z forward) |
| `world.physics()` / `world.writePhysics(num_start)` | The 17 physics sensor numbers: position (1-3), Euler angles (4-6), local velocity (7-9), angular velocity (10-12), absolute speeds (13-14), local Z (forward) and X (right) tilt (15-16) and compass (17) |

Conventions:

- **World axes**: X is east, Y is up and Z is north, matching the physics sensor.
- **Vehicle axes**: X is right, Y is up and Z is forward.
- **Euler angles**: extrinsic rotations about world X, then Y, then Z, so `R = Rz·Ry·Rx` maps local to world. This matches the game as measured in the Steam guide [How to use the euler x, euler y and euler z outputs of Physics and Astronomy Sensors](https://steamcommunity.com/sharedfiles/filedetails/?id=3302971632); a unit test checks the simulator against the guide's `ijkfromeuler`.
- **Angular velocity** (physics 10-12) is about the world axes, as in the game, not the vehicle's own axes.
- **Radar angles**: positive azimuth is right and positive elevation is up. By default (`angles = "vehicle"`) they are relative to the vehicle, not to the pivoted radar: in game, a radar on a pivot yawed 45° reports a target in its beam centre at 45°. `angles = "radar"` reports them relative to the radar's facing instead.
- **Compass**: 0 is north, east is −0.25.
- **Pivot**: positive yaw turns right.
- **Random numbers**: radar noise is seeded, so runs repeat exactly.

[`examples/stormworks-hud/bench.lua`](examples/stormworks-hud/bench.lua) is a copy of the radar bench from [stormworks-HUD](https://github.com/Spooky-Firefox/stormworks-HUD), which is itself a swlua project with a `radar/` and a `hud/` controller. To run it:

```sh
cd stormworks-HUD/radar && swlua sim   # radar controller with its monitor
cd stormworks-HUD/hud && swlua sim     # HUD, fed by the real radar controller
```

## swlua.toml

```toml
[project]
main = "src/main.lua"
out = "out/main.lua"
lib = ["../shared_lib"]    # extra require() search folders
limit = 8192               # character limit warning threshold

[minify]
rename_locals = true
rename_globals = true
hoist_api = true
hoist_constants = true

[sim]
screen = "3x2"             # 1x1 2x1 2x2 3x2 3x3 5x3 9x5, or pixels like 96x64
bench = "bench.lua"
touch = true
tick_rate = 60

[sim.properties]
"Max Speed" = 12
Enabled = true
Callsign = "ALPHA"

[sim.numbers]              # constant inputs
8 = 1013.25

[sim.bools]
4 = true
```

## Known differences from the game

- Pixel output of lines, circles and the font is approximate. Rectangles follow the game's sizes: `drawRect` covers w+1 × h+1 pixels and `drawRectF` covers w × h.
- `drawMap` has no terrain data.
- The map projection uses a scale of `zoom * 1000 / screen width` metres per pixel.
- `onDraw` runs once per tick for a single monitor.
- The world simulation is kinematic, with no flight physics and no terrain. Nothing blocks line of sight for radars.
- I wrote the physics sensor and radar conventions from memory of how the game works, and haven't measured them in game. The Euler angle convention and the world-frame angular velocity follow the guide linked above. Linear velocity (7-9) is assumed to be in the vehicle's own axes, and the compass to read -0.25 facing east; check those against a real physics sensor.

## Development

```sh
cargo test       # lexer/parser, minifier equivalence tests, simulator
cargo clippy --all-targets
```

The code is organised like this:

| Path | Contents |
| --- | --- |
| `src/lua/` | lexer, AST and parser |
| `src/minify/` | scope resolver, renamer, hoisting, printer |
| `src/bundle.rs` | `require` inlining |
| `src/check.rs` | linter |
| `src/sim/` | simulator, rasteriser, 3D world (`world.rs`) and its bench API (`world_lua.rs`) |
| `src/server.rs` and `web/index.html` | debugger |
