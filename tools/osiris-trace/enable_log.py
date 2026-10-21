"""Turn on libOsiris' built-in runtime log in a running Baldur's Gate 3, then detach.

Usage (game running, at the main menu):

    lldb -b -o "command script import tools/osiris-trace/enable_log.py" \
         -o "osiris_enable_log .cache/traces/osiris.log"

What it does (see docs/research/osiris-vm.md, "Runtime trace plan"):
  1. attaches to the game process,
  2. breaks once on `COsiris::InitGame` (runs when a story starts, e.g. on New Game),
  3. calls `COsiris::OpenLogFile(path, 0)` (which sets `COsiris::m_LogEnabled`) and ORs the
     logging bits into `_ParserFlags` (InitGame keeps bits 16-24 when it resets the flags),
  4. removes the breakpoint and detaches. The game keeps logging on its own; nothing is
     written to the install, only process memory of this session is changed.
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
    log_path = os.path.abspath(command.strip() or ".cache/traces/osiris.log")
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
    print(f"breakpoint on COsiris::InitGame: {bp.GetNumLocations()} location(s)")
    if bp.GetNumLocations() == 0:
        result.SetError("COsiris::InitGame not found (libOsiris not loaded?)")
        process.Detach()
        return

    print("waiting for a story to start (start a New Game in BG3)...")
    while True:
        process.Continue()
        state = process.GetState()
        if state in (lldb.eStateExited, lldb.eStateDetached, lldb.eStateCrashed):
            result.SetError(f"process ended while waiting: {lldb.SBDebugger.StateAsCString(state)}")
            return
        thread = next(
            (t for t in process if t.GetStopReason() == lldb.eStopReasonBreakpoint
             and t.GetStopReasonDataAtIndex(0) == bp.GetID()),
            None,
        )
        if thread is not None:
            break

    frame = thread.GetFrameAtIndex(0)
    this = frame.FindRegister("x0").GetValueAsUnsigned()
    open_log = _symbol_address(target, "COsiris::OpenLogFile(wchar_t const*, unsigned int)")
    if open_log is None:
        open_log = _symbol_address(target, "COsiris::OpenLogFile")
    print(f"InitGame hit: this=0x{this:x}, OpenLogFile at 0x{open_log:x}")

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
    current = process.ReadUnsignedFromMemory(flags_addr, 4, err)
    new = current | flags
    data = new.to_bytes(4, "little")
    process.WriteMemory(flags_addr, data, err)
    print(f"_ParserFlags 0x{current:08x} -> 0x{new:08x} ({'ok' if err.Success() else err})")

    enabled_addr = _symbol_address(target, "COsiris::m_LogEnabled")
    if enabled_addr is not None:
        enabled = process.ReadUnsignedFromMemory(enabled_addr, 1, err)
        print(f"COsiris::m_LogEnabled = {enabled}")

    target.BreakpointDelete(bp.GetID())
    process.Detach()
    print(f"detached; Osiris now logs to {log_path}")


def __lldb_init_module(debugger, internal_dict):
    debugger.HandleCommand(
        "command script add -f enable_log.osiris_enable_log osiris_enable_log"
    )
