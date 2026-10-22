#!/usr/bin/env python3
"""用 UI Automation 在真实 Edge 中重新加载 EMA2 解压扩展。

流程：

1. 找到 msedge.exe 的可见窗口（没有就先启动 Edge）；
2. 恢复并最大化窗口（等价 Win+Up），避免按钮被其他窗口遮挡；
3. 用 UIA 把 ``edge://extensions/`` 填入地址栏并回车（不依赖 Ctrl+L 焦点）；
4. 在 EMA2 的扩展卡片里按 ``Type=Button`` + ``Name=重新加载/Reload``
   精确查找按钮，优先 UIA Invoke，按钮不支持时退回鼠标点击。

依赖 pywinauto：``pip install pywinauto``。只操作真实、前台可见的 Edge。

用法：

    python scripts/reloadextension.py
    python scripts/reloadextension.py --timeout 30
    python scripts/reloadextension.py --name "EMA2 - Edge Agent"

退出码：0 成功 / 1 参数或依赖错误 / 2 找不到 Edge 窗口 / 3 找不到扩展或按钮。
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
import time
from typing import Any

DEFAULT_NAME = "EMA2 - Edge Agent"
DEFAULT_TIMEOUT = 20.0
EXTENSIONS_URL = "edge://extensions/"
RELOAD_LABELS = ("重新加载", "Reload")
EDGE_PATHS = (
    r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
    r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
)


def fail(code: str, message: str, hint: str) -> int:
    print(f"error={code} msg={message} hint={hint}", file=sys.stderr)
    return 1


def load_desktop() -> Any:
    try:
        from pywinauto import Desktop  # noqa: PLC0415
    except ImportError:
        print(
            "error=pywinauto_missing msg=pywinauto is not installed "
            "hint=pip install pywinauto",
            file=sys.stderr,
        )
        raise SystemExit(1)
    return Desktop


def edge_executable() -> str | None:
    for path in EDGE_PATHS:
        if os.path.isfile(path):
            return path
    return shutil.which("msedge")


def process_image(pid: int) -> str:
    """返回 pid 对应的可执行文件路径；拿不到时返回空串。"""
    try:
        import win32api  # noqa: PLC0415
        import win32con  # noqa: PLC0415
        import win32process  # noqa: PLC0415

        handle = win32api.OpenProcess(
            win32con.PROCESS_QUERY_INFORMATION | win32con.PROCESS_VM_READ, False, pid
        )
        try:
            return win32process.GetModuleFileNameEx(handle, 0)
        finally:
            win32api.CloseHandle(handle)
    except Exception:  # noqa: BLE001
        return ""


def visible_edge_windows(desktop: Any) -> list[Any]:
    windows = []
    for window in desktop.windows():
        try:
            if not window.is_visible():
                continue
            if not process_image(window.element_info.process_id).lower().endswith("msedge.exe"):
                continue
            rect = window.rectangle()
            windows.append((rect.width() * rect.height(), window))
        except Exception:  # noqa: BLE001
            continue
    windows.sort(key=lambda item: item[0], reverse=True)
    return [window for _, window in windows]


def ensure_edge_window(desktop: Any, executable: str, timeout: float) -> Any | None:
    """返回一个可见 Edge 窗口；没有就启动 Edge 后继续等。"""
    deadline = time.time() + timeout
    launched = False
    while time.time() < deadline:
        windows = visible_edge_windows(desktop)
        if windows:
            return windows[0]
        if not launched:
            subprocess.Popen([executable])
            launched = True
        time.sleep(0.5)
    return None


def maximize(window: Any) -> None:
    try:
        window.restore()
    except Exception:  # noqa: BLE001
        pass
    time.sleep(0.2)
    try:
        window.maximize()
    except Exception:  # noqa: BLE001
        pass
    time.sleep(0.2)
    try:
        window.set_focus()
    except Exception:  # noqa: BLE001
        pass


def navigate_to_extensions(edge: Any) -> str:
    """把 edge://extensions/ 填进地址栏并回车；返回使用的方式。"""
    maximize(edge)
    try:
        boxes = edge.descendants(class_name="OmniboxViewViews", control_type="Edit")
        if boxes:
            boxes[0].set_edit_text(EXTENSIONS_URL)
            time.sleep(0.2)
            boxes[0].type_keys("{ENTER}")
            return "omnibox"
    except Exception:  # noqa: BLE001
        pass
    # ValuePattern 不可用时退回键盘输入。
    edge.set_focus()
    edge.type_keys("^l")
    time.sleep(0.2)
    edge.type_keys(EXTENSIONS_URL + "{ENTER}")
    return "keyboard"


