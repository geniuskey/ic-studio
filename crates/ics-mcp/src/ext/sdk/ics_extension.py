# SPDX-License-Identifier: MIT
"""Helper for writing IC Studio extensions in Python (standard library only).

IC Studio puts this module on PYTHONPATH when it starts an extension's program, so
`from ics_extension import Extension` works without installing anything. You may also copy
this file into your extension (it is MIT licensed).

    from ics_extension import Extension

    ext = Extension()

    @ext.tool("summary")
    def summary(cell: str):
        info = ext.call("layout_summary", cell=cell)   # any built-in IC Studio tool
        ext.log(f"{cell}: {info['shapes']} shapes")
        return {"cell": cell, "shapes": info["shapes"]}

    if __name__ == "__main__":
        ext.run()

A tool returns a JSON-serialisable value (dict, list, str, number) and raises an exception to
report an error. Keyword arguments are the tool's arguments. The protocol is newline-delimited
JSON-RPC 2.0 on stdin / stdout, so never print to stdout yourself: use `ext.log` or stderr.
"""

import inspect
import json
import sys
import traceback

PROTOCOL = 1


class ToolError(Exception):
    """A built-in tool called with `Extension.call` failed."""


class Extension:
    def __init__(self):
        self._tools = {}
        self._next_id = 0
        self._out = sys.stdout
        # Anything printed by mistake goes to stderr instead of corrupting the protocol.
        sys.stdout = sys.stderr
        self.host = {}
        self.dir = None

    def tool(self, name=None):
        """Decorator registering a function as the tool `name` (default: its own name)."""

        def register(fn):
            self._tools[name or fn.__name__] = fn
            return fn

        return register

    # -- calls to IC Studio ------------------------------------------------------------

    def call(self, tool, **arguments):
        """Run a built-in IC Studio tool on the open project and return its JSON result.

        Raises ToolError when the tool fails."""
        self._next_id += 1
        rid = f"x{self._next_id}"
        self._send({"jsonrpc": "2.0", "id": rid, "method": "ics/call", "params": {"name": tool, "arguments": arguments}})
        while True:
            msg = self._read()
            if msg is None:
                raise ToolError("IC Studio closed the connection")
            if msg.get("id") == rid and "method" not in msg:
                if "error" in msg:
                    raise ToolError(msg["error"].get("message", str(msg["error"])))
                result = msg.get("result", {})
                text = "\n".join(p.get("text", "") for p in result.get("content", []))
                if result.get("isError"):
                    raise ToolError(text)
                try:
                    return json.loads(text)
                except ValueError:
                    return text
            # IC Studio does not send requests while waiting for us; ignore anything else.

    def log(self, message, level="info"):
        """Write a line to IC Studio's console."""
        self._send({"jsonrpc": "2.0", "method": "ics/log", "params": {"level": level, "message": str(message)}})

    # -- protocol ------------------------------------------------------------------------

    def _send(self, msg):
        self._out.write(json.dumps(msg) + "\n")
        self._out.flush()

    def _read(self):
        while True:
            line = sys.stdin.readline()
            if not line:
                return None
            line = line.strip()
            if line:
                return json.loads(line)

    def _reply(self, rid, result=None, error=None):
        msg = {"jsonrpc": "2.0", "id": rid}
        if error is not None:
            msg["error"] = error
        else:
            msg["result"] = result
        self._send(msg)

    def _list(self):
        tools = []
        for name, fn in self._tools.items():
            props = {p: {} for p in inspect.signature(fn).parameters if p not in ("self",)}
            tools.append({"name": name, "description": (fn.__doc__ or "").strip(), "inputSchema": {"type": "object", "properties": props}})
        return {"tools": tools}

    def _call(self, params):
        name = params.get("name")
        fn = self._tools.get(name)
        if fn is None:
            return {"content": [{"type": "text", "text": f"unknown tool: {name}"}], "isError": True}
        try:
            value = fn(**(params.get("arguments") or {}))
        except ToolError as e:
            return {"content": [{"type": "text", "text": str(e)}], "isError": True}
        except Exception as e:  # report, keep serving
            traceback.print_exc(file=sys.stderr)
            return {"content": [{"type": "text", "text": f"{type(e).__name__}: {e}"}], "isError": True}
        text = value if isinstance(value, str) else json.dumps(value)
        return {"content": [{"type": "text", "text": text}], "isError": False}

    def run(self):
        """Serve IC Studio's requests until it closes stdin or sends shutdown."""
        while True:
            msg = self._read()
            if msg is None:
                return
            method, rid, params = msg.get("method"), msg.get("id"), msg.get("params") or {}
            if method == "shutdown" or method == "exit":
                return
            if rid is None:
                continue  # other notifications
            if method == "initialize":
                self.host = params.get("hostInfo", {})
                self.dir = params.get("extension", {}).get("dir")
                self._reply(rid, {"protocolVersion": PROTOCOL, "capabilities": {"tools": {}}, "serverInfo": {"name": "ics-extension"}})
            elif method == "tools/list":
                self._reply(rid, self._list())
            elif method == "tools/call":
                self._reply(rid, self._call(params))
            elif method == "ping":
                self._reply(rid, {})
            else:
                self._reply(rid, error={"code": -32601, "message": f"method not found: {method}"})
