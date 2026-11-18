"""One real job through a packaged noh-mcp, started as an MCP client starts it.

Used by the Linux host probe and by the Windows acceptance of a published release. Standard
library only: it runs in distribution containers, in Linux desktop guests and on the Windows runner.
"""
import json
import os
from pathlib import Path
import queue
import subprocess
import threading
import time


# What a check asks of a packaged MCP server. Checks run from main, which can be newer than the
# package under test: they require only what they use, so an added tool or field never fails them.
# The server refuses unknown arguments, so a check also sends only the methods and arguments the
# oldest package still in a release cycle accepts, and keeps the protocol version that package
# negotiates. Those below are accepted by 0.2.0.
MCP_PROTOCOL = '2025-11-25'
MCP_TOOLS_USED = ('noh_inspect', 'noh_diagnose', 'noh_job', 'noh_export')


def mcp_environment():
    """The reduced environment an MCP client often gives a server: no NOH_* variable, a short PATH."""
    if os.name == 'nt':
        root = os.environ.get('SYSTEMROOT', r'C:\Windows')
        environment = {'SYSTEMROOT': root, 'WINDIR': root, 'PATH': root + r'\System32;' + root}
        for name in ('USERPROFILE', 'LOCALAPPDATA', 'APPDATA', 'TEMP', 'TMP'):
            if os.environ.get(name):
                environment[name] = os.environ[name]
        return environment
    environment = {'HOME': os.environ.get('HOME', '/tmp'), 'PATH': '/usr/bin:/bin', 'LANG': 'C.UTF-8'}
    if os.environ.get('XDG_RUNTIME_DIR'):
        environment['XDG_RUNTIME_DIR'] = os.environ['XDG_RUNTIME_DIR']
    return environment


ROOT_DIRECTORY = os.path.abspath(os.sep)


class McpServer:
    """Newline-delimited JSON-RPC over the server's stdio, every read bounded by one deadline."""

    def __init__(self, command, deadline):
        self.deadline = deadline
        self.process = subprocess.Popen([str(part) for part in command], stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                        env=mcp_environment(), cwd=ROOT_DIRECTORY)
        self.lines, self.errors, self.next_id = queue.Queue(), bytearray(), 0
        self.readers = [threading.Thread(target=target, daemon=True) for target in (self._read, self._read_errors)]
        for reader in self.readers:
            reader.start()

    def _read(self):
        for line in self.process.stdout:
            self.lines.put(line)
        self.lines.put(None)

    def _read_errors(self):
        for line in self.process.stderr:
            self.errors.extend(line[:max(0, 4000 - len(self.errors))])

    def remaining(self):
        left = self.deadline - time.monotonic()
        if left <= 0:
            raise TimeoutError('The MCP server did not answer in time')
        return left

    def send(self, message):
        self.process.stdin.write(json.dumps(message).encode() + b'\n')
        self.process.stdin.flush()

    def request(self, method, params):
        self.next_id += 1
        self.send({'jsonrpc': '2.0', 'id': self.next_id, 'method': method, 'params': params})
        while True:
            try:
                line = self.lines.get(timeout=self.remaining())
            except queue.Empty:
                raise TimeoutError(f'The MCP server did not answer {method} in time') from None
            if line is None:
                raise ValueError(f'The MCP server closed its output during {method}: '
                                 + self.errors.decode(errors='replace'))
            message = json.loads(line)
            if message.get('id') != self.next_id or 'method' in message:
                continue  # notifications, and anything else the server may send first
            if 'error' in message:
                raise ValueError(f'The MCP server refused {method}: {message["error"]}')
            return message['result']

    def call(self, name, arguments):
        result = self.request('tools/call', {'name': name, 'arguments': arguments})
        reply = result.get('structuredContent') or {}
        if result.get('isError'):
            raise ValueError(f'MCP {name} failed: ' + json.dumps(reply)[:1000])
        return reply

    def finish(self, name, arguments):
        """Start a job and follow it to its end: one job at a time, as the server requires."""
        reply = self.call(name, arguments)
        job, revision = reply['job_id'], reply.get('revision', 0)
        while reply.get('state') not in ('succeeded', 'failed', 'cancelled'):
            wait = int(min(10000, max(0, self.remaining() - 1) * 1000))
            reply = self.call('noh_job', {'job_id': job, 'after_revision': revision,
                                          'wait_ms': wait, 'include_result': True})
            revision = reply.get('revision', revision)
        if reply['state'] != 'succeeded':
            raise ValueError(f'MCP {name} job ended {reply["state"]}: ' + json.dumps(reply.get('error'))[:1000])
        return reply

    def close(self):
        """End of input is how a client leaves: the server must stop by itself, with status 0."""
        self.process.stdin.close()
        try:
            code = self.process.wait(timeout=max(1, min(30, self.deadline - time.monotonic())))
        except subprocess.TimeoutExpired:
            raise TimeoutError('The MCP server did not stop after the end of its input') from None
        self.release()
        if code != 0:
            raise ValueError(f'The MCP server stopped with status {code}: ' + self.errors.decode(errors='replace'))
        return code

    def kill(self):
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait(timeout=10)
        self.release()

    def release(self):
        for reader in self.readers:
            reader.join(timeout=5)
        for stream in (self.process.stdin, self.process.stdout, self.process.stderr):
            stream.close()


def mcp_job(command, clip, soundtrack, output, timeout=180):
    """One real job through the packaged MCP server: inspect, exact diagnosis, export."""
    started = time.monotonic()
    server = McpServer(command, started + timeout)
    try:
        hello = server.request('initialize', {'protocolVersion': MCP_PROTOCOL, 'capabilities': {},
                                              'clientInfo': {'name': 'noh-mcp-check', 'version': '1'}})
        if hello.get('protocolVersion') != MCP_PROTOCOL:
            raise ValueError(f'The MCP server answered protocol {hello.get("protocolVersion")!r}')
        server.send({'jsonrpc': '2.0', 'method': 'notifications/initialized'})
        tools = sorted(tool['name'] for tool in server.request('tools/list', {})['tools'])
        missing = sorted(set(MCP_TOOLS_USED) - set(tools))
        if missing:
            raise ValueError('The MCP server does not list ' + ', '.join(missing))
        for path, kind in ((clip, 'video'), (soundtrack, 'wav')):
            server.finish('noh_inspect', {'path': str(path), 'kind': kind})
        request = {'videos': [str(clip)], 'wav': str(soundtrack), 'output': str(output)}
        diagnosis = server.finish('noh_diagnose', {'request': request, 'level': 'exact'})
        exported = server.finish('noh_export', {'request': request, 'diagnosis_job_id': diagnosis['job_id']})
        if (exported.get('result') or {}).get('kind') != 'export' or not Path(output).is_file():
            raise ValueError('The MCP export reported success without its output file')
        exit_code = server.close()
    except BaseException:
        server.kill()
        raise
    return {'protocol': MCP_PROTOCOL, 'server': hello.get('serverInfo'), 'tools': tools,
            'environment': sorted(mcp_environment()), 'working_directory': ROOT_DIRECTORY,
            'exit_code': exit_code, 'seconds': round(time.monotonic() - started, 3)}
