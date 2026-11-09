"""GTK/WebKit shell. Windows keeps its existing native Rust renderer."""

import argparse, json, os, sys, threading, time, fcntl, math, socket
from pathlib import Path
from backend import Service, clamped_position, autostart_path, set_autostart

if os.environ.get("DISPLAY") and "GDK_BACKEND" not in os.environ:
    os.environ["GDK_BACKEND"] = "x11"
import gi

gi.require_version("Gtk", "3.0")
try:
    gi.require_version("WebKit2", "4.1")
except ValueError:
    gi.require_version("WebKit2", "4.0")
from gi.repository import Gtk, Gdk, GLib, WebKit2

try:
    gi.require_version("GtkLayerShell", "0.1")
    from gi.repository import GtkLayerShell
except (ValueError, ImportError):
    GtkLayerShell = None
import cairo


def main():
    parser = argparse.ArgumentParser(description="Muisland for Linux")
    parser.add_argument(
        "--version",
        action="version",
        version="Muisland " + globals().get("MUISLAND_VERSION", "development"),
    )
    parser.add_argument("--preview", action="store_true")
    parser.add_argument(
        "--background",
        action="store_true",
        help="Start collapsed (for desktop autostart)",
    )
    parser.add_argument("--show", action="store_true", help="Open the running island")
    parser.add_argument(
        "--reset-position",
        action="store_true",
        help="Return the island to the primary monitor",
    )
    parser.add_argument("--quit", action="store_true", help="Quit the running island")
    parser.add_argument("--no-network", action="store_true")
    parser.add_argument("--check-env", action="store_true")
    parser.add_argument("--smoke-test", action="store_true")
    args = parser.parse_args()
    if args.check_env:
        print("GTK 3, WebKitGTK, Cairo, and encrypted Linux backend are available.")
        return
    cache = Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")) / "muisland"
    cache.mkdir(parents=True, exist_ok=True, mode=0o700)
    os.chmod(cache, 0o700)
    socket_path = cache / ("preview.sock" if args.preview else "app.sock")
    command = "quit" if args.quit else "reset" if args.reset_position else "show"
    lock = open(cache / ("preview.lock" if args.preview else "app.lock"), "a")
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as client:
            try:
                client.sendto(command.encode(), str(socket_path))
            except OSError:
                print("The existing instance is starting; retry in a moment.")
        return
    if args.quit:
        return
    try:
        service = Service(
            preview=args.preview or args.smoke_test,
            network=not (args.no_network or args.smoke_test),
        )
    except Exception:
        service = Service(
            preview=True, network=not (args.no_network or args.smoke_test)
        )
        service.settings["claude"] = []
        service.settings["codex"] = []
        service.startup_error = "Muisland could not unlock local storage. Unlock your desktop keyring and restart. Existing encrypted files were kept."

    html = globals().get("MUISLAND_HTML") or Path(__file__).with_name(
        "island.html"
    ).read_text(encoding="utf-8")
    GLib.set_prgname("muisland")
    GLib.set_application_name("Muisland")
    app = Shell(service, html)
    if args.reset_position:
        app.reset_position()
    if args.show:
        app.show_menu()
    if socket_path.exists():
        socket_path.unlink()
    control = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
    control.bind(str(socket_path))
    os.chmod(socket_path, 0o600)
    control.setblocking(False)

    def remote_command(_source, _condition):
        try:
            value = control.recv(32).decode()
            if value == "show":
                app.show_menu()
            elif value == "reset":
                app.reset_position()
            elif value == "quit":
                Gtk.main_quit()
        except (OSError, UnicodeError):
            pass
        return True

    GLib.io_add_watch(control, GLib.IO_IN, remote_command)
    if args.smoke_test:

        def verify():
            app.smoke_ok = len(app.loaded) == 3 and app.requests >= 3
            try:
                assert (
                    tuple(app.windows["menu"].get_size()) == app.placed["menu"]
                ), "GTK menu dimensions do not match the rendered UI"
                assert (
                    tuple(app.windows["float"].get_size()) == app.placed["float"]
                ), "GTK float dimensions do not match the rendered UI"
                screenshot = os.environ.get("MUISLAND_TEST_SCREENSHOT")
                if screenshot and app.service.preview and not app.wayland:
                    window = app.windows["menu"].get_window()
                    w, h = app.windows["menu"].get_size()
                    pixbuf = Gdk.pixbuf_get_from_window(window, 0, 0, w, h)
                    if pixbuf:
                        pixbuf.savev(screenshot, "png", [], [])
                original = app.floating
                app.close_menu()
                app.show_menu()
                app.close_menu()
                assert app.floating == original, "Opening a menu moved the float"
                app.floating = (-100000, -100000)
                app.move_float()
                area = app.area()
                assert app.floating[0] >= area.x and app.floating[1] >= area.y
                app.hover(True)
                assert app.windows["bubble"].get_visible()
                app.hover(False)
                app.reset_position()
                app.service.handle("settings", {"menu_scale": 1.6})
                app.resize_all()
                assert app.placed["menu"][1] <= app.area().height
                app.show_menu()
                if app.layer:
                    assert all(
                        GtkLayerShell.is_layer_window(w) for w in app.windows.values()
                    )
                assert not hasattr(app.service, "startup_error")
            except Exception as error:
                app.smoke_ok = False
                print(
                    "Desktop smoke failure:",
                    type(error).__name__,
                    str(error),
                    flush=True,
                )
            print(
                (
                    "Linux GTK/WebKit ["
                    + (
                        "Wayland layer shell"
                        if app.layer
                        else "Wayland managed" if app.wayland else "X11"
                    )
                    + "] startup and native bridge: "
                )
                + ("OK" if app.smoke_ok else "FAILED"),
                flush=True,
            )
            Gtk.main_quit()
            return False

        GLib.timeout_add(6000, verify)
    try:
        Gtk.main()
    finally:
        app.save_position()
        control.close()
        socket_path.unlink(missing_ok=True)
    if args.smoke_test and not app.smoke_ok:
        raise SystemExit(1)


