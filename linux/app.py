"""GTK/WebKit shell. Windows keeps its existing native Rust renderer."""

import argparse, json, os, sys, threading, time, fcntl, math
from pathlib import Path
from backend import Service

if os.environ.get("DISPLAY") and "GDK_BACKEND" not in os.environ:
    os.environ["GDK_BACKEND"] = "x11"
import gi

gi.require_version("Gtk", "3.0")
try:
    gi.require_version("WebKit2", "4.1")
except ValueError:
    gi.require_version("WebKit2", "4.0")
from gi.repository import Gtk, Gdk, GLib, WebKit2
import cairo


def main():
    parser = argparse.ArgumentParser(description="Muisland for Linux")
    parser.add_argument("--preview", action="store_true")
    parser.add_argument("--no-network", action="store_true")
    parser.add_argument("--check-env", action="store_true")
    parser.add_argument("--smoke-test", action="store_true")
    args = parser.parse_args()
    if args.check_env:
        print("GTK 3, WebKitGTK, Cairo, and encrypted Linux backend are available.")
        return
    cache = Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")) / "muisland"
    cache.mkdir(parents=True, exist_ok=True, mode=0o700)
    lock = open(cache / ("preview.lock" if args.preview else "app.lock"), "a")
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        print("Muisland is already running.")
        return
    try:
        service = Service(
            preview=args.preview or args.smoke_test,
            network=not (args.no_network or args.smoke_test),
        )
    except Exception:
        service = Service(preview=True)
        service.settings["claude"] = []
        service.settings["codex"] = []
        service.startup_error = "Muisland could not unlock local storage. Unlock your desktop keyring and restart. Existing encrypted files were kept."

    html = globals().get("MUISLAND_HTML") or Path(__file__).with_name(
        "island.html"
    ).read_text(encoding="utf-8")
    app = Shell(service, html)
    if args.smoke_test:

        def verify():
            app.smoke_ok = len(app.loaded) == 3 and app.requests >= 3
            print(
                "Linux GTK/WebKit startup and native bridge: "
                + ("OK" if app.smoke_ok else "FAILED"),
                flush=True,
            )
            Gtk.main_quit()
            return False

        GLib.timeout_add(6000, verify)
    Gtk.main()
    if args.smoke_test and not app.smoke_ok:
        raise SystemExit(1)


