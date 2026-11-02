"""Drive the real Forge window with real mouse and keyboard input and save a screenshot (Windows, needs Pillow).

    python tools/uishot.py EXE FOLDER REPLAY OUT [ACTIONS]

EXE       the Forge build to run (target/debug/forge.exe)
FOLDER    the folder or file to open
REPLAY    keys fed to the editor through FORGE_REPLAY ("ctrl+n text:hello enter"), or "" for none
OUT       where to save the screenshot
ACTIONS   space separated, run after the window settled:
            x,y:c    click       x,y:d    double click   x,y:t    triple click   x,y:r    right click   x,y:m   move
            x1,y1>x2,y2:g        drag with the left button
            ctrl@x,y:c           the same with a key held (ctrl, shift or alt)
            wait:S               wait S seconds
            key:ctrl+a           a real key chord (letters and digits only, or esc)
            type:text            real typing (letters, digits, '.', '/')
            enter

Environment: TSHOT_WAIT seconds before the actions (default 9), TSHOT_AFTER=1 to shoot the window where it ends up,
TSHOT_W / TSHOT_H to resize first, TSHOT_KEEP=1 to leave the app running, TSHOT_CFG for the config folder.

Safety: nothing is sent, and no screenshot is taken, unless the Forge window has the foreground. Coordinates are
relative to the window's top-left corner. Clicks go to whatever is at that screen position, so it refuses to go on if
Forge loses the foreground."""
import ctypes, os, subprocess, sys, time
from ctypes import wintypes
from PIL import ImageGrab

u = ctypes.windll.user32
exe, folder, replay, out = sys.argv[1:5]
actions = sys.argv[5].split() if len(sys.argv) > 5 else []
cfg = os.environ.get("TSHOT_CFG", os.path.join(os.environ["TEMP"], "forge-cfg-t"))
env = dict(os.environ, FORGE_REPLAY=replay, FORGE_REPLAY_DELAY_MS="2500", FORGE_CONFIG_DIR=cfg)
p = subprocess.Popen([exe, folder], env=env)

found = []
for _ in range(100):
    time.sleep(0.3)
    found = []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def cb(h, l):
        pid = wintypes.DWORD()
        u.GetWindowThreadProcessId(h, ctypes.byref(pid))
        if pid.value == p.pid and u.IsWindowVisible(h):
            found.append(h)
        return True

    u.EnumWindows(cb, 0)
    if found:
        break
if not found:
    p.kill(); print('NO WINDOW for', p.pid, 'exit', p.poll()); sys.exit(1)
hwnd = found[0]
time.sleep(float(os.environ.get("TSHOT_WAIT", "9")))


def front():
    # Windows only lets a process take the foreground after a recent key press, so tap Alt first.
    u.keybd_event(0x12, 0, 0, 0); u.keybd_event(0x12, 0, 2, 0)
    u.ShowWindow(hwnd, 9 if not u.IsZoomed(hwnd) else 3)
    u.BringWindowToTop(hwnd); u.SetForegroundWindow(hwnd)
    time.sleep(0.5)
    return u.GetForegroundWindow() == hwnd


ok = False
for _ in range(8):
    if front():
        ok = True; break
    time.sleep(1.5)
if not ok:
    p.kill(); print('Forge could not be brought to the front: not sending any input'); sys.exit(2)
if os.environ.get('TSHOT_W'):
    u.MoveWindow(hwnd, 100, 100, int(os.environ['TSHOT_W']), int(os.environ.get('TSHOT_H', '700')), True); time.sleep(1.5)
time.sleep(0.4)
rect = wintypes.RECT()
u.GetWindowRect(hwnd, ctypes.byref(rect))
ox, oy = rect.left, rect.top


def move(x, y):
    u.SetCursorPos(ox + int(x) + 1, oy + int(y))
    time.sleep(0.1)
    u.SetCursorPos(ox + int(x), oy + int(y))
    time.sleep(0.4)


