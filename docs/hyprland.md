# Hyprland and Omarchy

One-shot commands suit key bindings. `--no-wait` returns once MELCloud has the command,
without spending further requests on checking it; the TUI or `klima status` shows the result.
Every press is a command to MELCloud and counts against the
[request budget](../README.md#request-budget). These use `SUPER + CTRL + ALT` with A, the arrows
and K, which Omarchy leaves free; it binds a few other keys with the same modifiers, such as R,
T and W.

## Hyprland 0.55 or later

In `~/.config/hypr/hyprland.lua`:

```lua
local function klima(keys, what, command)
  hl.bind(keys, hl.dsp.exec_cmd(command), { description = what })
end
klima("SUPER + CTRL + ALT + A", "Air conditioner on or off", "klima toggle --no-wait")
klima("SUPER + CTRL + ALT + UP", "Air conditioner warmer", "klima temp +0.5 --no-wait")
klima("SUPER + CTRL + ALT + DOWN", "Air conditioner cooler", "klima temp -0.5 --no-wait")
klima("SUPER + CTRL + ALT + K", "Air conditioner", "kitty --class klima klima")
hl.window_rule({
  match = { class = "^klima$" },
  float = true,
  center = true,
  size = { 1100, 720 },
})
```

The last binding opens the TUI in a terminal with the class `klima`, which the rule floats; with
Alacritty that is `alacritty --class klima -e klima`, with foot `foot --app-id klima klima`.

## Omarchy

Its helpers do the same in `~/.config/hypr/bindings.lua`, with its own launcher for TUIs, which
gives the window the class `org.omarchy.klima`:

```lua
o.bind("SUPER + CTRL + ALT + A", "Air conditioner on or off", "klima toggle --no-wait")
o.bind("SUPER + CTRL + ALT + UP", "Air conditioner warmer", "klima temp +0.5 --no-wait")
o.bind("SUPER + CTRL + ALT + DOWN", "Air conditioner cooler", "klima temp -0.5 --no-wait")
o.bind("SUPER + CTRL + ALT + K", "Air conditioner", { tui = "klima", focus = true })
```

and at the end of `~/.config/hypr/hyprland.lua`:

```lua
o.window("org.omarchy.klima", { float = true, center = true, size = { 1100, 720 } })
```

## Hyprland 0.53 or 0.54

In `hyprland.conf` (earlier versions write window rules differently):

```ini
bind = SUPER CTRL ALT, A, exec, klima toggle --no-wait
bind = SUPER CTRL ALT, UP, exec, klima temp +0.5 --no-wait
bind = SUPER CTRL ALT, DOWN, exec, klima temp -0.5 --no-wait
bind = SUPER CTRL ALT, K, exec, kitty --class klima klima
windowrule = match:class ^klima$, float on, center on, size 1100 720
```

## Notifications

To see what a binding did, make it send the first line of klima's answer as a notification. In
place of the A binding above, in Lua (on Omarchy, with `o.bind` in place of `klima`):

```lua
klima("SUPER + CTRL + ALT + A", "Air conditioner on or off",
  [[notify-send klima "$(klima toggle --no-wait 2>&1 | head -n 1)"]])
```

or in `hyprland.conf`:

```ini
bind = SUPER CTRL ALT, A, exec, notify-send klima "$(klima toggle --no-wait 2>&1 | head -n 1)"
```
