"""Turn on libOsiris' built-in runtime log in a running Baldur's Gate 3, then detach.

macOS-only: drives lldb against the macOS build (see docs/platforms.md).

Usage (game running, at the main menu):

    lldb -b -o "command script import tools/osiris-trace/enable_log.py" \
         -o "osiris_enable_log .cache/traces/osiris.log"

What it does (see docs/research/osiris-vm.md, "Runtime trace plan"):
  1. attaches to the game process,
  2. breaks once on `COsiris::InitGame` (runs when a story starts, e.g. on New Game) or
     `COsiris::Load` (loading a save),
  3. calls `COsiris::OpenLogFile(path, 0)` (which sets `COsiris::m_LogEnabled`) and ORs the
     logging bits into `_ParserFlags` (InitGame keeps bits 16-24 when it resets the flags),
  4. removes the breakpoint and detaches. The game keeps logging on its own; nothing is
     written to the install, only process memory of this session is changed.

`osiris_trace_probes LOG PROBE_LOG` does the same but stays attached and logs a few engine
functions (see PROBES) to PROBE_LOG: arguments on entry and, where useful, the return value.
It detaches when the file `.cache/traces/STOP` appears or the game exits:

    lldb -b -o "command script import tools/osiris-trace/enable_log.py" \
         -o "osiris_trace_probes .cache/traces/combat.log .cache/traces/combat-probes.log"
"""

import os
import time

import functools

import lldb

print = functools.partial(print, flush=True)  # noqa: A001 - lldb redirects stdout to a file

PROCESS_NAME = "Baldur's Gate 3"
# bit 19: rule firings, actions, calls, queries, INIT/EXIT; bit 24: rejected duplicate inserts.
# bit 20 (DB contents) and bit 21 (failed conditions) are very verbose; opt in via env var.
DEFAULT_FLAGS = (1 << 19) | (1 << 24)


def _symbol_address(target, name):
    for ctx in target.FindFunctions(name):
        sym = ctx.GetSymbol()
        if sym.IsValid():
            return sym.GetStartAddress().GetLoadAddress(target)
    for ctx in target.FindSymbols(name):
        sym = ctx.GetSymbol()
        if sym.IsValid():
            return sym.GetStartAddress().GetLoadAddress(target)
    return None


def osiris_enable_log(debugger, command, result, internal_dict):
    _trace(debugger, command.strip() or ".cache/traces/osiris.log", None, result)


def osiris_trace_probes(debugger, command, result, internal_dict):
    args = command.split()
    log = args[0] if args else ".cache/traces/osiris.log"
    probes = args[1] if len(args) > 1 else ".cache/traces/probes.log"
    _trace(debugger, log, os.path.abspath(probes), result)


