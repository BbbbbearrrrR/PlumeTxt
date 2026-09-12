"""Windows process-level comparison; does not measure first rendered frame or FPS.
Run: python scripts/benchmark-editors.py --rounds 3
Results and isolated Sublime data stay in tmp/editor-benchmark/<timestamp>/.
"""
import argparse
import ctypes as c
from ctypes import wintypes as w
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
MIB = 1024 * 1024
kernel = c.WinDLL('kernel32', use_last_error=True)
user = c.WinDLL('user32', use_last_error=True)
psapi = c.WinDLL('psapi', use_last_error=True)


class Memory(c.Structure):
    _fields_ = [('cb', w.DWORD), ('faults', w.DWORD)] + [(name, c.c_size_t) for name in
        ('peak_ws', 'ws', 'peak_paged', 'paged', 'peak_nonpaged', 'nonpaged', 'pagefile', 'peak_pagefile', 'private')]


class Entry(c.Structure):
    _fields_ = [('size', w.DWORD), ('usage', w.DWORD), ('pid', w.DWORD), ('heap', c.c_size_t),
                ('module', w.DWORD), ('threads', w.DWORD), ('parent', w.DWORD), ('priority', w.LONG),
                ('flags', w.DWORD), ('exe', w.WCHAR * 260)]


kernel.OpenProcess.argtypes = [w.DWORD, w.BOOL, w.DWORD]
kernel.OpenProcess.restype = w.HANDLE
kernel.CloseHandle.argtypes = [w.HANDLE]
kernel.CreateToolhelp32Snapshot.argtypes = [w.DWORD, w.DWORD]
kernel.CreateToolhelp32Snapshot.restype = w.HANDLE
kernel.Process32FirstW.argtypes = [w.HANDLE, c.POINTER(Entry)]
kernel.Process32NextW.argtypes = [w.HANDLE, c.POINTER(Entry)]
kernel.GetProcessTimes.argtypes = [w.HANDLE] + [c.POINTER(w.FILETIME)] * 4
kernel.TerminateProcess.argtypes = [w.HANDLE, w.UINT]
psapi.GetProcessMemoryInfo.argtypes = [w.HANDLE, c.POINTER(Memory), w.DWORD]
callback = c.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
user.EnumWindows.argtypes = [callback, w.LPARAM]
user.GetWindowThreadProcessId.argtypes = [w.HWND, c.POINTER(w.DWORD)]
user.GetWindowTextW.argtypes = [w.HWND, w.LPWSTR, c.c_int]
user.IsWindowVisible.argtypes = [w.HWND]
user.SendMessageTimeoutW.argtypes = [w.HWND, w.UINT, c.c_size_t, c.c_ssize_t,
                                    w.UINT, w.UINT, c.POINTER(c.c_size_t)]
user.SendMessageTimeoutW.restype = c.c_size_t
user.PostMessageW.argtypes = [w.HWND, w.UINT, c.c_size_t, c.c_ssize_t]


def descendants(root):
    snapshot = kernel.CreateToolhelp32Snapshot(2, 0)
    if snapshot == c.c_void_p(-1).value:
        raise c.WinError(c.get_last_error())
    pairs = []
    try:
        entry = Entry(size=c.sizeof(Entry))
        found = kernel.Process32FirstW(snapshot, c.byref(entry))
        while found:
            pairs.append((entry.pid, entry.parent))
            found = kernel.Process32NextW(snapshot, c.byref(entry))
    finally:
        kernel.CloseHandle(snapshot)
    result = {root}
    while True:
        newer = result | {pid for pid, parent in pairs if parent in result}
        if newer == result:
            return result
        result = newer


def metrics(pids):
    private = working = cpu = 0
    for pid in pids:
        handle = kernel.OpenProcess(0x410, False, pid)
        if not handle:
            continue
        try:
            mem = Memory(cb=c.sizeof(Memory))
            if psapi.GetProcessMemoryInfo(handle, c.byref(mem), mem.cb):
                private += mem.private
                working += mem.ws
            created, exited, kt, ut = (w.FILETIME() for _ in range(4))
            if kernel.GetProcessTimes(handle, c.byref(created), c.byref(exited), c.byref(kt), c.byref(ut)):
                cpu += sum((ft.dwHighDateTime << 32) + ft.dwLowDateTime for ft in (kt, ut)) / 1e7
        finally:
            kernel.CloseHandle(handle)
    return private / MIB, working / MIB, cpu


def window_for(pid, filename):
    matches = []
    @callback
    def visit(hwnd, _):
        owner = w.DWORD()
        user.GetWindowThreadProcessId(hwnd, c.byref(owner))
        if owner.value == pid and user.IsWindowVisible(hwnd):
            title = c.create_unicode_buffer(1024)
            user.GetWindowTextW(hwnd, title, len(title))
            if filename.lower() in title.value.lower():
                matches.append(hwnd)
        return True
    user.EnumWindows(visit, 0)
    return matches[0] if matches else None


