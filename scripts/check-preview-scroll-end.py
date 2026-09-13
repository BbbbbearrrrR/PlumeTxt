"""Check paged preview scrollbar endpoints using a generated file and an isolated app."""
import ctypes as c
import importlib.util
from pathlib import Path
import subprocess, time, sys
spec=importlib.util.spec_from_file_location('t', Path(__file__).with_name('check-workspace-preview.py'))
t=importlib.util.module_from_spec(spec); spec.loader.exec_module(t)
u=t.u
u.GetClientRect.argtypes=[t.n.b.w.HWND,c.POINTER(t.n.b.w.RECT)]
u.GetScrollInfo.argtypes=[t.n.b.w.HWND,c.c_int,c.c_void_p]
class Scroll(c.Structure):
    _fields_=[('size',c.c_uint),('mask',c.c_uint),('min',c.c_int),('max',c.c_int),('page',c.c_uint),('pos',c.c_int),('track',c.c_int)]
def state(h):
    s=Scroll(c.sizeof(Scroll),23); u.GetScrollInfo(h,1,c.byref(s)); return (s.pos,s.max,s.page)
def controls(h):
    result=[]
    @t.n.b.callback
    def visit(ch,_):
        name=c.create_unicode_buffer(128); u.GetClassNameW(ch,name,128)
        if name.value in ('RICHEDIT50W','PlumeTxtScroll'): result.append((ch,name.value,bool(u.IsWindowVisible(ch))))
        return True
    u.EnumChildWindows(h,visit,0); return result
exe=Path(sys.argv[1] if len(sys.argv)>1 else t.n.b.ROOT / 'target/release/plumetxt.exe').resolve()
root = t.n.b.ROOT / 'tmp' / 'preview-scroll-end'
root.mkdir(parents=True, exist_ok=True)
file = root / 'scroll-end.md'
block = ('# Section\n\n' + 'Normal paragraph with wrapped text. ' * 25 + '\n\n'
         '| Left | Right |\n|---|---|\n| one | two |\n| three | four |\n\n'
         '```python\nfor i in range(5):\n    print(i)\n```\n\n').encode()
with file.open('wb') as stream:
    for _ in range(33 * 1024 * 1024 // len(block) + 1):
        stream.write(block)
    stream.write(b'# File end\n\nEND_OF_TEST_FILE\n')

startup=subprocess.STARTUPINFO(); startup.dwFlags=subprocess.STARTF_USESHOWWINDOW; startup.wShowWindow=4
p=subprocess.Popen([str(exe),str(file)],cwd=exe.parent,startupinfo=startup)
try:
    h=t.n.wait_for(lambda:t.n.b.window_for(p.pid,file.name))
    preview=t.n.wait_for(lambda:t.n.editor(h))
    t.n.wait_for(lambda:len(t.text(preview))>100)
    time.sleep(1)
    preview=t.n.wait_for(lambda:t.n.editor(h))
    rect=t.n.b.w.RECT(); u.GetWindowRect(preview,c.byref(rect))
    bars=[]
    for ch,kind,visible in controls(h):
        if kind=='PlumeTxtScroll' and visible:
            r=t.n.b.w.RECT(); u.GetWindowRect(ch,c.byref(r))
            if r.left>=rect.left and r.left<rect.right and r.bottom-r.top>r.right-r.left: bars.append(ch)
    bar=bars[0]; r=t.n.b.w.RECT(); u.GetClientRect(bar,c.byref(r))
    # Grab the thumb at the top and drag it to the end of its track.
    u.SendMessageW(bar, 0x201, 1, (24 << 16) | 10)
    point = ((r.bottom - 2) << 16) | 10
    u.SendMessageW(bar, 0x200, 1, point)
    u.SendMessageW(bar, 0x202, 0, point)
    t.n.wait_for(lambda: 'END_OF_TEST_FILE' in t.text(preview))
    time.sleep(1)
    def check_visible_end():
        first = u.SendMessageW(preview, 0xCE, 0, 0)
        u.SendMessageW(preview, 0x115, 7, 0)  # Native SB_BOTTOM aligns the actual text.
        assert u.SendMessageW(preview, 0xCE, 0, 0) == first, 'Last screen is visually overscrolled'
    check_visible_end()
    before = state(preview)
    for _ in range(8):
        u.SendMessageW(preview, 0x20A, (-120 & 0xffff) << 16, 0)
        time.sleep(.05)
    time.sleep(.3)
    after = state(preview)
    assert before == after, f'Wheel moved beyond thumb endpoint: {before} -> {after}'
    assert abs(before[0] - (before[1] - before[2] + 1)) <= 1, f'Preview is not at bottom: {before}'
    for _ in range(45):
        u.SendMessageW(preview, 0x20A, 120 << 16, 0)
        time.sleep(.025)
    stable = 0
    previous = None
    for _ in range(600):
        u.SendMessageW(preview, 0x20A, (-120 & 0xffff) << 16, 0)
        time.sleep(.025)
        current = state(preview)
        at_end = ('END_OF_TEST_FILE' in t.text(preview)
                  and abs(current[0] - (current[1] - current[2] + 1)) <= 1)
        stable = stable + 1 if at_end and current == previous else 0
        if stable >= 20:
            break
        previous = current
    assert stable >= 20, 'Wheel did not reach the document end'
    check_visible_end()
    point = (2 << 16) | 10
    u.SendMessageW(bar, 0x201, 1, point)
    u.SendMessageW(bar, 0x202, 0, point)
    t.n.wait_for(lambda: 'END_OF_TEST_FILE' not in t.text(preview))
    time.sleep(.5)
    assert state(preview)[0] == 0, 'Thumb must also reach the top'
    print('Thumb and wheel reach the same visual bottom; thumb also reaches the top.')

finally:
    p.terminate(); p.wait()
