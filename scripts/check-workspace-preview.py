"""Native smoke test using only generated files and its own app instances."""
import ctypes as c
import importlib.util
from pathlib import Path
import subprocess
import sys
import time

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('native', Path(__file__).with_name('check-responsive-open.py'))
n = importlib.util.module_from_spec(spec)
spec.loader.exec_module(n)
u = n.u


def child(parent, class_name):
    found = []
    @n.b.callback
    def visit(hwnd, _):
        name = c.create_unicode_buffer(128)
        u.GetClassNameW(hwnd, name, len(name))
        if name.value == class_name and u.IsWindowVisible(hwnd):
            found.append(hwnd)
        return True
    u.EnumChildWindows(parent, visit, 0)
    return found[0] if found else None


def text(hwnd):
    length = u.SendMessageW(hwnd, 0xE, 0, 0)
    value = c.create_unicode_buffer(length + 1)
    u.SendMessageW(hwnd, 0xD, length + 1, c.addressof(value))
    return value.value


def modal(pid):
    found = []
    @n.b.callback
    def visit(hwnd, _):
        owner = n.b.w.DWORD()
        u.GetWindowThreadProcessId(hwnd, c.byref(owner))
        name = c.create_unicode_buffer(128)
        u.GetClassNameW(hwnd, name, len(name))
        if owner.value == pid and name.value == '#32770':
            found.append(hwnd)
        return True
    u.EnumWindows(visit, 0)
    return found[0] if found else None