def _trace(debugger, log_path, probe_log, result):
    log_path = os.path.abspath(log_path)
    os.makedirs(os.path.dirname(log_path), exist_ok=True)
    flags = int(os.environ.get("OSIRIS_LOG_FLAGS", str(DEFAULT_FLAGS)), 0)

    debugger.SetAsync(False)
    target = debugger.CreateTarget("")
    error = lldb.SBError()
    listener = debugger.GetListener()
    process = target.AttachToProcessWithName(listener, PROCESS_NAME, False, error)
    if not error.Success():
        result.SetError(f"attach failed: {error}")
        return
    print(f"attached to pid {process.GetProcessID()}")

    bp = target.BreakpointCreateByName("COsiris::InitGame")
    bp_load = target.BreakpointCreateByName("COsiris::Load")
    print(
        f"breakpoints: COsiris::InitGame {bp.GetNumLocations()}, "
        f"COsiris::Load {bp_load.GetNumLocations()} location(s)"
    )
    if bp.GetNumLocations() == 0:
        result.SetError("COsiris::InitGame not found (libOsiris not loaded?)")
        process.Detach()
        return
    story_bps = {bp.GetID(), bp_load.GetID()}

    print("waiting for a story to start (New Game or load a save in BG3)...")
    while True:
        process.Continue()
        state = process.GetState()
        if state in (lldb.eStateExited, lldb.eStateDetached, lldb.eStateCrashed):
            result.SetError(f"process ended while waiting: {lldb.SBDebugger.StateAsCString(state)}")
            return
        thread = next(
            (t for t in process if t.GetStopReason() == lldb.eStopReasonBreakpoint
             and t.GetStopReasonDataAtIndex(0) in story_bps),
            None,
        )
        if thread is not None:
            break

    frame = thread.GetFrameAtIndex(0)
    this = frame.FindRegister("x0").GetValueAsUnsigned()
    open_log = _symbol_address(target, "COsiris::OpenLogFile(wchar_t const*, unsigned int)")
    if open_log is None:
        open_log = _symbol_address(target, "COsiris::OpenLogFile")
    loading = thread.GetStopReasonDataAtIndex(0) == bp_load.GetID()
    print(
        f"{'Load' if loading else 'InitGame'} hit: this=0x{this:x}, OpenLogFile at 0x{open_log:x}"
    )

    escaped = log_path.replace("\\", "\\\\").replace('"', '\\"')
    expr = (
        f'((void (*)(void *, const wchar_t *, unsigned int))0x{open_log:x})'
        f'((void *)0x{this:x}, L"{escaped}", 0u)'
    )
    opts = lldb.SBExpressionOptions()
    opts.SetIgnoreBreakpoints(True)
    opts.SetTimeoutInMicroSeconds(10_000_000)
    value = frame.EvaluateExpression(expr, opts)
    if value.GetError().Fail() and "no result" not in str(value.GetError()):
        print(f"warning: OpenLogFile call reported: {value.GetError()}")

    flags_addr = _symbol_address(target, "_ParserFlags")
    if flags_addr is None:
        flags_addr = _symbol_address(target, "ParserFlags")
    err = lldb.SBError()
    if flags_addr is None:
        print("warning: _ParserFlags not found; only the default log level is active")
        target.BreakpointDelete(bp.GetID())
        process.Detach()
        return
    _or_flags(process, flags_addr, flags)
    if loading:
        # Load may reset the flags like InitGame does; set them again once it has returned.
        thread.StepOut()
        _or_flags(process, flags_addr, flags)

    enabled_addr = _symbol_address(target, "COsiris::m_LogEnabled")
    if enabled_addr is not None:
        enabled = process.ReadUnsignedFromMemory(enabled_addr, 1, err)
        print(f"COsiris::m_LogEnabled = {enabled}")

    for bp_id in story_bps:
        target.BreakpointDelete(bp_id)
    if probe_log is not None:
        _run_probes(debugger, target, process, probe_log)
        return
    process.Detach()
    print(f"detached; Osiris now logs to {log_path}")


def _or_flags(process, flags_addr, flags):
    err = lldb.SBError()
    current = process.ReadUnsignedFromMemory(flags_addr, 4, err)
    new = current | flags
    process.WriteMemory(flags_addr, new.to_bytes(4, "little"), err)
    print(f"_ParserFlags 0x{current:08x} -> 0x{new:08x} ({'ok' if err.Success() else err})")


# Engine functions logged by `osiris_trace_probes`: (label, file address in the arm64 slice of
# 4.1.1.7631656, registers logged on entry, whether to log w0 on return). Addresses are from
# the symbol index (scripts/symbol-index.sh); local symbols with long template signatures are
# easier to hit by address than by name.
PROBES = [
    ("esv::combat::System::NewInitiativeRoll", 0x10528E328, ["x0", "x1", "x2", "x3", "x4"], True),
    ("esv::combat::SystemHelper::RollDie", 0x1052684B0, ["x0", "x1", "w2", "w3"], True),
    ("esv::StatsSystem::ApplyDamage", 0x1057C3A94, ["x0", "x1", "x2", "x3"], False),
]
# Stop logging a probe after this many hits, in case it turns out to be hot.
MAX_HITS = 5000

_probe_state = {}


