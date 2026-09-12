"""Check standalone and workspace PDF search using generated, cropped/rotated pages."""
import ctypes as c
import importlib.util
from pathlib import Path
import subprocess
import time
import zlib

spec = importlib.util.spec_from_file_location('search_test', Path(__file__).with_name('check-workspace-search.py'))
s = importlib.util.module_from_spec(spec); spec.loader.exec_module(s)
t, u, w = s.t, s.u, s.w

class ScrollInfo(c.Structure):
    _fields_ = [('size', w.UINT), ('mask', w.UINT), ('minimum', c.c_int), ('maximum', c.c_int), ('page', w.UINT), ('position', c.c_int), ('track', c.c_int)]
u.GetScrollInfo.argtypes = [w.HWND, c.c_int, c.POINTER(ScrollInfo)]
def scroll_position(reader):
    info = ScrollInfo(c.sizeof(ScrollInfo), 0x17)
    u.GetScrollInfo(reader, 1, c.byref(info))
    return info.position

def make_pdf(path):
    def stream(text):
        data = zlib.compress(f'BT /F1 20 Tf 100 300 Td ({text}) Tj ET'.encode())
        return f'<< /Length {len(data)} /Filter /FlateDecode >>\nstream\n'.encode() + data + b'\nendstream'
    objects = [b'<< /Type /Catalog /Pages 2 0 R >>',
        b'<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 600 800] >>',
        b'<< /Type /Page /Parent 2 0 R /CropBox [50 100 550 700] /Resources << /Font << /F1 5 0 R >> >> /Contents 6 0 R >>',
        b'<< /Type /Page /Parent 2 0 R /CropBox [50 100 550 700] /Rotate 90 /Resources << /Font << /F1 5 0 R >> >> /Contents 7 0 R >>',
        b'<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>', stream('First Needle text'), stream('Second Needle text')]
    data = b'%PDF-1.7\n'; offsets = [0]
    for i, obj in enumerate(objects, 1):
        offsets.append(len(data)); data += f'{i} 0 obj\n'.encode() + obj + b'\nendobj\n'
    start = len(data)
    data += f'xref\n0 {len(offsets)}\n0000000000 65535 f \n'.encode() + b''.join(f'{v:010} 00000 n \n'.encode() for v in offsets[1:])
    data += f'trailer\n<< /Size {len(offsets)} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n'.encode()
    path.write_bytes(data)

def page_status(hwnd, page):
    found = []
    @t.n.b.callback
    def visit(child, _):
        name = c.create_unicode_buffer(80); u.GetClassNameW(child, name, 80)
        if name.value == 'Static' and t.text(child).startswith(f'Page {page} / 2'): found.append(child)
        return True
    u.EnumChildWindows(hwnd, visit, 0)
    return bool(found)

def run():
    root = t.n.b.ROOT / 'tmp' / 'pdf-search-smoke'; root.mkdir(parents=True, exist_ok=True)
    pdf = root / '搜索.pdf'; make_pdf(pdf)
    (root / 'note.txt').write_text('Needle in workspace', encoding='utf-8')
    for standalone in (True, False):
        proc = subprocess.Popen([str(t.n.b.ROOT / 'target/release/featherpad.exe'), str(pdf if standalone else root)])
        try:
            hwnd = t.n.wait_for(lambda: t.n.b.window_for(proc.pid, 'FeatherPad'))
            if standalone: t.n.wait_for(lambda: page_status(hwnd, 1))
            else: t.n.wait_for(lambda: t.child(hwnd, 'SysTreeView32'))
            s.find_command(hwnd, workspace=not standalone)
            panel = t.n.wait_for(lambda: t.child(hwnd, 'FeatherPadSearch'))
            edit, tree, status = [u.GetDlgItem(panel, n) for n in (1, 2, 3)]
            s.set_query(edit, 'needle')
            expected = 2 if standalone else 3
            t.n.wait_for(lambda: t.text(status).startswith(f'{expected} matches'), timeout=20)
            # Find the PDF's two-result group without reading remote tree strings.
            group = u.SendMessageW(tree, 0x110A, 0, 0)
            while group:
                first = u.SendMessageW(tree, 0x110A, 4, group)
                second = u.SendMessageW(tree, 0x110A, 1, first)
                if second: break
                group = u.SendMessageW(tree, 0x110A, 1, group)
            assert group and second
            u.SendMessageW(tree, 0x110B, 9, second)
            u.PostMessageW(tree, 0x100, 0x0D, 0)
            reader = t.n.wait_for(lambda: t.child(hwnd, 'FeatherPadReader'))
            t.n.wait_for(lambda: scroll_position(reader) > 0, timeout=20)
            assert t.child(hwnd, 'FeatherPadReader') and not t.n.editors(hwnd)
            time.sleep(.5)
            if standalone and '--capture' in __import__('sys').argv:
                from PIL import ImageGrab
                ImageGrab.grab(window=hwnd).save(root / 'hit.png')
            u.PostMessageW(edit, 0x100, 0x1B, 0)
            t.n.wait_for(lambda: not t.child(hwnd, 'FeatherPadSearch'))
            assert t.child(hwnd, 'FeatherPadReader')
        finally:
            proc.terminate(); proc.wait()
    print('PASS: standalone PDF Find, workspace PDF search before opening a file, page jump and rotated/cropped text')

if __name__ == '__main__': run()