def run():
    root = n.b.ROOT / 'tmp' / 'workspace-smoke'
    (root / 'nested').mkdir(parents=True, exist_ok=True)
    source = '# Parent\n\nBODY_MARKER\n\n## Child\n\nCHILD_MARKER\n\n# Next\n\nNEXT_MARKER\n'
    file = root / 'nested' / 'notes.md'
    file.write_text(source, encoding='utf-8')
    (root / 'other.txt').write_text('Other document', encoding='utf-8')
    exe = n.b.ROOT / 'target' / 'release' / 'featherpad.exe'
    proc = subprocess.Popen([str(exe), str(file)], cwd=exe.parent)
    try:
        hwnd = n.wait_for(lambda: n.b.window_for(proc.pid, file.name))
        assert child(hwnd, 'SysTreeView32') is None, 'Single-file startup must not show a workspace'
    finally:
        proc.terminate()
        proc.wait()
    proc = subprocess.Popen([str(exe), str(root)], cwd=exe.parent)
    try:
        hwnd = n.wait_for(lambda: n.b.window_for(proc.pid, 'FeatherPad'))
        tree = n.wait_for(lambda: child(hwnd, 'SysTreeView32'))
        time.sleep(.2)  # Let the initial folder layout finish before measuring the drag.
        before = n.b.w.RECT()
        u.GetWindowRect(tree, c.byref(before))
        u.SendMessageW(hwnd, 0x201, 1, (100 << 16) | 240)
        u.SendMessageW(hwnd, 0x200, 1, (100 << 16) | 360)
        during = n.b.w.RECT()
        u.GetWindowRect(tree, c.byref(during))
        assert during.right - during.left == before.right - before.left, f"Dragging must not reflow the document: {before.right-before.left} -> {during.right-during.left}"
        u.SendMessageW(hwnd, 0x202, 0, (100 << 16) | 360)
        after = n.b.w.RECT()
        u.GetWindowRect(tree, c.byref(after))
        assert after.right - after.left > before.right - before.left + 100
        assert u.GetWindowLongW(tree, -16) & 0x8000, 'Tree must disable horizontal scrolling'
        u.SendMessageW(hwnd, 0x8002, 130, 0)
        n.wait_for(lambda: child(hwnd, 'SysTreeView32') is None)
        u.SendMessageW(hwnd, 0x8002, 130, 0)
        n.wait_for(lambda: child(hwnd, 'SysTreeView32') == tree)
        restored = n.b.w.RECT()
        u.GetWindowRect(tree, c.byref(restored))
        assert restored.right - restored.left == after.right - after.left
        root_item = u.SendMessageW(tree, 0x110A, 0, 0)
        folder = u.SendMessageW(tree, 0x110A, 4, root_item)
        assert u.SendMessageW(tree, 0x110A, 4, folder) == 0, 'Nested directory should be lazy'
        u.SendMessageW(tree, 0x1102, 2, folder)
        item = n.wait_for(lambda: u.SendMessageW(tree, 0x110A, 4, folder))
        u.SendMessageW(tree, 0x110B, 9, item)
        n.wait_for(lambda: n.b.window_for(proc.pid, file.name))
        n.wait_for(lambda: len(n.editors(hwnd)) == 1 and 'BODY_MARKER' in text(n.editor(hwnd)) and u.GetWindowLongW(n.editor(hwnd), -16) & 0x800)
        u.SendMessageW(hwnd, 0x8002, 105, 0)
        n.wait_for(lambda: len(n.editors(hwnd)) == 2)
        edit = n.editor(hwnd)
        n.wait_for(lambda: 'BODY_MARKER' in text(edit) and not u.GetWindowLongW(edit, -16) & 0x800)
        original = text(edit)
        for state in (3, 9, 3, 9):  # Maximize and restore, keeping status controls in bounds.
            u.ShowWindow(hwnd, state)
            time.sleep(.15)
            assert len(n.editors(hwnd)) == 2
            assert child(hwnd, 'Static') is not None
        u.SendMessageW(hwnd, 0x8002, 105, 0)
        n.wait_for(lambda: len(n.editors(hwnd)) == 1)
        u.SendMessageW(hwnd, 0x8002, 105, 0)
        n.wait_for(lambda: len(n.editors(hwnd)) == 2)
        preview = n.editors(hwnd)[1]
        n.wait_for(lambda: 'BODY_MARKER' in text(preview))
        for _ in range(2):
            u.SendMessageW(hwnd, 0x8002, 122, 0)
            n.wait_for(lambda: child(hwnd, 'FeatherPadTerminal'))
            time.sleep(.35)
            assert text(edit) == original and 'BODY_MARKER' in text(preview)
            u.SendMessageW(hwnd, 0x8002, 122, 0)
            n.wait_for(lambda: not child(hwnd, 'FeatherPadTerminal'))
            assert text(edit) == original and child(hwnd, 'SysTreeView32') == tree
        # The arrow is in the heading gutter (32 px text margin, first heading).
        clicked = None
        for y in range(22, 74, 4):
            for x in (34, 40, 46):
                point = (y << 16) | x
                u.SendMessageW(preview, 0x201, 1, point)
                u.SendMessageW(preview, 0x202, 0, point)
                time.sleep(.03)
                if 'BODY_MARKER' not in text(preview):
                    clicked = point
                    break
            if clicked is not None:
                break
        assert clicked is not None, 'Clicking the heading arrow must collapse its body'
        assert 'CHILD_MARKER' not in text(preview) and 'NEXT_MARKER' in text(preview)
        assert text(edit) == original
        u.SendMessageW(preview, 0x201, 1, clicked)
        u.SendMessageW(preview, 0x202, 0, clicked)
        n.wait_for(lambda: 'CHILD_MARKER' in text(preview))
        u.SendMessageW(edit, 0xB1, c.c_size_t(-1).value, -1)
        pending = c.create_unicode_buffer('UNSAVED')
        u.SendMessageW(edit, 0xC2, 1, c.addressof(pending))
        other = u.SendMessageW(tree, 0x110A, 1, folder)
        u.PostMessageW(tree, 0x110B, 9, other)
        dialog = n.wait_for(lambda: modal(proc.pid))
        u.SendMessageW(dialog, 0x111, 2, 0)  # Cancel the save/discard prompt.
        n.wait_for(lambda: modal(proc.pid) is None)
        assert n.b.window_for(proc.pid, file.name) and text(edit).endswith('UNSAVED')
        u.SendMessageW(hwnd, 0x8002, 128, 0)
        n.wait_for(lambda: child(hwnd, 'SysTreeView32') is None)
        assert text(edit).endswith('UNSAVED') and file.read_text(encoding='utf-8') == source
        print('PASS: single-file default; lazy workspace; heading collapse/expand; source unchanged; unsaved-switch cancellation; close workspace')
    finally:
        proc.terminate()
        proc.wait()


if __name__ == '__main__':
    run()
