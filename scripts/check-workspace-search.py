"""Exercise workspace search in an isolated app using generated documents."""
import ctypes as c
import importlib.util
from pathlib import Path
import subprocess
import time

spec = importlib.util.spec_from_file_location('smoke', Path(__file__).with_name('check-workspace-preview.py'))
t = importlib.util.module_from_spec(spec)
spec.loader.exec_module(t)
u, w = t.u, t.n.b.w
u.GetDlgItem.argtypes = [w.HWND, c.c_int]; u.GetDlgItem.restype = w.HWND

def find_command(hwnd, workspace=False):
    # Exercise the native handler without requiring Windows foreground activation.
    # The Rust shortcut test verifies Ctrl+F / Ctrl+Shift+F routing separately.
    u.SendMessageW(hwnd, 0x8002, 135 if workspace else 136, 0)

def set_query(edit, value):
    value = c.create_unicode_buffer(value)
    u.SendMessageW(edit, 0xC, 0, c.addressof(value))

def selected(edit):
    start, end = w.DWORD(), w.DWORD()
    u.SendMessageW(edit, 0xB0, c.addressof(start), c.addressof(end))
    source = t.text(edit).replace('\r\n', '\r').replace('\n', '\r').encode('utf-16-le')
    return source[start.value * 2:end.value * 2].decode('utf-16-le')

def assert_visible(edit):
    start = w.DWORD()
    u.SendMessageW(edit, 0xB0, c.addressof(start), 0)
    line = u.SendMessageW(edit, 0x400 + 54, 0, start.value)
    first = u.SendMessageW(edit, 0xCE, 0, 0)
    rect = w.RECT(); u.GetClientRect(edit, c.byref(rect))
    assert first <= line < first + max(1, rect.bottom // 20), (first, line, rect.bottom)

def run():
    root = t.n.b.ROOT / 'tmp' / 'search-smoke'
    (root / 'nested').mkdir(parents=True, exist_ok=True)
    (root / '.git').mkdir(exist_ok=True)
    (root / '.git' / 'ignore.txt').write_text('NEEDLE', encoding='utf-8')
    (root / 'one.md').write_text('# Heading\n\n中文 😀 NEEDLE note\n', encoding='utf-8')
    (root / 'nested' / 'two.toml').write_text('name = "NEEDLE"\n', encoding='utf-16')
    large = root / 'large.md'
    with large.open('wb') as file:
        file.write(b'padding\n' * (1100 * 1024))
        file.write('中文 LARGE_NEEDLE here\n'.encode())
    proc = subprocess.Popen([str(t.n.b.ROOT / 'target/release/featherpad.exe'), str(root)])
    try:
        hwnd = t.n.wait_for(lambda: t.n.b.window_for(proc.pid, 'FeatherPad'))
        t.n.wait_for(lambda: t.child(hwnd, 'SysTreeView32'))
        find_command(hwnd)  # Find must work before any file has been selected.
        panel = t.n.wait_for(lambda: t.child(hwnd, 'FeatherPadSearch'))
        edit, tree, status = [u.GetDlgItem(panel, n) for n in (1, 2, 3)]
        assert u.GetWindowLongW(tree, -16) & 0x8000
        set_query(edit, 'missing')
        set_query(edit, 'needle')
        t.n.wait_for(lambda: t.text(status).startswith('3 matches'), timeout=20)
        assert '3 files' in t.text(status), t.text(status)
        if '--capture' in __import__('sys').argv:
            from PIL import ImageGrab
            u.SetForegroundWindow(hwnd)
            time.sleep(.15)
            rect = w.RECT(); u.GetWindowRect(hwnd, c.byref(rect))
            ImageGrab.grab(bbox=(rect.left, rect.top, rect.right, rect.bottom)).save(root / 'search.png')
        for group_index in range(3):
            group = u.SendMessageW(tree, 0x110A, 0, 0)
            for _ in range(group_index): group = u.SendMessageW(tree, 0x110A, 1, group)
            hit = u.SendMessageW(tree, 0x110A, 4, group)
            assert hit
            u.SendMessageW(tree, 0x110B, 9, hit)
            u.PostMessageW(tree, 0x100, 0x0D, 0)
            def loaded():
                editor = t.n.editor(hwnd)
                return editor and not u.GetWindowLongW(editor, -16) & 0x800 and selected(editor) == 'NEEDLE'
            t.n.wait_for(loaded, timeout=20)
            time.sleep(.3)
            assert_visible(t.n.editor(hwnd))
        # Re-query the large file and jump near EOF; only its editable region should load.
        set_query(edit, 'large_needle')
        u.PostMessageW(edit, 0x100, 0x0D, 0)  # Enter searches and opens the first match.
        t.n.wait_for(lambda: selected(t.n.editor(hwnd)) == 'LARGE_NEEDLE', timeout=20)
        assert len(t.text(t.n.editor(hwnd))) < 65536
        time.sleep(.3)
        assert_visible(t.n.editor(hwnd))
        u.SendMessageW(hwnd, 0x8002, 135, 0)
        u.SendMessageW(hwnd, 0x8002, 112, 0)
        time.sleep(.3)
        assert_visible(t.n.editor(hwnd))
        assert selected(t.n.editor(hwnd)) == 'LARGE_NEEDLE'
        if '--capture' in __import__('sys').argv:
            rect = w.RECT(); u.GetWindowRect(hwnd, c.byref(rect))
            ImageGrab.grab(bbox=(rect.left, rect.top, rect.right, rect.bottom)).save(root / 'hit.png')
        # Unsaved region edits must survive cancelling a result navigation.
        editor = t.n.editor(hwnd)
        insert = c.create_unicode_buffer('UNSAVED')
        u.SendMessageW(editor, 0xC2, 1, c.addressof(insert))
        set_query(edit, 'note')
        t.n.wait_for(lambda: t.text(status).startswith('1 matches'), timeout=20)
        group = u.SendMessageW(tree, 0x110A, 0, 0)
        hit = u.SendMessageW(tree, 0x110A, 4, group)
        u.SendMessageW(tree, 0x110B, 9, hit)
        u.PostMessageW(tree, 0x100, 0x0D, 0)
        dialog = t.n.wait_for(lambda: t.modal(proc.pid))
        u.SendMessageW(dialog, 0x111, 2, 0)
        t.n.wait_for(lambda: not t.modal(proc.pid))
        assert 'UNSAVED' in t.text(editor)
        u.PostMessageW(edit, 0x100, 0x1B, 0)
        t.n.wait_for(lambda: not t.child(hwnd, 'FeatherPadSearch'))
        assert t.child(hwnd, 'SysTreeView32')
        u.SendMessageW(hwnd, 0x8002, 135, 0)
        t.n.wait_for(lambda: t.child(hwnd, 'FeatherPadSearch'))
        assert t.text(edit) == 'note'
        set_query(edit, 'nothing-here')
        t.n.wait_for(lambda: t.text(status).startswith('0 matches'), timeout=20)
        u.SendMessageW(hwnd, 0x8002, 128, 0)
        assert not t.child(hwnd, 'FeatherPadSearch')
        assert b'UNSAVED' not in large.read_bytes()
    finally:
        proc.terminate(); proc.wait()
    print('PASS: grouped search, UTF-16, Markdown/large-file jumps, cancellation, unsaved edits and sidebar return')

if __name__ == '__main__': run()
