# Live fighter parameters — Smash 13.0.5

PR [#7](https://github.com/Deklou/Direct_Visionary/pull/7) adds a runtime
path for `fighter/common/param/fighter_param.prc`. File export remains separate.

## Addresses and layout

Checked in Ghidra against the Smash 13.0.5 `main` (image base `0x7100000000`):

| Item | Address / offset |
| --- | --- |
| PRC loader | `0x7103720EC0` (`main+0x3720EC0`) |
| Global parameter owner | `main+0x52BC3B0` |
| Fighter table | 94 rows of `0x628` bytes |
| `air_speed_x_stable` | row `+0x60` (`f32`) |
| `weight` | row `+0x80` (`f32`) |

`app::sv_fighter_util::get_default_fighter_param_walk_speed_max`
(`0x71022822B0`) and `get_default_fighter_param_air_brake_x`
(`0x7102282370`) read this table directly; the latter reads row `+0x64`.
Ghidra decompiles the PRC loader with a boolean return.
These addresses must not be reused for another game build.

The [skyline-smash `FighterParamTable` layout](https://github.com/ultimate-research/skyline-smash/blob/master/src/params/fighter_param.rs)
contains 369 fields, including `fighter_kind` (read-only).
`fighter_trait_fields.rs` maps the remaining 368 fields by type and offset.
The offsets fit one `0x628` row without overlap; this is a static check,
not a gameplay test for all 368 values.

## How it works

1. `src/roster/traits_view.rs` edits the same sparse fields used by export.
   Opening Roster preselects the main editor's fighter without overwriting
   a manual selection on every frame.
2. `src/app.rs` sends complete `trait_rules` snapshots through
   `src/game_link.rs` over the existing TCP link.
3. `plugins/slight_replica/src/slight/fighter_traits.rs` validates the game
   version, table size and Mario row hash, then writes typed values
   through the existing game-frame callback.
4. Before replacing or removing a rule, it restores the saved value if
   the field still contains its own write. It also recognizes when the game
   restores base values in the same table location; unknown external writes
   are left alone.

The initial `WorkModule::get_param_float` hook was discarded: diagnostic
logs showed `attribute_reads=0`. No replacement hook was required.

## Tests and limits

Ryujinx 1.1.1403, Smash 13.0.5: Bowser (`koopa`) was tested in game.
The log shows six fields written: `weight`, `air_speed_x_stable`,
`dash_speed`, `jump_y`, `jump_speed_x`, and `air_accel_x_mul`.
The second update restored and reapplied all six:
`matched=1/1 written=6 restored=6 changed_elsewhere=0 skipped=0`.
The fighter preselection was also checked in the desktop UI.

Other fields and integer, boolean and hash variants have not all been
individually tested in game. Some values are cached when the fighter
spawns and may require a new match or game restart.
Costume-backed characters still share their donor fighter's parameter row.

Diagnostic markers in `sd:/slight/diag.txt`: `trait rules applied`,
`TRAITS <fighter> <field>`, `TRAITS table snapshot`, `back_to_base`,
`changed_elsewhere`, and `skipped`.
