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
| `swlua sim [file] [--listen addr] [--paused]` | Browser debugger |

`run` and `sim` share these options: `--screen 3x2|96x64`, `--bench bench.lua`, `--prop "Max Speed=12"`, `--no-touch`, and `-m/--minified`. With `--minified` the simulator runs the bundled and minified output instead of your source, which checks that the build behaves the same.

Minifier options: `--keep-locals`, `--keep-globals`, `--no-hoist`, `--no-constants`, `--whitespace-only`, `--pretty` (indented output for inspection), and `--no-minify` (bundle only).

## The minifier

The minifier parses the whole Lua 5.3 grammar (goto, integer division, bitwise operators) and resolves scopes before it changes any names:

- **Comments and whitespace** are removed. A space is kept only where two tokens would merge, such as `a- -b` or `1 ..x`.
- **Parentheses** are rebuilt from operator precedence. Parentheses that change meaning are kept, such as `(f())`, which truncates to one value.
- **Locals** get the shortest names, with the most-used variables named first. Variables whose lifetimes don't overlap share a name.
- **Globals your script defines** are renamed too. Callbacks (`onTick`, `onDraw`, `httpReply`) and the game API are never renamed. Renaming switches off if the script uses `_ENV`, `_G`, `rawget` or `load`.
- **Method definitions** `function a:b()` become `function a.b(self)`, so `self` can be shortened as well.
- **API caching**: `screen.drawRect` used three times becomes one `local a=screen.drawRect` plus calls to `a(...)`. This only happens when it saves characters, and never for a table your script writes to.
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

The full bench API is `setNumber`, `setBool`, `getNumber`, `getBool`, `getOutputNumber`, `getOutputBool`, `setProperty`, `touch(x, y, down[, 2])`, `log(...)`, `stop()` and `getScreenSize()`.

A failing `assert` in the bench makes `swlua run` exit with a non-zero code. That lets a bench work as an automated test, for example in CI.

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
| `src/sim/` | simulator and rasteriser |
| `src/server.rs` and `web/index.html` | debugger |