def extension_version(card_name: str) -> str:
    match = re.search(r"(?:版本|version)\s*([0-9][0-9.]*)", card_name, re.IGNORECASE)
    return match.group(1) if match else "?"


def find_reload_button(desktop: Any, name: str, timeout: float) -> tuple[Any, Any, Any, str] | None:
    """在所有可见 Edge 窗口里找扩展卡片，再按 Name + Type 精确找按钮。"""
    deadline = time.time() + timeout
    while time.time() < deadline:
        for edge in visible_edge_windows(desktop):
            try:
                for card in edge.descendants(control_type="Group"):
                    if not card.window_text().startswith(name):
                        continue
                    for button in card.descendants(control_type="Button"):
                        label = button.window_text()
                        if label in RELOAD_LABELS:
                            return edge, card, button, label
            except Exception:  # noqa: BLE001
                continue
        time.sleep(0.4)
    return None


def click_button(edge: Any, button: Any) -> str:
    """优先 UIA Invoke；按钮不支持时退回鼠标点击（窗口已最大化）。"""
    maximize(edge)
    try:
        if button.is_enabled():
            button.invoke()
            return "invoke"
    except Exception:  # noqa: BLE001
        pass
    button.click_input()
    return "click_input"


def main() -> int:
    parser = argparse.ArgumentParser(description="Reload the EMA2 extension in Edge via UIA")
    parser.add_argument("--name", default=DEFAULT_NAME, help="extension card name prefix")
    parser.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT, help="seconds to wait for Edge/page/button")
    args = parser.parse_args()
    if args.timeout <= 0:
        return fail("invalid_timeout", f"--timeout must be positive, got {args.timeout}", "use e.g. --timeout 20")

    executable = edge_executable()
    if executable is None:
        return fail("edge_not_found", "cannot locate msedge.exe", "install Edge or add msedge.exe to PATH")

    Desktop = load_desktop()  # noqa: N806
    desktop = Desktop(backend="uia")

    edge = ensure_edge_window(desktop, executable, args.timeout)
    if edge is None:
        return fail(
            "edge_window_not_found",
            f"no visible msedge.exe window within {args.timeout:g}s",
            "start Edge or raise --timeout",
        )

    try:
        print(f"edge pid={edge.element_info.process_id} window={edge.window_text()!r}")
        method = navigate_to_extensions(edge)
        print(f"navigate={method} url={EXTENSIONS_URL}")
        found = find_reload_button(desktop, args.name, args.timeout)
        if found is None:
            return fail(
                "extension_not_found",
                f"no {args.name!r} card with a reload button on {EXTENSIONS_URL} within {args.timeout:g}s",
                "confirm Edge can open edge://extensions and the extension name matches; "
                "confirm which Edge window/profile has the extension loaded "
                "(with several windows this script only drives the window that owns the button)",
            )
        # 必须用命中按钮的那个窗口：多窗口/多 profile 时“面积最大窗口”可能是另一个。
        edge, card, button, label = found
        click_method = click_button(edge, button)
        version = extension_version(card.window_text())
        print(
            f"extension={args.name!r} version={version} "
            f"pid={edge.element_info.process_id} window={edge.window_text()!r}"
        )
        print(f"button={label!r} method={click_method} reloaded=1")
        return 0
    except Exception as exc:  # noqa: BLE001
        return fail("click_failed", f"{type(exc).__name__}: {exc}", "keep Edge visible and retry")
    finally:
        del desktop


if __name__ == "__main__":
    sys.exit(main())
