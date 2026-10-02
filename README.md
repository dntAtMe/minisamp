# mini-samp

A small multiplayer mod for GTA San Andreas (PC, 1.0 US), in the spirit of SA-MP / MTA.

The Rust workspace (`crates/`) is the current implementation; the Visual Studio projects
(`Client`, `ClientLibrary`, `ClientTarget`, `MiniSamp`) are the earlier C++/RakNet prototype.

## What works

- Two or more clients join a server and see each other as peds walking/running/sprinting around
- 20 Hz state sync over UDP with latency compensation (RTT measured by clock echo, remote
  states extrapolated by their end-to-end age)
- Remote players: game-driven animations (go-to tasks by move state) plus per-frame position
  correction, teleport on large errors
- Server admin port with status, packet log, kick and a network simulator (latency, jitter, loss)
- Turn-based JRPG battles: party of nearby players vs Ballas, server-authoritative rules,
  reliable event channel over UDP, arena staging with a fixed camera, command menu, HP bars,
  fight/hit animations, explosion effects and synthesised sound effects

On foot only so far: no vehicles, weapons, real-time damage or chat.

## Battles

Press **F5** (game window focused) near other players to start a fight; everyone within 30 m
joins. On your turn: **W/S** move the cursor, **A/D** pick a target, **F/Enter** confirm,
**Shift** goes back. Skills: Attack, Fire, Heal, Guard, Run. Sound plays only on the focused
client (`MINISAMP_SFX=always` to override).

The server decides everything (`crates/server/src/battle.rs`, unit-tested); clients only
present events and send choices. Battles can be driven headlessly through the admin port:
`battle_start`, `battle_act`, `battles`.

`python scripts/autobattle.py [--loss 20 --latency 80]` plays a whole battle through sa-mcp,
asserting after every action that all clients show the same round and HP. Under 20 % loss and
80 ms latency: 461 packets dropped, 31 reliable resends, 0 mismatches.

## Crates

| Crate | What |
|---|---|
| `crates/shared` | Protocol: `ClientPacket` / `ServerPacket`, `PlayerState`, clock echo (bincode over UDP) |
| `crates/server` | UDP server, 20 Hz snapshots, admin interface (`admin.rs`), netsim (`netsim.rs`) |
| `crates/client-dll` | `minisamp.asi`: net thread + game-thread sync, built on [sa-sdk](https://github.com/dntAtMe/sa-mcp) |
| `crates/injector`, `crates/client-target` | Earlier DLL-injection prototype and a D3D9 test app |

## Running

Requirements: `gta_sa.exe` 1.0 US with an ASI loader, Rust (nightly toolchain from
`rust-toolchain.toml`, target `i686-pc-windows-msvc`).

```powershell
./scripts/deploy.ps1 -GameDir "C:\path\to\GTA San Andreas"   # builds, installs minisamp.asi
target/i686-pc-windows-msvc/release/server.exe                # --port 7777 --admin-port 7778 --bind 0.0.0.0
```

The client is inactive unless `MINISAMP_SERVER` is set, so single-player is unaffected:

```powershell
$env:MINISAMP_SERVER = "127.0.0.1:7777"; $env:MINISAMP_NAME = "Alice"
& "C:\path\to\GTA San Andreas\gta_sa.exe"
```

There is no world setup in the client yet; for development the world comes from
[sa-mcp](https://github.com/dntAtMe/sa-mcp)'s boot config (empty Grove Street, no story).

## Developing with sa-mcp

`.mcp.json` runs sa-mcp from a sibling checkout (`../../sa-mcp`) with this repo's server
configured, so an agent can do the whole loop:

```
server_start
launch_instances {count: 2, env: {MINISAMP_SERVER: "127.0.0.1:7777"},
                  env_per_instance: [{MINISAMP_NAME: "Alice"}, {MINISAMP_NAME: "Bob"}]}
input {instance: 0, wait: false, steps: [{ms: 2000, actions: ["forward", "sprint"]}]}
sync_trace {seconds: 3}            # mean/p95/max error, network lag vs render error
server_netsim {latency_ms: 100, jitter_ms: 30, loss_pct: 5}
plugin_query {instance: 1, module: "minisamp.asi"}   # client internals (sa_debug_json)
screenshot {instance: 1}
```

`minisamp.asi` exports `sa_debug_json` (see `crates/client-dll/src/debug.rs`) and the server
implements sa-mcp's admin contract (`crates/server/src/admin.rs`).

### Sync quality (Alice sprints/turns/walks, Bob watches, `sync_trace` 7.5 s @ 10 Hz)

| Network | mean | p95 | max | snaps |
|---|---|---|---|---|
| localhost, task-only steering | 1.58 m | 2.71 m | 2.89 m | 7 |
| localhost, + per-frame correction | 0.36 m | 0.72 m | 0.84 m | 0 |
| 100 ms + 30 ms jitter + 5 % loss, no latency compensation | 1.99 m | 2.94 m | 3.14 m | 0 |
| same, with latency compensation | 0.66 m | 1.58 m | 3.41 m | 2 |

Remaining error is dead-reckoning overshoot at sharp turns.