def _probe_write(line):
    out = _probe_state["out"]
    out.write(f"{time.monotonic() - _probe_state['t0']:.3f} {line}\n")
    out.flush()


def probe_entry(frame, bp_loc, internal_dict):
    bp = bp_loc.GetBreakpoint()
    label, regs, want_ret = _probe_state["probes"][bp.GetID()]
    hits = _probe_state["hits"]
    hits[label] = hits.get(label, 0) + 1
    if hits[label] > MAX_HITS:
        bp.SetEnabled(False)
        _probe_write(f"{label} disabled after {MAX_HITS} hits")
        return False
    thread = frame.GetThread()
    values = " ".join(
        f"{r}=0x{frame.FindRegister(r).GetValueAsUnsigned():x}" for r in regs
    )
    _probe_write(f"enter {label} tid={thread.GetThreadID()} {values}")
    if want_ret:
        target = frame.GetThread().GetProcess().GetTarget()
        lr = frame.FindRegister("lr").GetValueAsUnsigned()
        ret = target.BreakpointCreateByAddress(lr)
        ret.SetOneShot(True)
        ret.SetThreadID(thread.GetThreadID())
        ret.SetScriptCallbackFunction("enable_log.probe_return")
        _probe_state["returns"][ret.GetID()] = label
    return False


def probe_return(frame, bp_loc, internal_dict):
    label = _probe_state["returns"].pop(bp_loc.GetBreakpoint().GetID(), "?")
    w0 = frame.FindRegister("w0").GetValueAsUnsigned()
    signed = w0 - (1 << 32) if w0 & 0x80000000 else w0
    _probe_write(f"return {label} tid={frame.GetThread().GetThreadID()} w0={signed}")
    return False


def _run_probes(debugger, target, process, probe_log):
    os.makedirs(os.path.dirname(probe_log), exist_ok=True)
    main = next(
        (m for m in target.module_iter() if m.GetFileSpec().GetFilename() == PROCESS_NAME),
        None,
    )
    if main is None:
        print("warning: main executable module not found; no probes")
        process.Detach()
        return
    _probe_state.update(
        out=open(probe_log, "a", encoding="utf-8"),
        t0=time.monotonic(),
        probes={},
        returns={},
        hits={},
    )
    _probe_write(f"attach pid={process.GetProcessID()}")
    for label, file_addr, regs, want_ret in PROBES:
        load = main.ResolveFileAddress(file_addr).GetLoadAddress(target)
        bp = target.BreakpointCreateByAddress(load)
        bp.SetScriptCallbackFunction("enable_log.probe_entry")
        _probe_state["probes"][bp.GetID()] = (label, regs, want_ret)
        print(f"probe {label} at 0x{load:x}")

    stop_file = os.path.abspath(".cache/traces/STOP")
    if os.path.exists(stop_file):
        os.remove(stop_file)
    print(f"probing; create {stop_file} to detach")
    debugger.SetAsync(True)
    listener = debugger.GetListener()
    process.Continue()
    event = lldb.SBEvent()
    while True:
        listener.WaitForEvent(1, event)
        state = process.GetState()
        if state in (lldb.eStateExited, lldb.eStateDetached, lldb.eStateCrashed):
            print(f"process ended: {lldb.SBDebugger.StateAsCString(state)}")
            break
        if state == lldb.eStateStopped:
            # A stop the callbacks didn't swallow (e.g. a signal); keep the game running.
            process.Continue()
        if os.path.exists(stop_file):
            os.remove(stop_file)
            process.Stop()
            time.sleep(0.5)
            for bp_id in list(_probe_state["probes"]) + list(_probe_state["returns"]):
                target.BreakpointDelete(bp_id)
            process.Detach()
            print("detached")
            break
    _probe_write("end")
    _probe_state["out"].close()


def __lldb_init_module(debugger, internal_dict):
    debugger.HandleCommand(
        "command script add -f enable_log.osiris_enable_log osiris_enable_log"
    )
    debugger.HandleCommand(
        "command script add -f enable_log.osiris_trace_probes osiris_trace_probes"
    )
