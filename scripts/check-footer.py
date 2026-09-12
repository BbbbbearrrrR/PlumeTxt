"""Check context footer sizing and its Commands button using generated documents only."""
import ctypes as c
import importlib.util
from pathlib import Path
import subprocess
import time

spec = importlib.util.spec_from_file_location('smoke', Path(__file__).with_name('check-workspace-preview.py'))
t = importlib.util.module_from_spec(spec)
spec.loader.exec_module(t)
u, w = t.u, t.n.b.w
g = c.WinDLL('gdi32')
u.GetDC.argtypes = [w.HWND]; u.GetDC.restype = w.HDC
u.ReleaseDC.argtypes = [w.HWND, w.HDC]
g.SelectObject.argtypes = [w.HDC, w.HANDLE]; g.SelectObject.restype = w.HANDLE
g.GetTextExtentPoint32W.argtypes = [w.HDC, w.LPCWSTR, c.c_int, c.POINTER(w.SIZE)]
u.GetDlgItem.argtypes = [w.HWND, c.c_int]; u.GetDlgItem.restype = w.HWND

def footer(hwnd, prefix):
    found = []
    @t.n.b.callback
    def visit(child, _):
        name = c.create_unicode_buffer(128)
        u.GetClassNameW(child, name, 128)
        if name.value == 'Static' and t.text(child).startswith(prefix): found.append(child)
        return True
    u.EnumChildWindows(hwnd, visit, 0)
    return found[0] if found else None

def fits(control):
    dc = u.GetDC(control)
    old = g.SelectObject(dc, u.SendMessageW(control, 0x31, 0, 0))
    size, rect = w.SIZE(), w.RECT()
    value = t.text(control)
    g.GetTextExtentPoint32W(dc, value, len(value), c.byref(size))
    u.GetClientRect(control, c.byref(rect))
    g.SelectObject(dc, old); u.ReleaseDC(control, dc)
    assert size.cx <= rect.right, (value, size.cx, rect.right)

def run():
    root = t.n.b.ROOT / 'tmp' / 'footer-smoke'
    root.mkdir(parents=True, exist_ok=True)
    note = root / 'note.md'; note.write_text('# Test\n\nContent.\n', encoding='utf-8')
    pdf = root / 'pages.pdf'
    objects = [b'<< /Type /Catalog /Pages 2 0 R >>', b'<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 595 842] >>', b'<< /Type /Page /Parent 2 0 R /Resources <<>> >>', b'<< /Type /Page /Parent 2 0 R /Resources <<>> >>']
    data = b'%PDF-1.4\n'; offsets = [0]
    for i, obj in enumerate(objects, 1):
        offsets.append(len(data)); data += f'{i} 0 obj\n'.encode() + obj + b'\nendobj\n'
    start = len(data)
    data += b'xref\n0 5\n0000000000 65535 f \n' + b''.join(f'{v:010} 00000 n \n'.encode() for v in offsets[1:])
    data += f'trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n'.encode()
    pdf.write_bytes(data)
    exe = t.n.b.ROOT / 'target' / 'release' / 'featherpad.exe'
    for path, prefix in [(note, 'Reading'), (pdf, 'Page 1 / 2')]:
        proc = subprocess.Popen([str(exe), str(path)])
        try:
            hwnd = t.n.wait_for(lambda: t.n.b.window_for(proc.pid, path.name))
            status = t.n.wait_for(lambda: footer(hwnd, prefix))
            for width in (820, 1200):
                u.MoveWindow(hwnd, 80, 80, width, 650, True)
                time.sleep(.25)
                fits(status)
                commands = u.GetDlgItem(hwnd, 118)
                assert commands and t.text(commands) == 'Commands  Ctrl+Shift+P'
                fits(commands)
            u.SendMessageW(commands, 0xF5, 0, 0)  # BM_CLICK
            panel = t.n.wait_for(lambda: t.child(hwnd, 'FeatherPadCommands'))
            commands_list = t.child(panel, 'ListBox')
            count = u.SendMessageW(commands_list, 0x18B, 0, 0)
            labels = []
            for i in range(count):
                size = u.SendMessageW(commands_list, 0x18A, i, 0)
                value = c.create_unicode_buffer(size + 1)
                u.SendMessageW(commands_list, 0x189, i, c.addressof(value))
                labels.append(value.value)
            assert count == (8 if path == pdf else 9), labels
            assert ('Print' in labels) == (path == pdf)
            assert ('Export PDF' in labels) == (path == note)
            assert 'Quit' not in labels and 'Bold' not in labels

            u.SendMessageW(hwnd, 0x8002, 118, 0)
            t.n.wait_for(lambda: not t.child(hwnd, 'FeatherPadCommands'))
            if path == pdf:
                reader = t.child(hwnd, 'FeatherPadReader')
                u.SendMessageW(reader, 0x8000 + 92, 1, 100000)
                t.n.wait_for(lambda: footer(hwnd, 'Page 2 / 2'))
            else:
                u.SendMessageW(hwnd, 0x8002, 112, 0)
                t.n.wait_for(lambda: footer(hwnd, 'Editing'))
        finally:
            proc.terminate(); proc.wait()
    print('PASS: context footer, live page number, narrow/wide text fit and Commands button')

if __name__ == '__main__': run()