class Shell:
    def __init__(self, service, html):
        self.service = service
        self.html = html
        self.windows = {}
        self.views = {}
        self.roles = {}
        self.floating = tuple(service.settings.get("position", (100, 80)))
        self.position_timer = 0
        self.placed = {}
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
        self.wayland = "Wayland" in Gdk.Display.get_default().__gtype__.name
        self.layer = bool(
            self.wayland and GtkLayerShell and GtkLayerShell.is_supported()
        )
        display = Gdk.Display.get_default()
        display.connect(
            "monitor-added", lambda *_: GLib.idle_add(self.monitors_changed)
        )
        display.connect(
            "monitor-removed", lambda *_: GLib.idle_add(self.monitors_changed)
        )
        Gdk.Screen.get_default().connect(
            "monitors-changed", lambda *_: GLib.idle_add(self.monitors_changed)
        )
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
        win.set_wmclass("muisland", "Muisland")
        win.set_icon_name("io.github.muhameho.Muisland")
        if self.layer:
            GtkLayerShell.init_for_window(win)
            GtkLayerShell.set_namespace(win, "muisland-" + role)
            GtkLayerShell.set_layer(win, GtkLayerShell.Layer.OVERLAY)
            GtkLayerShell.set_exclusive_zone(win, 0)
            GtkLayerShell.set_anchor(win, GtkLayerShell.Edge.TOP, True)
            GtkLayerShell.set_anchor(win, GtkLayerShell.Edge.LEFT, True)
            GtkLayerShell.set_keyboard_mode(
                win,
                (
                    GtkLayerShell.KeyboardMode.ON_DEMAND
                    if role == "menu"
                    else GtkLayerShell.KeyboardMode.NONE
                ),
            )
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
                (self.loaded.add(role), self.resize_all())
                if event == WebKit2.LoadEvent.FINISHED
                else None
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
            state["desktop"] = {
                "autostart": autostart_path().exists(),
                "backend": (
                    "Wayland layer shell"
                    if self.layer
                    else "Wayland managed window" if self.wayland else "X11 / XWayland"
                ),
            }
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
        return monitor.get_geometry() if self.layer else monitor.get_workarea()

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
            width, height = max(1, round(w * k)), max(1, round(h * k))
            self.placed[role] = (width, height)
            self.windows[role].set_size_request(width, height)
            self.windows[role].set_default_size(width, height)
            self.windows[role].resize(width, height)
            self.eval(
                role,
                "window.nativeScale=" + str(k) + ";document.body.style.zoom=" + str(k),
            )

    def position(self, role, x, y):
        win = self.windows[role]
        if self.layer:
            display = Gdk.Display.get_default()
            monitor = display.get_monitor_at_point(
                *self.floating
            ) or display.get_monitor(0)
            geometry = monitor.get_geometry()
            if GtkLayerShell.get_monitor(win) != monitor:
                GtkLayerShell.set_monitor(win, monitor)
            GtkLayerShell.set_margin(
                win, GtkLayerShell.Edge.LEFT, max(0, int(x - geometry.x))
            )
            GtkLayerShell.set_margin(
                win, GtkLayerShell.Edge.TOP, max(0, int(y - geometry.y))
            )
        elif not self.wayland:
            win.move(int(x), int(y))

    def move_float(self):
        if self.wayland and not self.layer:
            return
        area = self.area()
        self.floating = clamped_position(
            self.floating,
            self.placed["float"],
            (area.x, area.y, area.width, area.height),
        )
        self.position("float", *self.floating)
        if self.position_timer:
            GLib.source_remove(self.position_timer)
        self.position_timer = GLib.timeout_add(600, self.save_position)

    def save_position(self):
        self.position_timer = 0
        if not hasattr(self.service, "startup_error") and list(
            self.floating
        ) != self.service.settings.get("position"):
            try:
                self.service.handle("position", {"position": list(self.floating)})
            except Exception:
                pass
        return False

    def reset_position(self):
        display = Gdk.Display.get_default()
        monitor = display.get_primary_monitor() or display.get_monitor(0)
        area = monitor.get_workarea()
        self.floating = (area.x + 40, area.y + 60)
        self.close_menu()
        self.resize_all()
        self.move_float()
        self.windows["float"].show_all()
        return False

    def monitors_changed(self):
        self.resize_all()
        self.move_float()
        if self.open:
            self.show_menu()
        else:
            self.windows["bubble"].hide()
        return False

    def configured(self, role):
        if self.clamping:
            return False
        win = self.windows[role]
        if role == "float" and not self.wayland and win.get_visible():
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
        if not self.wayland or self.layer:
            area = self.area()
            fw, _ = self.placed["float"]
            w, h = self.placed["menu"]
            x, y = self.floating
            self.position(
                "menu",
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
            if not self.wayland or self.layer:
                area = self.area()
                x, y = self.floating
                fw, fh = self.placed["float"]
                bw, bh = self.placed["bubble"]
                right = x + fw / 2 < area.x + area.width / 2
                bx = x + fw + 8 if right else x - bw - 8
                by = max(
                    area.y + 8,
                    min(y + fh // 2 - bh // 2, area.y + area.height - bh - 8),
                )
                self.position(
                    "bubble", max(area.x + 8, min(bx, area.x + area.width - bw - 8)), by
                )
            if not self.windows["bubble"].get_visible():
                self.eval(
                    "bubble",
                    "(()=>{const b=document.querySelector('.bubble');if(b){b.style.animation='none';void b.offsetWidth;b.style.animation='';}})()",
                )
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
            self.down = (role, event.x_root, event.y_root, event.time, event.x, event.y)
        return False

    def motion(self, role, event):
        if self.layer and self.down and self.down[0] == role:
            dx, dy = event.x - self.down[4], event.y - self.down[5]
            if role == "float" and (self.dragging or math.hypot(dx, dy) > 4):
                self.dragging = True
                self.windows["bubble"].hide()
                self.floating = (self.floating[0] + int(dx), self.floating[1] + int(dy))
                self.move_float()
            return self.dragging
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
            GLib.timeout_add(50, self.drag_finished)
        return False

    def drag_finished(self):
        pointer = Gdk.Display.get_default().get_default_seat().get_pointer()
        mask = self.windows["float"].get_window().get_device_position(pointer)[-1]
        if mask & Gdk.ModifierType.BUTTON1_MASK:
            return True
        self.down = None
        GLib.timeout_add(100, lambda: (setattr(self, "dragging", False), False)[1])
        self.save_position()
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
        if action == "retry-storage":
            try:
                self.service = Service(network=self.service.network)
                self.floating = tuple(self.service.settings.get("position", (100, 80)))
                self.resize_all()
                self.move_float()
                answer(True, self.service.state())
                self.push()
            except Exception:
                answer(
                    False,
                    "The desktop keyring is still locked or unavailable. Unlock it and retry; existing encrypted files were kept.",
                )
            return
        if action == "reset-position":
            self.reset_position()
            answer(True, {})
            return
        if action == "autostart":
            try:
                if not self.service.preview:
                    set_autostart(bool(data.get("enabled")))
                answer(True, {})
                self.push()
            except OSError:
                answer(
                    False,
                    "Could not update desktop autostart. Check your config folder permissions.",
                )
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