def btn(down, up):
    u.mouse_event(down, 0, 0, 0, 0)
    time.sleep(0.2)
    u.mouse_event(up, 0, 0, 0, 0)
    time.sleep(0.12)


L = (0x2, 0x4); R = (0x8, 0x10)
KEYS = {"ctrl": 0x11, "shift": 0x10, "alt": 0x12}
for a in actions:
    if not a.startswith('wait:') and u.GetForegroundWindow() != hwnd:
        buf = ctypes.create_unicode_buffer(200); u.GetWindowTextW(u.GetForegroundWindow(), buf, 200)
        p.kill(); print('Forge lost the foreground to', repr(buf.value), ': stopping before', a); sys.exit(3)
    if a.startswith("wait:"):
        time.sleep(float(a[5:])); continue
    if a.startswith("type:"):
        for ch in a[5:]:
            vk = ord(ch.upper()) if ch.isalnum() else {".": 0xBE, "/": 0xBF}[ch]
            u.keybd_event(vk, 0, 0, 0); u.keybd_event(vk, 0, 2, 0); time.sleep(0.05)
        time.sleep(0.2); continue
    if a == "enter":
        u.keybd_event(0x0D, 0, 0, 0); u.keybd_event(0x0D, 0, 2, 0); time.sleep(1.0); continue
    if a.startswith("key:"):
        parts = a[4:].split("+")
        for k in parts[:-1]: u.keybd_event(KEYS[k], 0, 0, 0)
        vk = 0x1B if parts[-1] == 'esc' else ord(parts[-1].upper()); u.keybd_event(vk, 0, 0, 0); u.keybd_event(vk, 0, 2, 0)
        for k in parts[:-1]: u.keybd_event(KEYS[k], 0, 2, 0)
        time.sleep(0.3); continue
    pos, op = a.rsplit(":", 1)
    if op == "g":
        s, e = pos.split(">")
        sx, sy = map(float, s.split(",")); ex, ey = map(float, e.split(","))
        move(sx, sy); u.mouse_event(0x2, 0, 0, 0, 0); time.sleep(0.2)
        for i in range(1, 11):
            u.SetCursorPos(ox + int(sx + (ex - sx) * i / 10), oy + int(sy + (ey - sy) * i / 10)); u.mouse_event(0x1, 1, 0, 0, 0); time.sleep(0.1); u.mouse_event(0x1, -1, 0, 0, 0); time.sleep(0.1)
        time.sleep(0.2); u.mouse_event(0x4, 0, 0, 0, 0); time.sleep(0.5); continue
    mod = None
    if '@' in pos:
        mod, pos = pos.split('@')
    x, y = map(float, pos.split(","))
    move(x, y)
    if mod:
        u.keybd_event(KEYS[mod], 0, 0, 0); time.sleep(0.1)
    if op == "c": btn(*L)
    elif op == "d": btn(*L); btn(*L)
    elif op == "t": btn(*L); btn(*L); btn(*L)
    elif op == "r": btn(*R)
    if mod:
        u.keybd_event(KEYS[mod], 0, 2, 0)
    time.sleep(0.4)
time.sleep(0.5)
r2 = wintypes.RECT(); u.GetWindowRect(hwnd, ctypes.byref(r2))
print('rect', (rect.left, rect.top, rect.right, rect.bottom), '->', (r2.left, r2.top, r2.right, r2.bottom), 'zoomed', bool(u.IsZoomed(hwnd)), 'alive', p.poll() is None)
if u.GetForegroundWindow() != hwnd and not front():
    p.kill(); print('Forge is not in front: no screenshot taken'); sys.exit(4)
box = (r2.left, r2.top, r2.right, r2.bottom) if os.environ.get("TSHOT_AFTER") else (rect.left, rect.top, rect.right, rect.bottom)
ImageGrab.grab(bbox=box).save(out)
if not os.environ.get("TSHOT_KEEP"):
    p.kill()