class Shell:
    def __init__(self, service, html):
        self.service = service
        self.html = html
        self.windows = {}
        self.views = {}
        self.roles = {}
        self.floating = (100, 80)
        self.open = False
        self.dragging = False
        self.down = None
        self.hide_source = 0
        self.polling = False
        self.scales = {}
        self.loaded = set()
        self.clamping = False
        self.requests = 0
        self.smoke_ok = False
        self.fades = {}
        self.wayland = not bool(os.environ.get("DISPLAY"))
        for role in ("float", "menu", "bubble"):
            self.create(role)
        self.resize_all()
        self.windows["float"].show_all()
        self.move_float()
        self.windows["float"].present()
        GLib.timeout_add(5000, self.refresh)
        GLib.timeout_add(1000, self.push)
        if self.service.preview:
            GLib.timeout_add(150, lambda: (self.show_menu(), False)[1])

    def create(self, role):
        win = Gtk.Window(type=Gtk.WindowType.TOPLEVEL)
        win.set_title("Muisland")
        win.set_decorated(False)
        win.set_resizable(False)
        win.set_keep_above(True)
        win.set_skip_taskbar_hint(True)
        win.set_skip_pager_hint(True)
        win.set_app_paintable(True)
        screen = win.get_screen()
        visual = screen.get_rgba_visual()
        if visual:
            win.set_visual(visual)
        if role == "bubble":
            win.set_accept_focus(False)
            win.set_focus_on_map(False)
            win.set_type_hint(Gdk.WindowTypeHint.TOOLTIP)
        else:
            win.set_type_hint(Gdk.WindowTypeHint.UTILITY)
        manager = WebKit2.UserContentManager()
        manager.register_script_message_handler("muisland")
        manager.add_script(
            WebKit2.UserScript.new(
                "window.ISLAND_ROLE=" + json.dumps(role) + ";",
                WebKit2.UserContentInjectedFrames.TOP_FRAME,
                WebKit2.UserScriptInjectionTime.START,
                None,
                None,
            )
        )
        manager.connect(
            "script-message-received::muisland",
            lambda _m, result: self.message(role, result),
        )
        view = WebKit2.WebView.new_with_user_content_manager(manager)
        color = Gdk.RGBA()
        color.parse("rgba(0,0,0,0)")
        view.set_background_color(color)
        view.get_settings().set_enable_developer_extras(False)
        view.connect(
            "load-changed",
            lambda _v, event: (
                self.loaded.add(role) if event == WebKit2.LoadEvent.FINISHED else None
            ),
        )
        view.connect("decide-policy", self.policy)
        win.add(view)
        self.windows[role] = win
        self.views[role] = view
        win.connect("delete-event", lambda *_: (Gtk.main_quit(), True)[1])
        win.connect("configure-event", lambda *_: self.configured(role))
        if role == "menu":
            win.connect("focus-out-event", lambda *_: (self.close_menu(), False)[1])
        if role in ("float", "menu"):
            view.add_events(
                Gdk.EventMask.BUTTON_PRESS_MASK
                | Gdk.EventMask.POINTER_MOTION_MASK
                | Gdk.EventMask.BUTTON_RELEASE_MASK
            )
            view.connect(
                "button-press-event", lambda _v, event: self.press(role, event)
            )
            view.connect(
                "motion-notify-event", lambda _v, event: self.motion(role, event)
            )
            view.connect("button-release-event", lambda *_: self.release())
        view.load_html(self.html, "file:///muisland/" + role)

    def policy(self, view, decision, kind):
        if kind == WebKit2.PolicyDecisionType.NAVIGATION_ACTION:
            uri = decision.get_navigation_action().get_request().get_uri()
            if not (uri == "about:blank" or uri.startswith("file:///muisland/")):
                decision.ignore()
                return True
        return False

    def eval(self, role, script):
        self.views[role].run_javascript(script, None, None, None)

    def push(self):
        try:
            state = self.service.state()
            if hasattr(self.service, "startup_error"):
                state["error"] = self.service.startup_error
            for role in self.views:
                self.eval(role, "window.nativePush(" + json.dumps(state) + ")")
        except Exception:
            pass
        return True

    def area(self):
        display = Gdk.Display.get_default()
        x, y = self.floating
        monitor = (
            display.get_monitor_at_point(x, y)
            or display.get_primary_monitor()
            or display.get_monitor(0)
        )
        return monitor.get_workarea()

    def resize_all(self):
        s = self.service.settings
        area = self.area()
        for role, w, h, k in [
            (
                "float",
                76 if s["circle"] else 228,
                76 if s["circle"] else 58,
                s["float_scale"],
            ),
            ("menu", 420, 628, s["menu_scale"]),
            ("bubble", 306, 206, s["bubble_scale"]),
        ]:
            k = min(k, (area.width - 16) / w, (area.height - 16) / h)
            self.scales[role] = k
            self.windows[role].resize(max(1, round(w * k)), max(1, round(h * k)))
            self.eval(role, "document.body.style.zoom=" + str(k))

    def move_float(self):
        if self.wayland:
            return
        area = self.area()
        w, h = self.windows["float"].get_size()
        x, y = self.floating
        x = max(area.x + 8, min(x, area.x + area.width - w - 8))
        y = max(area.y + 8, min(y, area.y + area.height - h - 8))
        self.floating = (x, y)
        self.windows["float"].move(x, y)

    def configured(self, role):
        if self.clamping:
            return False
        win = self.windows[role]
        if role == "float" and not self.wayland:
            self.floating = win.get_position()
            self.clamping = True
            self.move_float()
            self.clamping = False
        if win.get_window():
            w, h = win.get_size()
            radius = (
                min(w, h) // 2
                if role == "float" and self.service.settings["circle"]
                else min(30, w // 2, h // 2)
            )
            rects = []
            for y in range(h):
                dy = max(radius - y - 0.5, y + 0.5 - (h - radius), 0)
                inset = (
                    math.ceil(radius - math.sqrt(max(0, radius * radius - dy * dy)))
                    if dy
                    else 0
                )
                rects.append(cairo.RectangleInt(inset, y, max(1, w - inset * 2), 1))
            win.get_window().input_shape_combine_region(cairo.Region(rects), 0, 0)
        return False

    def fade(self, role, show):
        previous = self.fades.pop(role, None)
        if previous:
            GLib.source_remove(previous)
        win = self.windows[role]
        start = time.monotonic()
        initial = win.get_opacity() if win.get_visible() else 0.0
        target = 1.0 if show else 0.0
        if show:
            win.show_all()
        if self.service.settings["reduced_motion"]:
            win.set_opacity(target)
            if not show:
                win.hide()
            return

        def tick():
            t = min(1.0, (time.monotonic() - start) / 0.18)
            win.set_opacity(initial + (target - initial) * (1 - (1 - t) ** 3))
            if t >= 1:
                self.fades.pop(role, None)
                if not show:
                    win.hide()
                return False
            return True

        self.fades[role] = GLib.timeout_add(16, tick)

    def show_menu(self):
        self.open = True
        self.windows["bubble"].hide()
        self.windows["float"].hide()
        self.resize_all()
        if not self.wayland:
            area = self.area()
            fw, _ = self.windows["float"].get_size()
            w, h = self.windows["menu"].get_size()
            x, y = self.floating
            self.windows["menu"].move(
                max(area.x + 8, min(x + fw // 2 - w // 2, area.x + area.width - w - 8)),
                max(area.y + 8, min(y, area.y + area.height - h - 8)),
            )
        self.fade("menu", True)
        self.windows["menu"].present()
        self.push()

    def close_menu(self):
        if not self.open:
            return
        self.open = False
        if not self.service.busy:
            self.service.handle("discard")
        self.fade("menu", False)
        self.resize_all()
        self.move_float()
        self.windows["float"].show_all()
        self.windows["float"].set_opacity(1.0)
        self.push()

    def hover(self, inside):
        if self.hide_source:
            GLib.source_remove(self.hide_source)
            self.hide_source = 0
        if inside and not self.open and self.service.settings["circle"]:
            self.resize_all()
            if not self.wayland:
                area = self.area()
                x, y = self.floating
                fw, fh = self.windows["float"].get_size()
                bw, bh = self.windows["bubble"].get_size()
                right = x + fw / 2 < area.x + area.width / 2
                bx = x + fw + 8 if right else x - bw - 8
                by = max(
                    area.y + 8,
                    min(y + fh // 2 - bh // 2, area.y + area.height - bh - 8),
                )
                self.windows["bubble"].move(
                    max(area.x + 8, min(bx, area.x + area.width - bw - 8)), by
                )
            if not self.windows["bubble"].get_visible():
                self.fade("bubble", True)
            self.push()
        elif not inside:
            self.hide_source = GLib.timeout_add(
                280,
                lambda: (
                    self.fade("bubble", False),
                    setattr(self, "hide_source", 0),
                    False,
                )[2],
            )

    def press(self, role, event):
        scale = self.scales[role]
        if event.button == 1 and (
            role == "float" or event.y / scale < 50 and event.x / scale < 300
        ):
            self.down = (role, event.x_root, event.y_root, event.time)
        return False

    def motion(self, role, event):
        if (
            self.down
            and self.down[0] == role
            and math.hypot(event.x_root - self.down[1], event.y_root - self.down[2]) > 4
        ):
            self.dragging = True
            self.windows["bubble"].hide()
            self.windows[role].begin_move_drag(
                1, int(event.x_root), int(event.y_root), event.time
            )
            self.down = None
        return False

    def release(self):
        self.down = None
        GLib.timeout_add(250, lambda: (setattr(self, "dragging", False), False)[1])
        return False

    def message(self, role, result):
        try:
            message = json.loads(result.get_js_value().to_string())
            number = message["id"]
            action = message["action"]
            data = message.get("data", {})
            self.requests += 1
        except (ValueError, KeyError):
            return

        def answer(ok, value):
            self.eval(
                role,
                "window.nativeResult("
                + str(int(number))
                + ","
                + ("true" if ok else "false")
                + ","
                + json.dumps(value)
                + ")",
            )

        if action == "view":
            if not self.dragging:
                self.show_menu() if data.get("view") == "menu" else self.close_menu()
            answer(True, {})
            return
        if action == "hover":
            self.hover(bool(data.get("inside")))
            answer(True, {})
            return
        if action == "quit":
            Gtk.main_quit()
            return
        if hasattr(self.service, "startup_error") and action != "state":
            answer(False, self.service.startup_error)
            return

        def work():
            try:
                value = self.service.handle(action, data)
                if not self.open and not self.service.busy:
                    self.service.handle("discard")
                value = (
                    value
                    if isinstance(value, dict) and "folder" in value
                    else self.service.state()
                )
                GLib.idle_add(
                    lambda: (
                        answer(True, value),
                        self.resize_all(),
                        self.push(),
                        False,
                    )[3]
                )
            except Exception as error:
                if not self.open and not self.service.busy:
                    self.service.handle("discard")
                # Error messages are ours or filesystem errors, never response bodies/auth JSON.
                text = (
                    str(error)
                    if isinstance(error, (ValueError, RuntimeError, FileNotFoundError))
                    else "The operation failed safely. Check app availability, permissions, or your desktop keyring."
                )
                GLib.idle_add(lambda: (answer(False, text), False)[1])

        threading.Thread(target=work, daemon=True).start()

    def refresh(self):
        if not self.polling:
            self.polling = True

            def work():
                try:
                    self.service.poll()
                finally:
                    self.polling = False
                    GLib.idle_add(self.push)

            threading.Thread(target=work, daemon=True).start()
        return True


if __name__ == "__main__":
    main()