def trial(app, executable, file, timeout):
    # Standardize warm filesystem cache, without allocating a whole-file buffer.
    with file.open('rb') as stream:
        while stream.read(4 * MIB):
            pass
    startup = subprocess.STARTUPINFO()
    startup.dwFlags = subprocess.STARTF_USESHOWWINDOW
    startup.wShowWindow = 4  # Show without activation where the application honours it.
    began = time.perf_counter()
    proc = subprocess.Popen([str(executable), str(file)], cwd=executable.parent, startupinfo=startup)
    first = None
    hwnd = None
    pids = {proc.pid}
    peak_private = peak_working = 0
    quiet = 0
    last_sample = began
    last_cpu = 0
    status = 'timeout'
    settled = None
    try:
        while time.perf_counter() - began < timeout:
            if proc.poll() is not None:
                status = 'exited_before_ready'
                break
            hwnd = window_for(proc.pid, file.name)
            reply = c.c_size_t()
            responsive = hwnd and user.SendMessageTimeoutW(hwnd, 0, 0, 0, 2, 50, c.byref(reply))
            now = time.perf_counter()
            if responsive and first is None:
                first = (now - began) * 1000
            if now - last_sample >= 0.5:
                pids = descendants(proc.pid)
                private, working, cpu = metrics(pids)
                peak_private = max(peak_private, private)
                peak_working = max(peak_working, working)
                quiet = quiet + 1 if responsive and cpu - last_cpu < 0.025 else 0
                last_cpu, last_sample = cpu, now
                if quiet >= 4:  # Two seconds of low CPU, includes a common settling delay.
                    settled = (now - began) * 1000
                    status = 'settled'
                    break
            time.sleep(0.05)
        pids = descendants(proc.pid)
        private, working, cpu = metrics(pids)
        idle_start = time.perf_counter()
        if status == 'settled':
            time.sleep(1)
        idle_cpu = metrics(pids)[2] - cpu
        return dict(app=app, file=file.name, size_mib=file.stat().st_size / MIB,
                    status=status, first_responsive_ms=first, settled_ms=settled,
                    private_mib=private, working_set_sum_mib=working,
                    sampled_peak_private_mib=peak_private, sampled_peak_working_set_sum_mib=peak_working,
                    cpu_seconds_to_sample=cpu, idle_cpu_core_percent=max(0,idle_cpu) / max(.001,time.perf_counter()-idle_start)*100,
                    processes=len(pids))
    finally:
        # Only close the benchmark process and descendants created by this run.
        if hwnd:
            user.PostMessageW(hwnd, 0x10, 0, 0)
        try:
            proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            proc.terminate()
            proc.wait(timeout=3)
        for pid in pids - {proc.pid}:
            handle = kernel.OpenProcess(1, False, pid)
            if handle:
                kernel.TerminateProcess(handle, 0)
                kernel.CloseHandle(handle)


def fixture(path, size):
    line = (b'# Heading\n\n**Bold** and [link](https://example.com).\n\n```rust\nlet value = 42;\n```\n\n'
            if path.suffix == '.md' else 'name,city,count,note\nAlice,Paris,42,"quoted, field"\n张三,北京,17,"中文内容"\n'.encode())
    block = line * (MIB // len(line))
    with path.open('wb') as stream:
        remaining = size
        while remaining:
            piece = block[:remaining]
            if len(piece) < len(block):
                valid = piece.decode('utf-8', errors='ignore').encode('utf-8')
                piece = valid + b' ' * (len(piece) - len(valid))
            stream.write(piece)
            remaining -= len(piece)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--rounds', type=int, default=3)
    parser.add_argument('--timeout', type=int, default=60)
    parser.add_argument('--sublime', type=Path, default=Path('D:/softwares/Sublime Text'))
    args = parser.parse_args()
    assert 1 <= args.rounds <= 10 and 5 <= args.timeout <= 180
    output = ROOT / 'tmp' / 'editor-benchmark' / time.strftime('%Y%m%d-%H%M%S')
    output.mkdir(parents=True)
    sublime = output / 'Sublime'
    shutil.copytree(args.sublime, sublime, ignore=shutil.ignore_patterns('Data', 'unins*'))
    preferences = sublime / 'Data' / 'Packages' / 'User'
    preferences.mkdir(parents=True)
    (preferences / 'Preferences.sublime-settings').write_text(json.dumps({
        'hot_exit': False, 'remember_open_files': False, 'update_check': False}), encoding='utf-8')
    apps = [('FeatherPad', ROOT / 'FeatherPad.exe'), ('Sublime Text 4200', sublime / 'sublime_text.exe')]
    cases = [('markdown-1MiB.md',1),('markdown-8MiB.md',8),('markdown-64MiB.md',64),
             ('markdown-1GiB.md',1024),('table-1MiB.csv',1),('table-64MiB.csv',64)]
    for name, size in cases:
        fixture(output / name, size * MIB)
    warmup = output / 'warmup.md'
    fixture(warmup, 64 * 1024)
    for app, exe in apps:
        print('WARMUP', json.dumps(trial(app,exe,warmup,args.timeout)), flush=True)
    results = []
    for round_number in range(args.rounds):
        for name, _ in cases:
            for app, exe in apps[::1 if round_number % 2 == 0 else -1]:
                result = trial(app, exe, output / name, args.timeout)
                result['round'] = round_number + 1
                results.append(result)
                with (output / 'results.jsonl').open('a',encoding='utf-8') as stream:
                    stream.write(json.dumps(result) + '\n')
                print(json.dumps(result),flush=True)
    metadata = {'rounds':args.rounds,'featherpad_sha256':hashlib.sha256((ROOT/'FeatherPad.exe').read_bytes()).hexdigest(),
                'sublime_sha256':hashlib.sha256((sublime/'sublime_text.exe').read_bytes()).hexdigest(),
                'method':'Warm cache, fresh processes, isolated Sublime defaults. Responsive = filename title plus WM_NULL reply. Settled = 4 half-second samples below 25ms CPU each. Not a first-paint, full-load, edit-latency or FPS measurement. Working-set sums double-count shared pages. Peaks sampled every 0.5s.'}
    (output / 'metadata.json').write_text(json.dumps(metadata,indent=2),encoding='utf-8')
    print('RESULTS',output,flush=True)


if __name__ == '__main__':
    main()
