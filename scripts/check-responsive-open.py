"""Native smoke test: first text, editable readiness, cancellation and large-file save.
Run against target/release/featherpad.exe; touches only tmp/responsive-open/.
"""
import ctypes as c
from ctypes import wintypes as w
import importlib.util
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import time

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('bench', Path(__file__).with_name('benchmark-editors.py'))
b = importlib.util.module_from_spec(spec)
spec.loader.exec_module(b)
u = b.user
u.EnumChildWindows.argtypes = [w.HWND, b.callback, w.LPARAM]
u.GetClassNameW.argtypes = [w.HWND, w.LPWSTR, c.c_int]
u.GetWindowLongW.argtypes = [w.HWND, c.c_int]
u.SendMessageW.argtypes = [w.HWND, w.UINT, c.c_size_t, c.c_ssize_t]
u.SendMessageW.restype = c.c_ssize_t


def editors(hwnd):
    found = []
    @b.callback
    def visit(child, _):
        name = c.create_unicode_buffer(128)
        u.GetClassNameW(child, name, 128)
        if name.value.upper() == 'RICHEDIT50W' and u.IsWindowVisible(child):
            found.append(child)
        return True
    u.EnumChildWindows(hwnd, visit, 0)
    return found


def editor(hwnd):
    return next(iter(editors(hwnd)), None)


def wait_for(check, timeout=60):
    began = time.perf_counter()
    while time.perf_counter() - began < timeout:
        result = check()
        if result:
            return result
        time.sleep(.01)
    raise AssertionError('Timed out')


def measure(exe, path):
    expected = path.read_text(encoding='utf-8').replace('\r\n', '\n').replace('\r', '\n')
    began = time.perf_counter()
    proc = subprocess.Popen([str(exe), str(path)], cwd=exe.parent)
    try:
        hwnd = wait_for(lambda: b.window_for(proc.pid, path.name))
        first = (time.perf_counter() - began) * 1000
        text_at = None
        maximum_reply = 0
        while time.perf_counter() - began < 90:
            started = time.perf_counter()
            reply = c.c_size_t()
            ok = u.SendMessageTimeoutW(hwnd, 0, 0, 0, 2, 1000, c.byref(reply))
            maximum_reply = max(maximum_reply, (time.perf_counter() - started) * 1000)
            edit = editor(hwnd)
            length = u.SendMessageW(edit, 0xE, 0, 0) if edit and ok else 0
            if length and text_at is None:
                text_at = (time.perf_counter() - began) * 1000
            if length and not u.GetWindowLongW(edit, -16) & 0x800:
                # Verify the whole buffer: a temporary internal read-only toggle is not completion.
                contents = c.create_unicode_buffer(length + 1)
                u.SendMessageW(edit, 0xD, length + 1, c.addressof(contents))
                if contents.value.replace('\r\n', '\n').replace('\r', '\n') != expected:
                    time.sleep(.01)
                    continue
                result = dict(file=path.name, window_ms=first, first_text_ms=text_at,
                              editable_ms=(time.perf_counter()-began)*1000, max_sampled_reply_ms=maximum_reply)
                print(json.dumps(result), flush=True)
                return result
            time.sleep(.01)
        raise AssertionError('Never became editable')
    finally:
        proc.terminate()
        proc.wait()


def workflow(exe, root):
    path = root / 'large.md'
    b.fixture(path, 1024 * b.MIB)
    expected = hashlib.sha256(b'INSERTED\n')
    with path.open('rb') as stream:
        while block := stream.read(b.MIB):
            expected.update(block)
    proc = subprocess.Popen([str(exe), str(path)], cwd=exe.parent)
    try:
        hwnd = wait_for(lambda: b.window_for(proc.pid, path.name))
        # Large files must open directly in the editor, without an activation command.
        wait_for(lambda: b.window_for(proc.pid, ' · Region'))
        edit = wait_for(lambda: editor(hwnd))
        wait_for(lambda: u.SendMessageW(edit, 0xE, 0, 0) and not u.GetWindowLongW(edit, -16) & 0x800)
        u.SendMessageW(hwnd, 0x8002, 112, 0)
        assert editor(hwnd) == edit, 'Ctrl+E must stay in the editor in both text modes'
        u.SendMessageW(hwnd, 0x8002, 105, 0)
        wait_for(lambda: len(editors(hwnd)) == 2)
        preview = editors(hwnd)[1]
        wait_for(lambda: u.SendMessageW(preview, 0xE, 0, 0) > 0)
        u.SendMessageW(edit, 0xB1, 0, 0)  # EM_SETSEL
        inserted = c.create_unicode_buffer('INSERTED\r\n')
        u.SendMessageW(edit, 0xC2, 1, c.addressof(inserted))  # marshalled EM_REPLACESEL
        def preview_updated():
            length = u.SendMessageW(preview, 0xE, 0, 0)
            rendered = c.create_unicode_buffer(length + 1)
            u.SendMessageW(preview, 0xD, length + 1, c.addressof(rendered))
            return 'INSERTED' in rendered.value
        wait_for(preview_updated)
        u.SendMessageW(hwnd, 0x8002, 105, 0)
        wait_for(lambda: len(editors(hwnd)) == 1)
        u.SendMessageW(hwnd, 0x8002, 103, 0)
        wait_for(lambda: path.stat().st_size == 1024 * b.MIB + len(b'INSERTED\n'))
        # Wait for save completion and reload, not merely the rename becoming observable.
        wait_for(lambda: b.window_for(proc.pid, ' · Region') and not b.window_for(proc.pid, '* '))
        actual = hashlib.sha256()
        with path.open('rb') as stream:
            while block := stream.read(b.MIB):
                actual.update(block)
        assert actual.digest() == expected.digest()
        edit = wait_for(lambda: editor(hwnd))
        wait_for(lambda: u.SendMessageW(edit, 0xE, 0, 0) and not u.GetWindowLongW(edit, -16) & 0x800)
        # Enter a region again, cancel an in-flight load with New, and ensure no late overwrite.
        u.SendMessageW(hwnd, 0x8002, 126, 0)
        wait_for(lambda: editor(hwnd) is None)
        u.SendMessageW(hwnd, 0x8002, 112, 0)
        u.SendMessageW(hwnd, 0x8002, 101, 0)
        wait_for(lambda: b.window_for(proc.pid, 'Untitled'))
        time.sleep(.5)
        assert b.window_for(proc.pid, 'Untitled')
        assert u.SendMessageW(editor(hwnd), 0xE, 0, 0) == 0
        print('PASS: 1 GiB opens editable; region preview updates live; save SHA-256 preserved; cancellation safe', flush=True)
    finally:
        proc.terminate()
        proc.wait()


if __name__ == '__main__':
    root = b.ROOT / 'tmp' / 'responsive-open'
    root.mkdir(parents=True, exist_ok=True)
    exe = b.ROOT / 'target' / 'release' / 'featherpad.exe'
    results = []
    for name, size in [('table-1MiB.csv', 1), ('markdown-8MiB.md', 8)]:
        path = root / name
        b.fixture(path, size * b.MIB)
        results.append(measure(exe, path))
    workflow(exe, root)
    (root / 'results.json').write_text(json.dumps(results, indent=2), encoding='utf-8')
