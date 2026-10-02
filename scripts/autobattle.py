"""Plays a whole turn-based battle through sa-mcp and checks every client stays consistent.

Talks MCP (JSON-RPC over stdio) to sa-mcp. Assumes the server and >= 2 clients are running
(sa-mcp: server_start + launch_instances with MINISAMP_SERVER). Starts a battle if none is
active, then on each client whose turn it is navigates the menu with `input` like a player
would. After every action it asserts with `plugin_compare` that all clients show the same
round, actor and HP.

usage: python scripts/autobattle.py [--sa-mcp PATH] [--loss PCT] [--latency MS] [--enemies N] [--seed N]
"""

import argparse
import json
import os
import random
import subprocess
import sys
import time

SKILLS = ["Attack", "Fire", "Heal", "Guard", "Run"]
HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_SA_MCP = os.path.join(HERE, "..", "..", "..", "sa-mcp", "target", "i686-pc-windows-msvc", "release", "sa-mcp.exe")


class Mcp:
    def __init__(self, exe):
        self.p = subprocess.Popen([exe], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
        self.id = 0
        self.rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "autobattle", "version": "1"}})

    def rpc(self, method, params):
        self.id += 1
        self.p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params}) + "\n")
        self.p.stdin.flush()
        return json.loads(self.p.stdout.readline())

    def call(self, tool, **args):
        r = self.rpc("tools/call", {"name": tool, "arguments": args})["result"]
        text = r["content"][0].get("text", "")
        if r.get("isError"):
            raise RuntimeError(f"{tool}: {text}")
        try:
            return json.loads(text)
        except json.JSONDecodeError:
            return text


def battle_of(mcp, inst):
    return mcp.call("plugin_query", instance=inst, module="minisamp.asi").get("battle")


def press(mcp, inst, actions):
    """Each action is held for one short step followed by a release step (menus act on edges)."""
    steps = []
    for a in actions:
        steps += [{"ms": 120, "actions": [a]}, {"ms": 120}]
    mcp.call("input", instance=inst, steps=steps, wait=True)


def choose_skill(b, rng):
    # Party members come first in the HP list (combatant id order).
    party = sum(1 for n, _ in b["hp"] if not n.startswith("Ballas"))
    party_low = any(0 < v < 45 for _, v in b["hp"][:party])
    if party_low and rng.random() < 0.7:
        return "Heal"
    return "Fire" if rng.random() < 0.6 else "Attack"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sa-mcp", default=os.environ.get("SA_MCP_EXE", DEFAULT_SA_MCP))
    ap.add_argument("--loss", type=float, default=0.0)
    ap.add_argument("--latency", type=int, default=0)
    ap.add_argument("--enemies", type=int, default=2)
    ap.add_argument("--seed", type=int, default=7)
    ap.add_argument("--max-actions", type=int, default=60)
    args = ap.parse_args()
    rng = random.Random(args.seed)

    mcp = Mcp(os.path.abspath(args.sa_mcp))
    instances = [i["instance"] for i in mcp.call("list_instances")["instances"]]
    if len(instances) < 2:
        sys.exit("need >= 2 running clients")
    mcp.call("server_netsim", latency_ms=args.latency, jitter_ms=args.latency // 4, loss_pct=args.loss)

    if not mcp.call("server_admin", request={"cmd": "battles"})["active"]:
        leader = min(p["id"] for p in mcp.call("server_status")["players"])
        mcp.call("server_admin", request={"cmd": "battle_start", "leader": leader, "enemies": args.enemies, "seed": args.seed})
    time.sleep(1.0)

    actions, mismatches, outcome = 0, [], None
    deadline = time.time() + 300
    while actions < args.max_actions and time.time() < deadline:
        states = {i: battle_of(mcp, i) for i in instances}
        if any(b is None for b in states.values()):
            outcome = next((b and b.get("outcome") for b in states.values() if b), None)
            break
        outcomes = {b.get("outcome") for b in states.values()}
        if outcomes != {None}:
            outcome = outcomes.pop()
            break
        mover = next((i for i, b in states.items() if b["my_turn"]), None)
        if mover is None:
            time.sleep(0.25)
            continue
        b = states[mover]
        skill = choose_skill(b, rng)
        idx = SKILLS.index(skill)
        press(mcp, mover, ["back"] * idx + ["enter_exit"] + (["enter_exit"] if skill in ("Attack", "Fire", "Heal") else []))
        actions += 1
        print(f"action {actions}: instance {mover} ({b['actor']}) {skill}")
        # Results reach every client over the reliable channel; give it time, then compare.
        time.sleep(1.0 + args.latency / 500)
        c = mcp.call("plugin_compare", fields=["battle.id", "battle.round", "battle.hp"])
        if not c["consistent"]:
            time.sleep(1.5)  # allow resends under loss before calling it a mismatch
            c = mcp.call("plugin_compare", fields=["battle.id", "battle.round", "battle.hp"])
            if not c["consistent"]:
                mismatches.append(c)
                print("  MISMATCH:", json.dumps(c["values"]))

    time.sleep(0.5)
    status = mcp.call("server_status")
    finished = mcp.call("server_admin", request={"cmd": "battles"})["finished"]
    last = finished[-1] if finished else None
    mcp.call("server_netsim", latency_ms=0, jitter_ms=0, loss_pct=0)
    print(json.dumps({
        "outcome": outcome,
        "actions_played": actions,
        "rounds": last and last["round"],
        "final_hp": last and [(c["name"], c["hp"]) for c in last["combatants"]],
        "mismatches": len(mismatches),
        "netsim": {"loss_pct": args.loss, "latency_ms": args.latency, "dropped": status["netsim_dropped"]},
        "reliable": {p["name"]: p["reliable"] for p in status["players"]},
    }, indent=2))
    sys.exit(0 if outcome and not mismatches else 1)


if __name__ == "__main__":
    main()
